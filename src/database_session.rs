//! Persisted database-scoped JWT session.
//!
//! Minted by `POST /v1/auth/database` (grant_type=existing_database +
//! database_id), refreshed via the same endpoint with
//! grant_type=refresh_token. Bound to a single database + workspace;
//! the JWT carries workspace + database read/write scope. The server
//! does not rotate the refresh token.
//!
//! Stored at `~/.hotdata/database_session.json` (mode 0600).

use crate::config;
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const REFRESH_LEEWAY_SECONDS: u64 = 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DatabaseSession {
    pub access_token: String,
    pub refresh_token: String,
    pub database_id: String,
    pub workspace_id: String,
    pub access_expires_at: u64,
    pub refresh_expires_at: u64,
}

pub fn session_path() -> Option<PathBuf> {
    config::config_dir().ok().map(|d| d.join("database_session.json"))
}

#[allow(dead_code)] // Reserved for flows that re-use a cached database session.
pub fn load() -> Option<DatabaseSession> {
    let path = session_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save(session: &DatabaseSession) -> Result<(), String> {
    let path = session_path().ok_or_else(|| "no database session path available".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let json = serde_json::to_string_pretty(session)
        .map_err(|e| format!("serialize failed: {e}"))?;

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

#[allow(dead_code)] // Reserved for flows that re-use a cached database session.
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
    database_id: String,
    expires_in: u64,
    refresh_expires_in: u64,
}

fn redact(s: &str) -> String {
    util::mask_credential(s)
}

/// Trade a refresh token for a fresh database JWT (no rotation). Same
/// endpoint as the new-mint path: `POST /v1/auth/database` with
/// grant_type=refresh_token.
pub fn refresh(api_url: &str, refresh_token: &str) -> Result<DatabaseSession, String> {
    let url = format!("{}/auth/database", api_url.trim_end_matches('/'));
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
    let (status, body_text) = util::send_debug_with_redaction(
        &client,
        req,
        Some(&body_log),
        &["token", "refresh_token"],
    )
    .map_err(|e| format!("connection error: {e}"))?;
    if !status.is_success() {
        return Err(format!("database refresh failed: HTTP {status}: {body_text}"));
    }
    let resp: MintResponse = serde_json::from_str(&body_text)
        .map_err(|e| format!("malformed refresh response: {e}"))?;
    Ok(session_from_response(resp, String::new()))
}

/// Build a [`DatabaseSession`] from a mint/refresh response. The mint
/// response doesn't carry the workspace public_id, so the caller passes
/// it in (it's what the JWT's `workspaces` claim restricts the bearer
/// to). For refresh, `workspace_id` is left blank — the caller fills it
/// from the prior session, since the database-id ↔ workspace mapping is
/// invariant across refreshes.
pub(crate) fn session_from_response(resp: MintResponse, workspace_id: String) -> DatabaseSession {
    let now = now_unix();
    DatabaseSession {
        access_token: resp.token,
        refresh_token: resp.refresh_token,
        database_id: resp.database_id,
        workspace_id,
        access_expires_at: now + resp.expires_in,
        refresh_expires_at: now + resp.refresh_expires_in,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::test_helpers::with_temp_config_dir;

    fn mk_session(access_offset: i64, refresh_offset: i64) -> DatabaseSession {
        let now = now_unix() as i64;
        DatabaseSession {
            access_token: "cached".into(),
            refresh_token: "cached-refresh".into(),
            database_id: "dbid_abc".into(),
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
        assert_eq!(loaded.database_id, "dbid_abc");
        assert_eq!(loaded.workspace_id, "work_xyz");
    }

    #[test]
    fn file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _guard) = with_temp_config_dir();
        save(&mk_session(60, 60)).unwrap();
        let mode = fs::metadata(session_path().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn refresh_posts_grant_type_to_database_endpoint() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/auth/database")
            .match_body(mockito::Matcher::JsonString(
                r#"{"grant_type":"refresh_token","refresh_token":"stable-refresh"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"token":"new-jwt","refresh_token":"stable-refresh","database_id":"dbid_abc","expires_in":300,"refresh_expires_in":259200}"#,
            )
            .create();

        let s = refresh(&server.url(), "stable-refresh").unwrap();
        m.assert();
        assert_eq!(s.access_token, "new-jwt");
        assert_eq!(s.refresh_token, "stable-refresh");
        assert_eq!(s.database_id, "dbid_abc");
    }

    #[test]
    fn refresh_http_error() {
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/auth/database").with_status(401).create();
        let err = refresh(&server.url(), "x").unwrap_err();
        m.assert();
        assert!(err.contains("401"));
    }
}
