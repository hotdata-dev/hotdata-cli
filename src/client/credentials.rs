//! Credential inspection: validate the active profile's auth state and read
//! claims (workspace scope, token source) from a minted api-key JWT.
//!
//! This is the infrastructure half of auth — consumed by the SDK seam and by
//! `main`'s workspace resolution. The interactive login/register/status UI
//! lives in [`crate::commands::auth`], which depends on this module (never the
//! reverse).

use crate::config::{self, ApiKeySource};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Debug, PartialEq)]
pub enum AuthStatus {
    Authenticated,
    NotConfigured,
    Invalid(u16),
    ConnectionError(String),
}

pub fn check_status(profile_config: &config::ProfileConfig) -> AuthStatus {
    // Same precedence as the SDK seam: user-scoped CLI session / api_key
    // fallback.
    let api_key_fallback = profile_config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");

    // PKCE-origin sessions don't write an api_key, so absence of a key
    // alone isn't "not configured" — only true if there's also no
    // cached JWT session to validate.
    if api_key_fallback.is_none() && crate::client::jwt::load_session().is_none() {
        return AuthStatus::NotConfigured;
    }

    let access_token =
        match crate::client::jwt::ensure_access_token(profile_config, api_key_fallback) {
            Ok(t) => t,
            Err(_) => return AuthStatus::Invalid(401),
        };

    let url = format!("{}/workspaces", profile_config.api_url);
    let client = reqwest::blocking::Client::new();
    let req = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"));
    match crate::util::send_debug(&client, req, None) {
        Ok((status, _)) if status.is_success() => AuthStatus::Authenticated,
        Ok((status, _)) => AuthStatus::Invalid(status.as_u16()),
        Err(e) => AuthStatus::ConnectionError(e.to_string()),
    }
}

/// Workspace public-ids the active api-key credential (`--api-key` /
/// `HOTDATA_API_KEY`) is scoped to, read from its minted JWT's `workspaces`
/// claim. A database API token carries exactly one. Empty when there's no api
/// key, it can't be exchanged, or the claim is absent (an unrestricted token).
pub(crate) fn api_key_workspace_ids(profile_config: &config::ProfileConfig) -> Vec<String> {
    let Some(key) = profile_config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty() && *k != "PLACEHOLDER")
    else {
        return Vec::new();
    };
    let Ok(token) = crate::client::jwt::ensure_access_token(profile_config, Some(key)) else {
        return Vec::new();
    };
    jwt_array_claim(&token, "workspaces")
}

/// When the active credential is a user-supplied api key (`--api-key` /
/// `HOTDATA_API_KEY`), exchange it for a JWT and return that JWT's `source`
/// claim (e.g. `database_api_token`). This lets `auth status` double as a
/// validator: a successful mint proves the key is accepted, and the source
/// confirms which kind of token it is. Returns `None` for CLI-session
/// credentials or if the key can't be exchanged.
pub(crate) fn api_key_jwt_source(profile_config: &config::ProfileConfig) -> Option<String> {
    if !matches!(
        profile_config.api_key_source,
        ApiKeySource::Flag | ApiKeySource::Env
    ) {
        return None;
    }
    let key = profile_config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty() && *k != "PLACEHOLDER")?;
    let token = crate::client::jwt::ensure_access_token(profile_config, Some(key)).ok()?;
    jwt_string_claim(&token, "source")
}

/// Decode a JWT payload (no signature verification) and return the named
/// string claim. Mirrors the decoder in `database_session` — the server
/// validates signatures on receipt, so the CLI only peeks at claims.
fn jwt_string_claim(token: &str, claim: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get(claim).and_then(|v| v.as_str()).map(String::from)
}

