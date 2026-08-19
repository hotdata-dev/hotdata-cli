//! Credential inspection: validate the active profile's auth state and, for
//! an api-key credential, discover which workspace(s) it's authorized for.
//!
//! An `hd_...` API key is sent verbatim as the bearer (no client-side JWT
//! exchange — see [`crate::client::jwt`]), so it carries no claims to decode.
//! Workspace scope is instead discovered the same way any caller would: by
//! asking the API.
//!
//! This is the infrastructure half of auth — consumed by the SDK seam and by
//! `main`'s workspace resolution. The interactive login/register/status UI
//! lives in [`crate::commands::auth`], which depends on this module (never the
//! reverse).

use crate::config::{self, ApiKeySource};

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
    let client = crate::client::raw_http::build_http_client();
    let req = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"));
    match crate::util::send_debug(&client, req, None) {
        Ok((status, _)) if status.is_success() => AuthStatus::Authenticated,
        Ok((status, _)) => AuthStatus::Invalid(status.as_u16()),
        Err(e) => AuthStatus::ConnectionError(e.to_string()),
    }
}

/// The workspace a command with no `--workspace-id` targets for this profile —
/// the single source of truth shared by `main`'s `resolve_workspace` and
/// `auth status`, so the status readout can never disagree with where commands
/// actually run.
///
/// An api-key credential scoped to exactly one workspace (a database API token)
/// pins that workspace. For a multi-workspace api key we honor the saved
/// default (`workspaces set` moves a workspace to the front of the config list)
/// when the key can reach it, otherwise fall back to the credential's own first
/// authorized workspace. A CLI session uses the saved default. `None` means no
/// default is known and the caller must pass `--workspace-id`.
///
/// The scope comes from a live `GET /workspaces`, so this is best-effort rather
/// than a guarantee: when that probe fails we fall back to the saved default
/// and let the gateway be the one to reject it. Note the consequence for an
/// unrestricted key with no saved default — previously it had no discoverable
/// scope and the caller was forced to pass `--workspace-id`; now it resolves to
/// whichever workspace the API lists first.
pub(crate) fn default_workspace_id(profile_config: &config::ProfileConfig) -> Option<String> {
    let saved_default = || {
        profile_config
            .workspaces
            .first()
            .map(|w| w.public_id.clone())
    };
    if !matches!(
        profile_config.api_key_source,
        ApiKeySource::Flag | ApiKeySource::Env
    ) {
        return saved_default();
    }
    let ids = api_key_workspace_ids(profile_config);
    if let [only] = ids.as_slice() {
        return Some(only.clone());
    }
    // Multi-workspace key: prefer the saved default when the key authorizes it,
    // else the key's first.
    //
    // Empty `ids` no longer means "unrestricted". `api_key_workspace_ids` now
    // asks the server, so an unrestricted key comes back with the full list;
    // empty means only that we couldn't find out — no key, or the probe failed.
    // Honoring the saved default in that case is a deliberate degradation: it's
    // the best guess available, and the gateway still rejects it if wrong.
    if let Some(first) = saved_default()
        && (ids.is_empty() || ids.contains(&first))
    {
        return Some(first);
    }
    ids.into_iter().next()
}

/// Response shape of `GET /workspaces`.
#[derive(serde::Deserialize)]
struct WsListResponse {
    workspaces: Vec<WsItem>,
}

#[derive(serde::Deserialize)]
struct WsItem {
    public_id: String,
    name: String,
}

