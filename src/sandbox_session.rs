//! Persisted sandbox-scoped JWT session.
//!
//! Distinct from the user-scoped session in [`crate::jwt`]:
//!
//! * Minted by `POST /v1/auth/sandbox` (with no body, or
//!   `grant_type=existing_sandbox` + `sandbox_id`), not `/o/token/`.
//! * Bound to a single sandbox + workspace; the JWT carries only
//!   workspace-read + sandbox-read/write scope.
//! * Refreshed via `POST /v1/auth/sandbox` with
//!   `grant_type=refresh_token` — same endpoint as the new-mint path,
//!   dispatched by body field (mirrors `POST /o/token/`). The server
//!   does **not** rotate the refresh token. The user's own credentials
//!   are never involved — possession of the sandbox refresh token is
//!   enough.
//!
//! Stored at `~/.hotdata/sandbox_session.json` (mode 0600).

use crate::config;
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Refresh ahead of expiry to avoid racing it.
const REFRESH_LEEWAY_SECONDS: u64 = 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxSession {
    pub access_token: String,
    pub refresh_token: String,
    pub sandbox_id: String,
    pub workspace_id: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
}

pub fn session_path() -> Option<PathBuf> {
    config::config_dir()
        .ok()
        .map(|d| d.join("sandbox_session.json"))
}

#[allow(dead_code)] // Reserved for parent-side flows that resurrect a session.
pub fn load() -> Option<SandboxSession> {
    let path = session_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(session: &SandboxSession) -> Result<(), String> {
    let path = session_path().ok_or_else(|| "no sandbox session path available".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let json =
        serde_json::to_string_pretty(session).map_err(|e| format!("serialize failed: {e}"))?;

    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| format!("open failed: {e}"))?;
    f.write_all(json.as_bytes())
        .map_err(|e| format!("write failed: {e}"))?;
    Ok(())
}

pub fn clear() {
    if let Some(path) = session_path() {
        let _ = fs::remove_file(path);
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Deserialize)]
pub(crate) struct MintResponse {
    token: String,
    refresh_token: String,
    sandbox_id: String,
    expires_in: u64,
    refresh_expires_in: u64,
}

fn redact(s: &str) -> String {
    util::mask_credential(s)
}

/// Trade a refresh token for a fresh sandbox JWT. The server does
/// **not** rotate the refresh token (matches DOT's
/// ``ROTATE_REFRESH_TOKEN=False``), so the same value is returned on
/// every call. Same endpoint as the new-mint path —
/// ``POST /v1/auth/sandbox`` with ``grant_type=refresh_token`` in the
/// body, mirroring ``POST /o/token/``.
pub fn refresh(api_url: &str, refresh_token: &str) -> Result<SandboxSession, String> {
    let url = format!("{}/auth/sandbox", api_url.trim_end_matches('/'));
    let body = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    });
    let body_log = serde_json::json!({
        "grant_type": "refresh_token",
        "refresh_token": redact(refresh_token),
    });

    let client = reqwest::blocking::Client::new();
    let req = client.post(&url).json(&body);
    let (status, body_text) =
        util::send_debug_with_redaction(&client, req, Some(&body_log), &["token", "refresh_token"])
            .map_err(|e| format!("connection error: {e}"))?;
    if !status.is_success() {
        return Err(format!(
            "sandbox refresh failed: HTTP {status}: {body_text}"
        ));
    }
    let resp: MintResponse =
        serde_json::from_str(&body_text).map_err(|e| format!("malformed refresh response: {e}"))?;
    Ok(session_from_response(
        resp,
        /*workspace_id*/ String::new(),
    ))
}