/// Decode a JWT payload (no signature verification) and return the named claim
/// as a list of strings. Empty when the token is unparseable or the claim is
/// absent / not a string array (e.g. the `workspaces` scope claim).
fn jwt_array_claim(token: &str, claim: &str) -> Vec<String> {
    token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|value| {
            value.get(claim).and_then(|c| c.as_array()).map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::{ApiUrl, AppUrl, ProfileConfig, test_helpers::with_temp_config_dir};

    fn mock_profile(url: &str, api_key: Option<&str>) -> ProfileConfig {
        ProfileConfig {
            api_key: api_key.map(String::from),
            api_url: ApiUrl(Some(url.to_string())),
            // Point app_url at the same server so any oauth path (e.g.
            // ensure_access_token minting from an api_key) hits the
            // mock instead of the real production app.
            app_url: AppUrl(Some(url.to_string())),
            ..Default::default()
        }
    }

    /// Persist a fully-valid session so check_status can short-circuit
    /// the JWT mint/refresh path and go straight to the /workspaces
    /// probe — mirrors the on-disk state immediately after a PKCE login.
    fn save_test_session(token: &str) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        crate::client::jwt::save_session(&crate::client::jwt::Session {
            access_token: token.to_string(),
            access_expires_at: now + 3600,
            refresh_token: "r".into(),
            refresh_expires_at: now + 86400,
            source: "pkce".into(),
        })
        .unwrap();
    }

    // --- jwt_string_claim / jwt_array_claim tests ---

    #[test]
    fn jwt_string_claim_extracts_source() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"source":"database_api_token","exp":123}"#);
        let token = format!("header.{payload}.sig");
        assert_eq!(
            jwt_string_claim(&token, "source").as_deref(),
            Some("database_api_token")
        );
        // Missing claim, non-string claim, and malformed tokens yield None.
        assert_eq!(jwt_string_claim(&token, "nope"), None);
        assert_eq!(jwt_string_claim(&token, "exp"), None);
        assert_eq!(jwt_string_claim("not-a-jwt", "source"), None);
    }

    #[test]
    fn jwt_array_claim_extracts_workspaces() {
        let payload = URL_SAFE_NO_PAD.encode(br#"{"workspaces":["work_a","work_b"]}"#);
        let token = format!("header.{payload}.sig");
        assert_eq!(
            jwt_array_claim(&token, "workspaces"),
            vec!["work_a", "work_b"]
        );
        // Missing claim / malformed tokens yield an empty list.
        assert!(jwt_array_claim(&token, "nope").is_empty());
        assert!(jwt_array_claim("not-a-jwt", "workspaces").is_empty());
    }

    #[test]
    fn api_key_workspace_ids_decodes_the_tokens_workspace_claim() {
        // A database API token is authorized for exactly one workspace, carried
        // in its minted JWT's `workspaces` claim — that's what scopes requests.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let payload = URL_SAFE_NO_PAD
            .encode(br#"{"workspaces":["workbound"],"source":"database_api_token"}"#);
        let jwt = format!("header.{payload}.sig");
        let mint = server
            .mock("POST", "/o/token/")
            .match_body(mockito::Matcher::UrlEncoded(
                "grant_type".into(),
                "api_token".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(format!(
                r#"{{"access_token":"{jwt}","expires_in":300,"refresh_token":"r"}}"#
            ))
            .create();

        let profile = mock_profile(&server.url(), Some("hd_dbtoken"));
        let ids = api_key_workspace_ids(&profile);
        mint.assert();
        assert_eq!(ids, vec!["workbound".to_string()]);
    }

    // --- check_status tests ---

    #[test]
    fn status_not_configured_when_no_key_no_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://localhost", None);
        assert_eq!(check_status(&profile), AuthStatus::NotConfigured);
    }

    #[test]
    fn status_not_configured_when_placeholder_no_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://localhost", Some("PLACEHOLDER"));
        assert_eq!(check_status(&profile), AuthStatus::NotConfigured);
    }

    #[test]
    fn status_authenticated_with_valid_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("valid-jwt");
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer valid-jwt")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), None);
        assert_eq!(check_status(&profile), AuthStatus::Authenticated);
        mock.assert();
    }

    #[test]
    fn status_authenticated_via_api_token_fallback_when_no_session() {
        // Realistic upgrade path: user has an api_key in config but no
        // session.json yet. ensure_access_token must mint a JWT from
        // the api_key, then check_status probes /workspaces with it.
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
            .with_body(r#"{"access_token":"minted-jwt","expires_in":300,"refresh_token":"r"}"#)
            .create();
        let probe_mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer minted-jwt")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), Some("hd_xyz"));
        assert_eq!(check_status(&profile), AuthStatus::Authenticated);
        mint_mock.assert();
        probe_mock.assert();
    }

    #[test]
    fn status_invalid_when_session_revoked_server_side() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("revoked-jwt");
        let mut server = mockito::Server::new();
        let mock = server.mock("GET", "/workspaces").with_status(401).create();

        let profile = mock_profile(&server.url(), None);
        assert_eq!(check_status(&profile), AuthStatus::Invalid(401));
        mock.assert();
    }

    #[test]
    fn status_invalid_with_forbidden() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("jwt");
        let mut server = mockito::Server::new();
        let mock = server.mock("GET", "/workspaces").with_status(403).create();

        let profile = mock_profile(&server.url(), None);
        assert_eq!(check_status(&profile), AuthStatus::Invalid(403));
        mock.assert();
    }

    #[test]
    fn status_invalid_when_api_token_rejected_no_session() {
        // No session, and the api_key fallback is rejected by the mint
        // endpoint — collapse to Invalid(401) so `auth status` shows
        // the user they need to re-auth.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let mock = server.mock("POST", "/o/token/").with_status(401).create();

        let profile = mock_profile(&server.url(), Some("hd_revoked"));
        assert_eq!(check_status(&profile), AuthStatus::Invalid(401));
        mock.assert();
    }

    #[test]
    fn status_connection_error_during_probe() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("jwt");
        let profile = mock_profile("http://127.0.0.1:1", None);
        match check_status(&profile) {
            AuthStatus::ConnectionError(_) => {}
            other => panic!("expected ConnectionError, got {:?}", other),
        }
    }
}