/// Fetch the workspaces `bearer` is authorized for via `GET {api_url}/workspaces`.
///
/// Shared by every caller that needs a live workspace list for a bearer
/// credential: this module's own [`check_status`]-adjacent probes,
/// `commands::auth`'s post-login cache and `auth status` display. Callers
/// that treat a failure as "no workspaces" rather than a hard error can
/// `.unwrap_or_default()` the result.
pub(crate) fn fetch_workspaces(
    api_url: &str,
    bearer: &str,
) -> Result<Vec<config::WorkspaceEntry>, String> {
    let url = format!("{api_url}/workspaces");
    let client = crate::client::raw_http::build_http_client();
    let req = client
        .get(&url)
        .header("Authorization", format!("Bearer {bearer}"));
    let (status, body) = crate::util::send_debug(&client, req, None).map_err(|e| format!("{e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let parsed: WsListResponse = serde_json::from_str(&body).map_err(|e| format!("{e}"))?;
    Ok(parsed
        .workspaces
        .into_iter()
        .map(|w| config::WorkspaceEntry {
            public_id: w.public_id,
            name: w.name,
        })
        .collect())
}

/// Workspace public-ids the active api-key credential (`--api-key` /
/// `HOTDATA_API_KEY`) is authorized for, discovered via `GET /workspaces`
/// (the same probe [`check_status`] uses) — the raw token carries no claims
/// to decode client-side. A database API token is scoped to exactly one; an
/// unrestricted token returns every workspace it can reach. Empty when
/// there's no api key or the request fails.
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
    fetch_workspaces(&profile_config.api_url.to_string(), &token)
        .map(|ws| ws.into_iter().map(|w| w.public_id).collect())
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

    #[test]
    fn api_key_workspace_ids_fetches_workspaces_with_the_raw_key_as_bearer() {
        // A database API token is authorized for exactly one workspace,
        // discovered by asking the API directly — the raw key carries no
        // claims to decode client-side.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer hd_dbtoken")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"workspaces":[{"public_id":"workbound","name":"Workbound"}]}"#)
            .create();

        let profile = mock_profile(&server.url(), Some("hd_dbtoken"));
        let ids = api_key_workspace_ids(&profile);
        m.assert();
        assert_eq!(ids, vec!["workbound".to_string()]);
    }

    #[test]
    fn api_key_workspace_ids_empty_when_request_fails() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1", Some("hd_dbtoken"));
        assert!(api_key_workspace_ids(&profile).is_empty());
    }

    // --- default_workspace_id tests ---

    fn ws(id: &str) -> config::WorkspaceEntry {
        config::WorkspaceEntry {
            public_id: id.into(),
            name: id.into(),
        }
    }

    /// `GET /workspaces` mock returning the given public-ids.
    fn mock_workspaces(server: &mut mockito::Server, ids: &[&str]) -> mockito::Mock {
        let body = serde_json::json!({
            "workspaces": ids.iter().map(|id| serde_json::json!({"public_id": id, "name": id})).collect::<Vec<_>>(),
        });
        server
            .mock("GET", "/workspaces")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body.to_string())
            .create()
    }

    #[test]
    fn default_workspace_id_session_uses_saved_default_without_network() {
        // Config source (a CLI session): the saved default, no network call.
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = ProfileConfig {
            workspaces: vec![ws("work_saved"), ws("work_other")],
            ..Default::default() // api_key_source defaults to Config
        };
        assert_eq!(
            default_workspace_id(&profile),
            Some("work_saved".to_string())
        );
    }

    #[test]
    fn default_workspace_id_single_workspace_token_pins_its_own() {
        // A database token authorizes exactly one workspace — use it even when a
        // different workspace sits at the front of the (unrelated) config cache.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let probe = mock_workspaces(&mut server, &["work_only"]);
        let mut profile = mock_profile(&server.url(), Some("hd_dbtoken"));
        profile.api_key_source = ApiKeySource::Env;
        profile.workspaces = vec![ws("work_saved")];
        assert_eq!(
            default_workspace_id(&profile),
            Some("work_only".to_string())
        );
        probe.assert();
    }

    #[test]
    fn default_workspace_id_multi_key_honors_saved_default_when_authorized() {
        // Multi-workspace key + a saved default the key can reach → the saved
        // default wins (so `workspaces set` keeps working).
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let _probe = mock_workspaces(&mut server, &["work_a", "work_saved", "work_b"]);
        let mut profile = mock_profile(&server.url(), Some("hd_org"));
        profile.api_key_source = ApiKeySource::Env;
        profile.workspaces = vec![ws("work_saved")];
        assert_eq!(
            default_workspace_id(&profile),
            Some("work_saved".to_string())
        );
    }

    #[test]
    fn default_workspace_id_multi_key_falls_back_to_first_authorized() {
        // Saved default is NOT one the key authorizes → the credential's first
        // authorized workspace, never a workspace the gateway would 403.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let _probe = mock_workspaces(&mut server, &["work_a", "work_b"]);
        let mut profile = mock_profile(&server.url(), Some("hd_org"));
        profile.api_key_source = ApiKeySource::Env;
        profile.workspaces = vec![ws("work_unauthorized")];
        assert_eq!(default_workspace_id(&profile), Some("work_a".to_string()));
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
        // session.json yet. check_status sends the raw key verbatim as the
        // bearer on the /workspaces probe — no mint step.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let probe_mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer hd_xyz")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), Some("hd_xyz"));
        assert_eq!(check_status(&profile), AuthStatus::Authenticated);
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
        // No session, and the raw api_key is rejected by the server on the
        // actual /workspaces probe — there's no client-side mint step left
        // to reject it earlier.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer hd_revoked")
            .with_status(401)
            .create();

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