/// Build a [`SandboxSession`] from a mint/refresh response. The mint
/// response itself doesn't include the workspace public_id, so the
/// caller passes it in (the workspace the sandbox was created against
/// is what the JWT's `workspaces` claim restricts the bearer to). For
/// refresh, workspace_id is left blank — the caller fills it in from
/// the prior session, since the sandbox-id ↔ workspace mapping is
/// invariant across refreshes.
pub(crate) fn session_from_response(resp: MintResponse, workspace_id: String) -> SandboxSession {
    let now = now_unix();
    SandboxSession {
        access_token: resp.token,
        refresh_token: resp.refresh_token,
        sandbox_id: resp.sandbox_id,
        workspace_id,
        access_expires_at: now + resp.expires_in,
        refresh_expires_at: now + resp.refresh_expires_in,
    }
}

/// Decode a JWT's payload (without verifying the signature) and pull
/// out the named string claim. Returns `None` if the token is
/// unparseable or the claim is missing.
fn jwt_string_claim(token: &str, claim: &str) -> Option<String> {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get(claim).and_then(|v| v.as_str()).map(String::from)
}

/// Decode the `exp` claim out of a JWT without verifying the signature.
/// Returns `None` if the token is unparseable; in that case the caller
/// should treat it as expired (force-refresh or fail).
fn jwt_exp(token: &str) -> Option<u64> {
    use base64::Engine;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(parts[1].as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    value.get("exp").and_then(|v| v.as_u64())
}

/// If `HOTDATA_SANDBOX_TOKEN` is set in the environment, return
/// `(token, sandbox_public_id)` — the sandbox public_id read from the
/// JWT's `sandbox` claim. Returns `None` if no env var is set, or if
/// the token isn't a parseable JWT (in which case we can still use it
/// as a bearer but can't identify the sandbox).
pub fn sandbox_token_in_use() -> Option<(String, Option<String>)> {
    let token = std::env::var("HOTDATA_SANDBOX_TOKEN").ok()?;
    if token.is_empty() {
        return None;
    }
    let sandbox_id = jwt_string_claim(&token, "sandbox");
    Some((token, sandbox_id))
}

/// In-child equivalent of [`ensure_access_token`] that operates on env
/// vars only — used by [`crate::sdk::Api`] when the parent
/// `sandbox run` already passed in `HOTDATA_SANDBOX_TOKEN` and
/// `HOTDATA_SANDBOX_REFRESH_TOKEN`. The new tokens are *not* persisted
/// to disk: the child may not have write access to the parent's
/// config dir (sandboxed FS), and re-doing the refresh on the next
/// invocation costs one HTTP call.
///
/// Falls back to the current `HOTDATA_SANDBOX_TOKEN` value if a
/// refresh isn't needed or fails.
pub fn refresh_from_env(api_url: &str) -> Option<String> {
    let current = std::env::var("HOTDATA_SANDBOX_TOKEN").ok()?;
    let needs_refresh = match jwt_exp(&current) {
        Some(exp) => exp.saturating_sub(REFRESH_LEEWAY_SECONDS) <= now_unix(),
        None => true,
    };
    if !needs_refresh {
        return Some(current);
    }
    let rt = std::env::var("HOTDATA_SANDBOX_REFRESH_TOKEN").ok()?;
    if rt.is_empty() {
        return Some(current);
    }
    match refresh(api_url, &rt) {
        Ok(new_session) => Some(new_session.access_token),
        Err(_) => Some(current),
    }
}

/// Return the cached sandbox session's access token, refreshing if
/// it's about to expire. Returns `None` if no session is cached, the
/// refresh token is past its TTL, or the refresh call failed.
#[allow(dead_code)] // Reserved for parent-side flows that re-use a cached session.
pub fn ensure_access_token(api_url: &str) -> Option<String> {
    let session = load()?;
    let now = now_unix();

    if !session.access_token.is_empty() && now + REFRESH_LEEWAY_SECONDS < session.access_expires_at
    {
        return Some(session.access_token);
    }

    if session.refresh_token.is_empty() || now >= session.refresh_expires_at {
        return None;
    }

    match refresh(api_url, &session.refresh_token) {
        Ok(mut new_session) => {
            // Carry workspace_id over (refresh response omits it).
            new_session.workspace_id = session.workspace_id.clone();
            let tok = new_session.access_token.clone();
            let _ = save(&new_session);
            Some(tok)
        }
        Err(_) => {
            clear();
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_helpers::with_temp_config_dir;

    fn mk_session(access_offset: i64, refresh_offset: i64) -> SandboxSession {
        let now = now_unix() as i64;
        SandboxSession {
            access_token: "cached".into(),
            refresh_token: "cached-refresh".into(),
            sandbox_id: "s_abc12345".into(),
            workspace_id: "work_xyz".into(),
            access_expires_at: (now + access_offset).max(0) as u64,
            refresh_expires_at: (now + refresh_offset).max(0) as u64,
        }
    }

    #[test]
    fn round_trip() {
        let (_tmp, _guard) = with_temp_config_dir();
        let s = mk_session(3600, 86400);
        save(&s).unwrap();
        let loaded = load().unwrap();
        assert_eq!(loaded.access_token, "cached");
        assert_eq!(loaded.sandbox_id, "s_abc12345");
        assert_eq!(loaded.workspace_id, "work_xyz");
    }

    #[test]
    fn file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _guard) = with_temp_config_dir();
        save(&mk_session(60, 60)).unwrap();
        let mode = fs::metadata(session_path().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn ensure_returns_cached_when_fresh() {
        let (_tmp, _guard) = with_temp_config_dir();
        save(&mk_session(3600, 86400)).unwrap();
        // Unreachable URL — if the code reached the network we'd see an error here.
        let tok = ensure_access_token("http://127.0.0.1:1");
        assert_eq!(tok.as_deref(), Some("cached"));
    }

    #[test]
    fn ensure_returns_none_when_no_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        assert!(ensure_access_token("http://127.0.0.1:1").is_none());
    }

    #[test]
    fn ensure_returns_none_when_refresh_dead() {
        let (_tmp, _guard) = with_temp_config_dir();
        // Access and refresh both expired.
        save(&mk_session(-10, -10)).unwrap();
        assert!(ensure_access_token("http://127.0.0.1:1").is_none());
    }

    #[test]
    fn refresh_posts_grant_type_to_sandbox_endpoint() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/auth/sandbox")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::JsonString(
                    r#"{"grant_type":"refresh_token","refresh_token":"stable-refresh"}"#
                        .to_string(),
                ),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                // Server does not rotate — same refresh_token comes back.
                r#"{"ok":true,"token":"new-jwt","refresh_token":"stable-refresh","sandbox_id":"s_abc12345","expires_in":300,"refresh_expires_in":259200}"#,
            )
            .create();

        let s = refresh(&server.url(), "stable-refresh").unwrap();
        m.assert();
        assert_eq!(s.access_token, "new-jwt");
        assert_eq!(s.refresh_token, "stable-refresh");
        assert_eq!(s.sandbox_id, "s_abc12345");
    }

    #[test]
    fn refresh_http_error() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/auth/sandbox")
            .with_status(401)
            .create();
        let err = refresh(&server.url(), "x").unwrap_err();
        m.assert();
        assert!(err.contains("401"));
    }

    #[test]
    fn ensure_refreshes_and_persists() {
        let (_tmp, _guard) = with_temp_config_dir();
        // Access expired but refresh still good.
        let mut existing = mk_session(-10, 86400);
        existing.workspace_id = "work_xyz".into();
        save(&existing).unwrap();

        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/auth/sandbox")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"token":"refreshed","refresh_token":"cached-refresh","sandbox_id":"s_abc12345","expires_in":300,"refresh_expires_in":259200}"#,
            )
            .create();
        let tok = ensure_access_token(&server.url());
        m.assert();
        assert_eq!(tok.as_deref(), Some("refreshed"));
        let after = load().unwrap();
        assert_eq!(after.access_token, "refreshed");
        // No rotation — same refresh_token as before.
        assert_eq!(after.refresh_token, "cached-refresh");
        assert_eq!(after.workspace_id, "work_xyz");
    }
}
