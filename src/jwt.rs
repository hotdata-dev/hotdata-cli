//! JWT session management for the CLI.
//!
//! A *session* is the `{access_token, refresh_token}` pair returned by
//! `/o/token/`. Access tokens are short-lived (5 min); refresh tokens
//! last 7 days for PKCE-origin sessions, 36 h for api-token-origin.
//!
//! The session is cached in `~/.hotdata/session.json` (mode 0600).
//! Before every API call, [`ensure_access_token`] decides what to do:
//!
//! | Cached state | Action |
//! |---|---|
//! | Access token valid for > 30 s | return it directly |
//! | Access expiring or expired, refresh token valid | call `/o/token/` with `grant_type=refresh_token` |
//! | Refresh token dead, `api_key` present | re-mint via `grant_type=api_token` |
//! | Refresh token dead, no `api_key` | return an error — user must `hotdata auth` again |
//!
//! The raw `hd_...` API token (flow 3 in the design doc) is *never*
//! persisted to the session file — it stays in the main config or the
//! `HOTDATA_API_KEY` env var and is only used transiently to mint.

use crate::config;
use crate::util;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const CLIENT_ID: &str = "hotdata-cli";
/// Refresh early so callers don't race an expiring token.
const REFRESH_LEEWAY_SECONDS: u64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Session {
    pub access_token: String,
    /// Unix timestamp when `access_token` expires.
    pub access_expires_at: u64,
    pub refresh_token: String,
    /// Unix timestamp when `refresh_token` hits its absolute TTL. Not
    /// precisely enforced client-side (server will reject); stored as
    /// a soft hint so we know when to skip the refresh attempt and go
    /// straight to the re-mint path.
    pub refresh_expires_at: u64,
    /// How this session was originally minted. Informational.
    #[serde(default)]
    pub source: String,
}

/// Path to the session cache file. Returns `None` if the home
/// directory can't be resolved — in which case we operate without
/// caching.
pub fn session_path() -> Option<PathBuf> {
    config::config_dir().ok().map(|d| d.join("session.json"))
}

