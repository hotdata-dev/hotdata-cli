use crate::config::{self, ApiKeySource};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use crossterm::ExecutableCommand;
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::stdout;

pub fn logout(profile: &str) {
    crate::jwt::clear_session();
    if let Err(e) = config::clear_workspaces(profile) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
    println!("{}", "Logged out.".green());
}

#[derive(Debug, PartialEq)]
pub enum AuthStatus {
    Authenticated,
    NotConfigured,
    Invalid(u16),
    ConnectionError(String),
}

pub fn check_status(profile_config: &config::ProfileConfig) -> AuthStatus {
    let api_key_fallback = profile_config
        .api_key
        .as_deref()
        .filter(|k| !k.is_empty() && *k != "PLACEHOLDER");

    // PKCE-origin sessions don't write an api_key, so absence of a key
    // alone isn't "not configured" — only true if there's also no
    // cached JWT session to validate.
    if api_key_fallback.is_none() && crate::jwt::load_session().is_none() {
        return AuthStatus::NotConfigured;
    }

    let access_token = match crate::jwt::ensure_access_token(profile_config, api_key_fallback) {
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

pub fn status(profile: &str) {
    let profile_config = match config::load(profile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // The credential the CLI is *about to use*. Note: even when an
    // override is set, the wire credential is still a JWT (minted on
    // demand from the override) — but we report the user-visible source.
    let method_label = match profile_config.api_key_source {
        ApiKeySource::Flag => "API Key flag",
        ApiKeySource::Env => "API Key env",
        ApiKeySource::Config => "CLI Session",
    };

    // For Flag/Env we mask the api_key the user supplied. For the
    // CLI session path we mask the refresh_token — it's stable across
    // commands (unlike the 5-min access_token), so the tail stays
    // recognizable between runs.
    let credential_tail = match profile_config.api_key_source {
        ApiKeySource::Flag | ApiKeySource::Env => profile_config
            .api_key
            .as_deref()
            .map(crate::util::mask_credential),
        ApiKeySource::Config => crate::jwt::load_session()
            .map(|s| crate::util::mask_credential(&s.refresh_token)),
    };
    let method_suffix = match credential_tail {
        Some(tail) => format!(" - {method_label} [{tail}]"),
        None => format!(" - {method_label}"),
    };

    match check_status(&profile_config) {
        AuthStatus::NotConfigured => {
            print_row("Authenticated", &"No".red().to_string());
        }
        AuthStatus::Authenticated => {
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row(
                "Authenticated",
                &format!("{}{}", "Yes".green(), method_suffix.dark_grey()),
            );
            match profile_config.workspaces.first() {
                Some(w) => {
                    print_row("Workspace", &format!("{} {}", w.name.as_str().cyan(), format!("({})", w.public_id).dark_grey()));
                    print_row("", &"use 'hotdata workspaces set' to switch workspaces".dark_grey().to_string());
                }
                None => print_row("Current Workspace", &"None".dark_grey().to_string()),
            }
        }
        AuthStatus::Invalid(_) => {
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row(
                "Authenticated",
                &format!("{}{}", "No".red(), method_suffix.dark_grey()),
            );
        }
        AuthStatus::ConnectionError(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Deserialize)]
struct WsListResponse { workspaces: Vec<WsItem> }

#[derive(Deserialize)]
struct WsItem { public_id: String, name: String }

/// Wait for the browser callback, verify state, and extract the authorization code.
fn receive_callback(server: &tiny_http::Server, expected_state: &str) -> Result<String, String> {
    let request = server.recv().map_err(|e| format!("failed to receive callback: {e}"))?;
    let raw_url = request.url().to_string();
    let params = parse_query_params(&raw_url);

    if params.get("state").map(String::as_str) != Some(expected_state) {
        let _ = request.respond(tiny_http::Response::from_string("Login failed: state mismatch"));
        return Err("state mismatch — possible CSRF attack".into());
    }

    let code = match params.get("code") {
        Some(c) => c.clone(),
        None => {
            let _ = request.respond(tiny_http::Response::from_string("Login failed: no code"));
            return Err("no authorization code received in callback".into());
        }
    };

    let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>Hotdata — Login Successful</title>
  <style>
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    body {
      font-family: ui-sans-serif, system-ui, -apple-system, sans-serif;
      background: #111827;
      color: #e5e7eb;
      display: flex;
      align-items: center;
      justify-content: center;
      min-height: 100vh;
    }
    .card {
      background: #1f2937;
      border: 1px solid #374151;
      border-radius: 0.5rem;
      padding: 2.5rem;
      max-width: 420px;
      width: 100%;
      text-align: center;
    }
    .icon {
      width: 48px;
      height: 48px;
      background: #14532d;
      border-radius: 50%;
      display: flex;
      align-items: center;
      justify-content: center;
      margin: 0 auto 1.25rem;
    }
    .icon svg { width: 24px; height: 24px; stroke: #86efac; }
    h1 { font-size: 1.25rem; font-weight: 600; color: #f3f4f6; margin-bottom: 0.5rem; }
    p { font-size: 0.875rem; color: #9ca3af; line-height: 1.5; }
  </style>
</head>
<body>
  <div class="card">
    <div class="icon">
      <svg fill="none" viewBox="0 0 24 24" stroke-width="2.5" stroke="currentColor">
        <path stroke-linecap="round" stroke-linejoin="round" d="M4.5 12.75l6 6 9-13.5" />
      </svg>
    </div>
    <h1>Login successful</h1>
    <p>You're now authenticated with Hotdata.<br/>You can close this tab and return to the terminal.</p>
  </div>
</body>
</html>"#;
    let response = tiny_http::Response::from_string(html).with_header(
        "Content-Type: text/html"
            .parse::<tiny_http::Header>()
            .unwrap(),
    );
    let _ = request.respond(response);

    Ok(code)
}

fn is_already_signed_in(profile_config: &config::ProfileConfig) -> bool {
    check_status(profile_config) == AuthStatus::Authenticated
}

pub fn login() {
    let profile_config = config::load("default").unwrap_or_default();
    let app_url = profile_config.app_url.to_string();

    // Check if already authenticated
    if is_already_signed_in(&profile_config) {
        println!("{}", "You are already signed in.".green());
        print!("Do you want to log in again? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush().unwrap();
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        if !input.trim().eq_ignore_ascii_case("y") {
            return;
        }
    }

    let code_verifier = generate_code_verifier();
    let code_challenge = generate_code_challenge(&code_verifier);
    let state = generate_random_string(32);

    // Bind to port 0 so the OS picks an available port. DOT's consent
    // page will redirect here with `?code=...&state=...`.
    let server =
        tiny_http::Server::http("127.0.0.1:0").expect("failed to start local callback server");
    let port = server.server_addr().to_ip().unwrap().port();
    let redirect_uri = format!("http://127.0.0.1:{port}/");

    // DOT's `/o/authorize/` endpoint is mounted off the app URL (the
    // browser-facing one; allauth session cookies live here). We send
    // no `scope` parameter — the consent page picks permissions and
    // workspace scope interactively, then composes the scope string
    // server-side (see HotdataAllowForm).
    let login_url = format!(
        "{app_url}/o/authorize/\
        ?client_id=hotdata-cli\
        &response_type=code\
        &redirect_uri={redirect_uri}\
        &code_challenge={code_challenge}\
        &code_challenge_method=S256\
        &state={state}",
        app_url = app_url.trim_end_matches('/'),
    );

    println!("Opening browser to log in...");
    stdout()
        .execute(Print("If your browser does not open, visit:\n  "))
        .unwrap()
        .execute(SetForegroundColor(Color::DarkGrey))
        .unwrap()
        .execute(Print(format!("{login_url}\n")))
        .unwrap()
        .execute(ResetColor)
        .unwrap();

    if let Err(e) = open::that(&login_url) {
        eprintln!("failed to open browser: {e}");
    }

    println!("Waiting for login callback...");

    let code = match receive_callback(&server, &state) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    match crate::jwt::mint_from_pkce_code(&profile_config, &code, &code_verifier, &redirect_uri) {
        Ok(session) => {
            if let Err(e) = crate::jwt::save_session(&session) {
                eprintln!("warning: could not save session: {e}");
            }
            stdout()
                .execute(SetForegroundColor(Color::Green))
                .unwrap()
                .execute(Print("Logged in successfully.\n"))
                .unwrap()
                .execute(ResetColor)
                .unwrap();

            // Best-effort workspace cache using the freshly minted JWT.
            // Fall back to the existing on-disk list if the fetch fails.
            let workspaces = cache_workspaces(&profile_config, &session.access_token)
                .unwrap_or(profile_config.workspaces);
            match workspaces.first() {
                Some(w) => {
                    print_row("Workspace", &format!("{} {}", w.name.as_str().cyan(), format!("({})", w.public_id).dark_grey()));
                    print_row("", &"use 'hotdata workspaces set' to switch workspaces".dark_grey().to_string());
                }
                None => print_row("Workspace", &"None".dark_grey().to_string()),
            }
        }
        Err(msg) => {
            eprintln!("{}", msg.red());
            std::process::exit(1);
        }
    }
}

/// Fetch workspaces with a freshly minted JWT and cache them in config.
/// Returns the freshly fetched list so callers can display it without
/// having to reload config from disk.
fn cache_workspaces(
    profile: &config::ProfileConfig,
    access_token: &str,
) -> Result<Vec<config::WorkspaceEntry>, String> {
    let url = format!("{}/workspaces", profile.api_url);
    let client = reqwest::blocking::Client::new();
    let req = client
        .get(&url)
        .header("Authorization", format!("Bearer {access_token}"));
    let (status, body) = crate::util::send_debug(&client, req, None).map_err(|e| format!("{e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}"));
    }
    let ws: WsListResponse = serde_json::from_str(&body).map_err(|e| format!("{e}"))?;
    let entries: Vec<config::WorkspaceEntry> = ws
        .workspaces
        .into_iter()
        .map(|w| config::WorkspaceEntry {
            public_id: w.public_id,
            name: w.name,
        })
        .collect();
    config::save_workspaces("default", entries.clone())?;
    Ok(entries)
}

fn generate_code_verifier() -> String {
    generate_random_string(64)
}

fn generate_random_string(len: usize) -> String {
    let charset = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| charset[rng.gen_range(0..charset.len())] as char)
        .collect()
}

fn generate_code_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn parse_query_params(url: &str) -> HashMap<String, String> {
    url.splitn(2, '?')
        .nth(1)
        .unwrap_or("")
        .split('&')
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
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
        crate::jwt::save_session(&crate::jwt::Session {
            access_token: token.to_string(),
            access_expires_at: now + 3600,
            refresh_token: "r".into(),
            refresh_expires_at: now + 86400,
            source: "pkce".into(),
        })
        .unwrap();
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
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(401)
            .create();

        let profile = mock_profile(&server.url(), None);
        assert_eq!(check_status(&profile), AuthStatus::Invalid(401));
        mock.assert();
    }

    #[test]
    fn status_invalid_with_forbidden() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("jwt");
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(403)
            .create();

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
        let mock = server
            .mock("POST", "/o/token/")
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

    // --- is_already_signed_in tests ---

    #[test]
    fn already_signed_in_when_session_valid() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("session-jwt");
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer session-jwt")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), None);
        assert!(is_already_signed_in(&profile));
        mock.assert();
    }

    #[test]
    fn not_signed_in_when_no_key_no_session() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://localhost", None);
        assert!(!is_already_signed_in(&profile));
    }

    #[test]
    fn not_signed_in_when_session_invalid() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_test_session("expired-jwt");
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(401)
            .create();

        let profile = mock_profile(&server.url(), None);
        assert!(!is_already_signed_in(&profile));
        mock.assert();
    }

    // --- receive_callback tests ---

    #[test]
    fn receive_callback_success() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();

        // Simulate browser redirect in a background thread
        let handle = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            client
                .get(format!(
                    "http://127.0.0.1:{port}/callback?code=test-auth-code&state=expected-state"
                ))
                .send()
                .unwrap();
        });

        let result = receive_callback(&server, "expected-state");
        handle.join().unwrap();

        assert_eq!(result.unwrap(), "test-auth-code");
    }

    #[test]
    fn receive_callback_state_mismatch() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();

        let handle = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let _ = client
                .get(format!(
                    "http://127.0.0.1:{port}/callback?code=code&state=wrong-state"
                ))
                .send();
        });

        let result = receive_callback(&server, "expected-state");
        handle.join().unwrap();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("state mismatch"));
    }

    // --- cache_workspaces tests ---

    #[test]
    fn cache_workspaces_persists_to_config() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer jwt-xyz")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"workspaces":[{"public_id":"ws-1","name":"My WS"},{"public_id":"ws-2","name":"Other"}]}"#,
            )
            .create();

        let profile = mock_profile(&server.url(), None);
        let entries = cache_workspaces(&profile, "jwt-xyz").unwrap();
        m.assert();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].public_id, "ws-1");
        assert_eq!(entries[0].name, "My WS");

        // Reload from disk and confirm the cache survived.
        let loaded = config::load("default").unwrap();
        assert_eq!(loaded.workspaces.len(), 2);
        assert_eq!(loaded.workspaces[1].public_id, "ws-2");
    }

    #[test]
    fn cache_workspaces_empty_list() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/workspaces")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), None);
        let entries = cache_workspaces(&profile, "jwt").unwrap();
        m.assert();
        assert!(entries.is_empty());
    }

    #[test]
    fn cache_workspaces_http_error() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("GET", "/workspaces").with_status(500).create();
        let profile = mock_profile(&server.url(), None);
        let err = cache_workspaces(&profile, "jwt").unwrap_err();
        m.assert();
        assert!(err.contains("500"), "got: {err}");
    }

    #[test]
    fn cache_workspaces_connection_error() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1", None);
        assert!(cache_workspaces(&profile, "jwt").is_err());
    }

    #[test]
    fn receive_callback_no_code() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();

        let handle = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::new();
            let _ = client
                .get(format!(
                    "http://127.0.0.1:{port}/callback?state=expected-state"
                ))
                .send();
        });

        let result = receive_callback(&server, "expected-state");
        handle.join().unwrap();

        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no authorization code"));
    }
}

fn print_row(label: &str, value: &str) {
    stdout()
        .execute(SetForegroundColor(Color::DarkGrey))
        .unwrap()
        .execute(Print(format!("{:<16}", if label.is_empty() { String::new() } else { format!("{label}:") })))
        .unwrap()
        .execute(ResetColor)
        .unwrap()
        .execute(Print(format!("{value}\n")))
        .unwrap();
}