pub fn load_session() -> Option<Session> {
    let path = session_path()?;
    let raw = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_session(session: &Session) -> Result<(), String> {
    let path = session_path().ok_or_else(|| "no session path available".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let json = serde_json::to_string_pretty(session).map_err(|e| format!("serialize failed: {e}"))?;

    // mode 0600 — session file contains a refresh token, treat it like a
    // credential on disk.
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

pub fn clear_session() {
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
struct TokenResponse {
    access_token: String,
    expires_in: u64,
    refresh_token: Option<String>,
}

fn session_from_response(resp: TokenResponse, fallback_refresh: Option<String>, source: &str) -> Session {
    let refresh_token = resp.refresh_token.or(fallback_refresh).unwrap_or_default();
    // We don't know the exact refresh TTL server-side (7 d or 36 h
    // depending on origin). Store a conservative estimate so we don't
    // refresh-attempt with a known-dead token; server enforces the
    // real deadline.
    let refresh_ttl = if source == "api_token" {
        36 * 60 * 60
    } else {
        7 * 24 * 60 * 60
    };
    Session {
        access_token: resp.access_token,
        access_expires_at: now_unix() + resp.expires_in,
        refresh_token,
        refresh_expires_at: now_unix() + refresh_ttl,
        source: source.to_string(),
    }
}

fn oauth_base(profile: &config::ProfileConfig) -> String {
    // DOT (`/o/authorize/`, `/o/token/`, …) is mounted on the webapp
    // (app_url), not the API. The api_url host typically only serves
    // the `/v1` runtimedb routes.
    profile.app_url.to_string().trim_end_matches('/').to_string()
}

/// Build a redacted JSON view of a form body for `--debug` printing.
/// `util::send_debug` takes the printable body separately from the
/// wire body, so we hand it this masked view while the actual `.form()`
/// payload sends real values.
fn redacted_form_body(params: &[(&str, &str)]) -> serde_json::Value {
    let masked: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .map(|(k, v)| {
            let display = match *k {
                "code" | "code_verifier" | "api_token" | "refresh_token" => {
                    util::mask_credential(v)
                }
                _ => v.to_string(),
            };
            (k.to_string(), serde_json::Value::String(display))
        })
        .collect();
    serde_json::Value::Object(masked)
}

/// Token-endpoint responses contain the access + refresh JWTs in
/// plaintext. Mask both before printing, but return the unredacted
/// body so the caller can still parse real values out of it.
const TOKEN_REDACT_KEYS: &[&str] = &["access_token", "refresh_token"];

/// Exchange a PKCE authorization code for a session.
pub fn mint_from_pkce_code(
    profile: &config::ProfileConfig,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<Session, String> {
    let url = format!("{}/o/token/", oauth_base(profile));
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
        ("client_id", CLIENT_ID),
    ];

    let client = reqwest::blocking::Client::new();
    let req = client.post(&url).form(&params);
    let body_log = redacted_form_body(&params);
    let (status, body_text) = util::send_debug_with_redaction(
        &client,
        req,
        Some(&body_log),
        TOKEN_REDACT_KEYS,
    )
    .map_err(|e| format!("connection error: {e}"))?;
    if !status.is_success() {
        return Err(format!("token exchange failed: HTTP {status}: {body_text}"));
    }
    let body: TokenResponse = serde_json::from_str(&body_text)
        .map_err(|e| format!("malformed token response: {e}"))?;
    Ok(session_from_response(body, None, "pkce"))
}

/// Exchange an opaque API token for a session.
pub fn mint_from_api_token(
    profile: &config::ProfileConfig,
    api_token: &str,
) -> Result<Session, String> {
    let url = format!("{}/o/token/", oauth_base(profile));
    let params = [
        ("grant_type", "api_token"),
        ("api_token", api_token),
        ("client_id", CLIENT_ID),
    ];

    let client = reqwest::blocking::Client::new();
    let req = client.post(&url).form(&params);
    let body_log = redacted_form_body(&params);
    let (status, body_text) = util::send_debug_with_redaction(
        &client,
        req,
        Some(&body_log),
        TOKEN_REDACT_KEYS,
    )
    .map_err(|e| format!("connection error: {e}"))?;
    if !status.is_success() {
        return Err(format!("api_token exchange failed: HTTP {status}: {body_text}"));
    }
    let body: TokenResponse = serde_json::from_str(&body_text)
        .map_err(|e| format!("malformed token response: {e}"))?;
    Ok(session_from_response(body, None, "api_token"))
}

/// Refresh an existing session via the refresh-token grant.
pub fn refresh(profile: &config::ProfileConfig, session: &Session) -> Result<Session, String> {
    let url = format!("{}/o/token/", oauth_base(profile));
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", session.refresh_token.as_str()),
        ("client_id", CLIENT_ID),
    ];

    let client = reqwest::blocking::Client::new();
    let req = client.post(&url).form(&params);
    let body_log = redacted_form_body(&params);
    let (status, body_text) = util::send_debug_with_redaction(
        &client,
        req,
        Some(&body_log),
        TOKEN_REDACT_KEYS,
    )
    .map_err(|e| format!("connection error: {e}"))?;
    if !status.is_success() {
        return Err(format!("refresh failed: HTTP {status}: {body_text}"));
    }
    let body: TokenResponse = serde_json::from_str(&body_text)
        .map_err(|e| format!("malformed token response: {e}"))?;
    Ok(session_from_response(
        body,
        // Rotation is off server-side, so the same refresh token
        // should come back — but fall back to the old one if the
        // server decides to drop it from the response.
        Some(session.refresh_token.clone()),
        &session.source,
    ))
}

/// Return a valid access token, minting or refreshing as needed.
///
/// The caller passes in whatever credential they want to fall back on
/// (an `hd_...` API key from `--api-key`, env var, or config). If the
/// cached session is usable it's returned without touching the API;
/// otherwise the session is refreshed/re-minted and persisted.
pub fn ensure_access_token(
    profile: &config::ProfileConfig,
    api_key_fallback: Option<&str>,
) -> Result<String, String> {
    // 0) An explicit identity override (`--api-key`, `HOTDATA_API_KEY`,
    // or `.env`) is asserting a specific identity for *this invocation*.
    // The on-disk session may belong to a completely different user
    // from a prior `hotdata auth` and must not be reused. Mint fresh
    // and deliberately skip persisting so we don't clobber the
    // interactive session. Surface the real mint error here too — if
    // the override key is bad, "HTTP 401" is more useful than the
    // generic "session expired" message the cache-fallthrough returns.
    //
    // Only `ApiKeySource::Config` continues to honor the cache: that's
    // a stable identity persisted in config.yml, paired with a session
    // minted from that same identity.
    if matches!(
        profile.api_key_source,
        config::ApiKeySource::Flag | config::ApiKeySource::Env
    ) && let Some(api_key) = api_key_fallback
    {
        let session = mint_from_api_token(profile, api_key)?;
        return Ok(session.access_token);
    }

    let now = now_unix();

    // 1) Cached session is still good.
    if let Some(session) = load_session() {
        if !session.access_token.is_empty() && now + REFRESH_LEEWAY_SECONDS < session.access_expires_at {
            return Ok(session.access_token);
        }

        // 2) Access expired but refresh might still work.
        if !session.refresh_token.is_empty() && now < session.refresh_expires_at {
            match refresh(profile, &session) {
                Ok(new_session) => {
                    let tok = new_session.access_token.clone();
                    let _ = save_session(&new_session);
                    return Ok(tok);
                }
                Err(_) => {
                    // Refresh rejected — fall through to re-mint.
                    clear_session();
                }
            }
        }
    }

    // 3) No cache, or refresh is dead → need a fresh mint.
    if let Some(api_key) = api_key_fallback {
        match mint_from_api_token(profile, api_key) {
            Ok(session) => {
                let tok = session.access_token.clone();
                save_session(&session)?;
                return Ok(tok);
            }
            Err(_) => {
                // API token rejected (revoked, expired, or invalid).
                // Fall through to the re-auth hint — hide the raw HTTP
                // error from the user; the api.rs caller appends a
                // `hotdata auth` hint.
            }
        }
    }

    Err("session expired or revoked".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiUrl, AppUrl, ProfileConfig, test_helpers::with_temp_config_dir};

    fn mock_profile(url: &str) -> ProfileConfig {
        ProfileConfig {
            app_url: AppUrl(Some(url.to_string())),
            api_url: ApiUrl(Some(url.to_string())),
            ..Default::default()
        }
    }

    fn cached_session(access_offset: i64, refresh_offset: i64) -> Session {
        let now = now_unix() as i64;
        Session {
            access_token: "cached-jwt".into(),
            access_expires_at: (now + access_offset).max(0) as u64,
            refresh_token: "cached-refresh".into(),
            refresh_expires_at: (now + refresh_offset).max(0) as u64,
            source: "pkce".into(),
        }
    }

    // --- session persistence ---

    #[test]
    fn session_round_trip() {
        let (_tmp, _guard) = with_temp_config_dir();
        let s = Session {
            access_token: "a".into(),
            access_expires_at: 100,
            refresh_token: "r".into(),
            refresh_expires_at: 200,
            source: "pkce".into(),
        };
        save_session(&s).unwrap();
        let loaded = load_session().unwrap();
        assert_eq!(loaded.access_token, "a");
        assert_eq!(loaded.access_expires_at, 100);
        assert_eq!(loaded.refresh_token, "r");
        assert_eq!(loaded.refresh_expires_at, 200);
        assert_eq!(loaded.source, "pkce");
    }

    #[test]
    fn session_file_is_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _guard) = with_temp_config_dir();
        save_session(&Session::default()).unwrap();
        let path = session_path().unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "session file must be 0600 (contains refresh token)");
    }

    #[test]
    fn load_session_returns_none_when_missing() {
        let (_tmp, _guard) = with_temp_config_dir();
        assert!(load_session().is_none());
    }

    #[test]
    fn load_session_returns_none_when_corrupt() {
        let (_tmp, _guard) = with_temp_config_dir();
        let path = session_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json").unwrap();
        assert!(load_session().is_none());
    }

    #[test]
    fn clear_session_removes_file() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_session(&Session::default()).unwrap();
        assert!(load_session().is_some());
        clear_session();
        assert!(load_session().is_none());
        // Idempotent — clearing again is a no-op.
        clear_session();
    }

    // --- mint_from_pkce_code ---

    #[test]
    fn mint_from_pkce_code_success() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "authorization_code".into()),
                mockito::Matcher::UrlEncoded("code".into(), "auth-code".into()),
                mockito::Matcher::UrlEncoded("code_verifier".into(), "verifier".into()),
                mockito::Matcher::UrlEncoded(
                    "redirect_uri".into(),
                    "http://127.0.0.1:1234/".into(),
                ),
                mockito::Matcher::UrlEncoded("client_id".into(), "hotdata-cli".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"access_token":"jwt-abc","expires_in":300,"refresh_token":"refresh-xyz"}"#,
            )
            .create();

        let profile = mock_profile(&server.url());
        let session =
            mint_from_pkce_code(&profile, "auth-code", "verifier", "http://127.0.0.1:1234/")
                .unwrap();
        m.assert();
        assert_eq!(session.access_token, "jwt-abc");
        assert_eq!(session.refresh_token, "refresh-xyz");
        assert_eq!(session.source, "pkce");
        assert!(session.access_expires_at > now_unix());
        // PKCE-origin sessions get the 7-day refresh TTL hint.
        let ttl = session.refresh_expires_at.saturating_sub(now_unix());
        assert!((7 * 24 * 60 * 60 - 5..=7 * 24 * 60 * 60 + 5).contains(&ttl));
    }

    #[test]
    fn mint_from_pkce_code_trims_trailing_slash_in_app_url() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"a","expires_in":1,"refresh_token":"r"}"#)
            .create();

        // Append a trailing slash — oauth_base must strip it so we don't
        // end up POSTing to `//o/token/`.
        let url = format!("{}/", server.url());
        let profile = mock_profile(&url);
        mint_from_pkce_code(&profile, "c", "v", "uri").unwrap();
        m.assert();
    }

    #[test]
    fn mint_from_pkce_code_http_error_includes_status_and_body() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .with_status(403)
            .with_body("forbidden by policy")
            .create();

        let profile = mock_profile(&server.url());
        let err = mint_from_pkce_code(&profile, "c", "v", "uri").unwrap_err();
        m.assert();
        assert!(err.contains("403"), "got: {err}");
        assert!(err.contains("forbidden by policy"), "got: {err}");
    }

    #[test]
    fn mint_from_pkce_code_malformed_response() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .with_status(200)
            .with_body("not json")
            .create();

        let profile = mock_profile(&server.url());
        let err = mint_from_pkce_code(&profile, "c", "v", "uri").unwrap_err();
        m.assert();
        assert!(err.contains("malformed"), "got: {err}");
    }

    #[test]
    fn mint_from_pkce_code_connection_error() {
        let profile = mock_profile("http://127.0.0.1:1");
        let err = mint_from_pkce_code(&profile, "c", "v", "uri").unwrap_err();
        assert!(err.contains("connection"), "got: {err}");
    }

    // --- mint_from_api_token ---

    #[test]
    fn mint_from_api_token_success_uses_36h_refresh_ttl() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "api_token".into()),
                mockito::Matcher::UrlEncoded("api_token".into(), "hd_xyz".into()),
                mockito::Matcher::UrlEncoded("client_id".into(), "hotdata-cli".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"jwt-1","expires_in":300,"refresh_token":"r1"}"#)
            .create();

        let profile = mock_profile(&server.url());
        let session = mint_from_api_token(&profile, "hd_xyz").unwrap();
        m.assert();
        assert_eq!(session.access_token, "jwt-1");
        assert_eq!(session.refresh_token, "r1");
        assert_eq!(session.source, "api_token");
        // api_token-origin sessions get the shorter 36h refresh TTL hint.
        let ttl = session.refresh_expires_at.saturating_sub(now_unix());
        assert!((36 * 60 * 60 - 5..=36 * 60 * 60 + 5).contains(&ttl));
    }

    #[test]
    fn mint_from_api_token_http_error() {
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/o/token/").with_status(401).create();

        let profile = mock_profile(&server.url());
        let err = mint_from_api_token(&profile, "bad-key").unwrap_err();
        m.assert();
        assert!(err.contains("401"), "got: {err}");
    }

    // --- refresh ---

    #[test]
    fn refresh_keeps_old_refresh_token_when_server_omits_it() {
        // Rotation-off case: server returns no refresh_token, and we
        // must carry the old one forward so the next refresh works.
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "refresh_token".into()),
                mockito::Matcher::UrlEncoded("refresh_token".into(), "stable-refresh".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"new-jwt","expires_in":300}"#)
            .create();

        let profile = mock_profile(&server.url());
        let session = Session {
            refresh_token: "stable-refresh".into(),
            source: "pkce".into(),
            ..Default::default()
        };
        let new_session = refresh(&profile, &session).unwrap();
        m.assert();
        assert_eq!(new_session.access_token, "new-jwt");
        assert_eq!(new_session.refresh_token, "stable-refresh");
        // Source is carried over from the original session.
        assert_eq!(new_session.source, "pkce");
    }

    #[test]
    fn refresh_uses_rotated_refresh_token_when_server_returns_one() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"access_token":"new-jwt","expires_in":300,"refresh_token":"rotated"}"#,
            )
            .create();

        let profile = mock_profile(&server.url());
        let session = Session {
            refresh_token: "old".into(),
            source: "api_token".into(),
            ..Default::default()
        };
        let new_session = refresh(&profile, &session).unwrap();
        m.assert();
        assert_eq!(new_session.refresh_token, "rotated");
        assert_eq!(new_session.source, "api_token");
    }

    #[test]
    fn refresh_http_error() {
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/o/token/").with_status(400).create();

        let profile = mock_profile(&server.url());
        let session = Session {
            refresh_token: "x".into(),
            ..Default::default()
        };
        let err = refresh(&profile, &session).unwrap_err();
        m.assert();
        assert!(err.contains("400"), "got: {err}");
    }

    // --- ensure_access_token: each branch of the decision table ---

    #[test]
    fn ensure_returns_cached_token_without_http_when_valid() {
        let (_tmp, _guard) = with_temp_config_dir();
        // 10 min into the future, well past REFRESH_LEEWAY_SECONDS.
        save_session(&cached_session(600, 7 * 24 * 3600)).unwrap();

        // Profile points at a port that's not listening — if the code
        // reached out to the network this would surface as an error.
        let profile = mock_profile("http://127.0.0.1:1");
        let token = ensure_access_token(&profile, None).unwrap();
        assert_eq!(token, "cached-jwt");
    }

    #[test]
    fn ensure_refreshes_when_inside_leeway_window() {
        // Token still has a few seconds left but is inside the 30s
        // leeway, so the orchestrator should refresh proactively.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"refreshed-jwt","expires_in":300}"#)
            .create();

        save_session(&cached_session(5, 86400)).unwrap();
        let profile = mock_profile(&server.url());
        let token = ensure_access_token(&profile, None).unwrap();
        m.assert();
        assert_eq!(token, "refreshed-jwt");
        // New session was persisted to disk.
        assert_eq!(load_session().unwrap().access_token, "refreshed-jwt");
    }

    #[test]
    fn ensure_refreshes_when_access_expired() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"refreshed-jwt","expires_in":300}"#)
            .create();

        save_session(&cached_session(-10, 86400)).unwrap();
        let profile = mock_profile(&server.url());
        let token = ensure_access_token(&profile, None).unwrap();
        m.assert();
        assert_eq!(token, "refreshed-jwt");
    }

    #[test]
    fn ensure_falls_back_to_api_token_mint_when_refresh_rejected() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let refresh_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            ))
            .with_status(400)
            .with_body("invalid_grant")
            .create();
        let mint_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "api_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"reminted-jwt","expires_in":300,"refresh_token":"r2"}"#)
            .create();

        save_session(&cached_session(-10, 86400)).unwrap();
        let profile = mock_profile(&server.url());
        let token = ensure_access_token(&profile, Some("hd_xyz")).unwrap();
        refresh_mock.assert();
        mint_mock.assert();
        assert_eq!(token, "reminted-jwt");
        let loaded = load_session().unwrap();
        assert_eq!(loaded.access_token, "reminted-jwt");
        assert_eq!(loaded.source, "api_token");
    }

    #[test]
    fn ensure_mints_from_api_token_when_no_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "api_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"fresh-jwt","expires_in":300,"refresh_token":"r"}"#)
            .create();

        let profile = mock_profile(&server.url());
        let token = ensure_access_token(&profile, Some("hd_xyz")).unwrap();
        m.assert();
        assert_eq!(token, "fresh-jwt");
        assert_eq!(load_session().unwrap().access_token, "fresh-jwt");
    }

    #[test]
    fn ensure_skips_refresh_when_refresh_ttl_expired() {
        // Refresh token is past its soft TTL — the orchestrator should
        // skip the refresh attempt entirely and go straight to the
        // api_token re-mint path.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let mint_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "api_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"reminted","expires_in":300,"refresh_token":"r"}"#)
            .expect(1)
            .create();
        // Refresh path must NOT be hit.
        let refresh_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "refresh_token".into(),
            ))
            .expect(0)
            .create();

        save_session(&cached_session(-10, -10)).unwrap();
        let profile = mock_profile(&server.url());
        let token = ensure_access_token(&profile, Some("hd_xyz")).unwrap();
        mint_mock.assert();
        refresh_mock.assert();
        assert_eq!(token, "reminted");
    }

    #[test]
    fn ensure_errors_when_no_session_and_no_api_key() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1");
        let err = ensure_access_token(&profile, None).unwrap_err();
        assert!(err.contains("session"), "got: {err}");
    }

    #[test]
    fn ensure_errors_when_api_token_rejected() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "api_token".into(),
            ))
            .with_status(401)
            .create();

        let profile = mock_profile(&server.url());
        let err = ensure_access_token(&profile, Some("revoked")).unwrap_err();
        m.assert();
        // Error is the generic "session expired or revoked" — the raw
        // HTTP status is suppressed so api.rs can append a clean
        // re-auth hint.
        assert!(err.contains("session"), "got: {err}");
    }

    // --- ensure_access_token: --api-key (Flag source) overrides cache ---

    #[test]
    fn ensure_with_flag_source_bypasses_valid_cached_session() {
        // A perfectly valid PKCE session is on disk, but the user
        // passed --api-key — we must mint a fresh JWT from that key
        // instead of reusing the cached session.
        let (_tmp, _guard) = with_temp_config_dir();
        save_session(&cached_session(3600, 7 * 24 * 3600)).unwrap();

        let mut server = mockito::Server::new();
        let mint_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "api_token".into()),
                mockito::Matcher::UrlEncoded("api_token".into(), "hd_flag_token".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"flag-jwt","expires_in":300,"refresh_token":"r"}"#)
            .create();

        let mut profile = mock_profile(&server.url());
        profile.api_key_source = config::ApiKeySource::Flag;
        let token = ensure_access_token(&profile, Some("hd_flag_token")).unwrap();
        mint_mock.assert();
        assert_eq!(token, "flag-jwt");
    }

    #[test]
    fn ensure_with_flag_source_does_not_overwrite_cached_session() {
        // Flag-driven mints are for one-shot CLI invocations; persisting
        // them would silently log the interactive user out.
        let (_tmp, _guard) = with_temp_config_dir();
        let original = cached_session(3600, 7 * 24 * 3600);
        save_session(&original).unwrap();

        let mut server = mockito::Server::new();
        let _mint = server
            .mock("POST", "/o/token/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"flag-jwt","expires_in":300,"refresh_token":"r"}"#)
            .create();

        let mut profile = mock_profile(&server.url());
        profile.api_key_source = config::ApiKeySource::Flag;
        ensure_access_token(&profile, Some("hd_flag_token")).unwrap();

        // session.json must still hold the original PKCE session.
        let after = load_session().unwrap();
        assert_eq!(after.access_token, original.access_token);
        assert_eq!(after.refresh_token, original.refresh_token);
    }

    #[test]
    fn ensure_with_flag_source_surfaces_mint_error() {
        // When --api-key was passed explicitly, the user wants the real
        // failure reason, not the generic "session expired or revoked"
        // message that the cache-fall-through path returns.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/o/token/")
            .with_status(401)
            .with_body("invalid api token")
            .create();

        let mut profile = mock_profile(&server.url());
        profile.api_key_source = config::ApiKeySource::Flag;
        let err = ensure_access_token(&profile, Some("bad")).unwrap_err();
        m.assert();
        assert!(err.contains("401"), "got: {err}");
    }

    #[test]
    fn ensure_with_env_source_bypasses_valid_cached_session() {
        // HOTDATA_API_KEY (whether exported in the shell or loaded
        // from .env) must override a cached session for the same
        // reason --api-key does: the env var asserts a specific
        // identity for this invocation.
        let (_tmp, _guard) = with_temp_config_dir();
        let original = cached_session(3600, 7 * 24 * 3600);
        save_session(&original).unwrap();

        let mut server = mockito::Server::new();
        let mint_mock = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("grant_type".into(), "api_token".into()),
                mockito::Matcher::UrlEncoded("api_token".into(), "hd_env_token".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"access_token":"env-jwt","expires_in":300,"refresh_token":"r"}"#)
            .create();

        let mut profile = mock_profile(&server.url());
        profile.api_key_source = config::ApiKeySource::Env;
        let token = ensure_access_token(&profile, Some("hd_env_token")).unwrap();
        mint_mock.assert();
        assert_eq!(token, "env-jwt");

        // Cached session must remain untouched — same no-clobber
        // guarantee as the Flag path.
        let after = load_session().unwrap();
        assert_eq!(after.access_token, original.access_token);
    }

    #[test]
    fn ensure_with_config_source_still_uses_cached_session() {
        // Regression guard: api_key_source = Config (the default) must
        // continue to short-circuit on a valid cache, not mint.
        let (_tmp, _guard) = with_temp_config_dir();
        save_session(&cached_session(3600, 7 * 24 * 3600)).unwrap();

        let profile = mock_profile("http://127.0.0.1:1");
        // Config source — even with an api_key fallback, the cache wins.
        assert_eq!(profile.api_key_source, config::ApiKeySource::Config);
        let token = ensure_access_token(&profile, Some("hd_config_key")).unwrap();
        assert_eq!(token, "cached-jwt");
    }

    #[test]
    fn ensure_clears_session_when_refresh_dies_with_no_fallback() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/o/token/").with_status(400).create();

        save_session(&cached_session(-10, 86400)).unwrap();
        let profile = mock_profile(&server.url());
        let err = ensure_access_token(&profile, None).unwrap_err();
        m.assert();
        assert!(err.contains("session"), "got: {err}");
        // Stale session must be cleared so the next attempt doesn't
        // burn a network call on the same dead refresh token.
        assert!(load_session().is_none());
    }
}
