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
    if let Err(e) = config::remove_api_key(profile) {
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
    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => return AuthStatus::NotConfigured,
    };

    let url = format!("{}/workspaces", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
    {
        Ok(resp) if resp.status().is_success() => AuthStatus::Authenticated,
        Ok(resp) => AuthStatus::Invalid(resp.status().as_u16()),
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

    let source_label = if profile_config.api_key_source == ApiKeySource::Env {
        " (env override)"
    } else {
        ""
    };

    match check_status(&profile_config) {
        AuthStatus::NotConfigured => {
            print_row("Authenticated", &"No".red().to_string());
            print_row("API Key", &"Not configured".red().to_string());
        }
        AuthStatus::Authenticated => {
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row("Authenticated", &"Yes".green().to_string());
            print_row("API Key", &format!("{}{source_label}", "Valid".green()));
            match profile_config.workspaces.first() {
                Some(w) => {
                    print_row("Workspace", &format!("{} {}", w.name.as_str().cyan(), format!("({})", w.public_id).dark_grey()));
                    print_row("", &"use 'hotdata workspaces set' to switch workspaces".dark_grey().to_string());
                }
                None => print_row("Current Workspace", &"None".dark_grey().to_string()),
            }
        }
        AuthStatus::Invalid(code) => {
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row("Authenticated", &"No".red().to_string());
            print_row(
                "API Key",
                &format!(
                    "{}{source_label}",
                    format!("Invalid (HTTP {})", code).red()
                ),
            );
        }
        AuthStatus::ConnectionError(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum LoginResult {
    Success { token: String, workspace: Option<config::WorkspaceEntry> },
    Forbidden,
    Failed(String),
    ConnectionError(String),
}

#[derive(Deserialize)]
struct TokenResponse {
    token: String,
}

#[derive(Deserialize)]
struct WsListResponse { workspaces: Vec<WsItem> }

#[derive(Deserialize)]
struct WsItem { public_id: String, name: String }

/// Exchange an authorization code + PKCE verifier for an API token,
/// then fetch available workspaces.
fn exchange_token(api_url: &str, code: &str, code_verifier: &str) -> LoginResult {
    let token_url = format!("{api_url}/auth/token");
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .post(&token_url)
        .json(&serde_json::json!({ "code": code, "code_verifier": code_verifier }))
        .send()
    {
        Ok(r) => r,
        Err(e) => return LoginResult::ConnectionError(e.to_string()),
    };

    if resp.status() == reqwest::StatusCode::FORBIDDEN {
        return LoginResult::Forbidden;
    }

    if !resp.status().is_success() {
        return LoginResult::Failed(format!("HTTP {}", resp.status()));
    }

    let body: TokenResponse = match resp.json() {
        Ok(b) => b,
        Err(e) => return LoginResult::Failed(format!("error parsing token response: {e}")),
    };

    // Save the token
    if let Err(e) = config::save_api_key("default", &body.token) {
        return LoginResult::Failed(format!("error saving token: {e}"));
    }

    // Fetch and cache workspaces
    let ws_url = format!("{api_url}/workspaces");
    let default_workspace = if let Ok(r) = client.get(&ws_url).header("Authorization", format!("Bearer {}", body.token)).send() {
        if r.status().is_success() {
            if let Ok(ws) = r.json::<WsListResponse>() {
                let entries: Vec<config::WorkspaceEntry> = ws.workspaces.into_iter()
                    .map(|w| config::WorkspaceEntry { public_id: w.public_id, name: w.name })
                    .collect();
                let first = entries.first().cloned();
                let _ = config::save_workspaces("default", entries);
                first
            } else { None }
        } else { None }
    } else { None };

    LoginResult::Success { token: body.token, workspace: default_workspace }
}

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
    let api_url = profile_config.api_url.to_string();
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

    // Bind to port 0 so the OS picks an available port
    let server =
        tiny_http::Server::http("127.0.0.1:0").expect("failed to start local callback server");
    let port = server.server_addr().to_ip().unwrap().port();

    let login_url = format!(
        "{app_url}/auth/cli-login?code_challenge={code_challenge}&code_challenge_method=S256&state={state}&callback_port={port}"
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

    match exchange_token(&api_url, &code, &code_verifier) {
        LoginResult::Success { workspace, .. } => {
            stdout()
                .execute(SetForegroundColor(Color::Green))
                .unwrap()
                .execute(Print("Logged in successfully.\n"))
                .unwrap()
                .execute(ResetColor)
                .unwrap();

            match workspace {
                Some(w) => {
                    print_row("Workspace", &format!("{} {}", w.name.as_str().cyan(), format!("({})", w.public_id).dark_grey()));
                    print_row("", &"use 'hotdata workspaces set' to switch workspaces".dark_grey().to_string());
                }
                None => print_row("Workspace", &"None".dark_grey().to_string()),
            }
        }
        LoginResult::Forbidden => {
            eprintln!("{}", "You are not authorized to create a new API token.".red());
            std::process::exit(1);
        }
        LoginResult::Failed(msg) => {
            eprintln!("token exchange failed: {msg}");
            std::process::exit(1);
        }
        LoginResult::ConnectionError(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    }
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
    use config::{ApiUrl, ProfileConfig, test_helpers::with_temp_config_dir};

    fn mock_profile(api_url: &str, api_key: Option<&str>) -> ProfileConfig {
        ProfileConfig {
            api_key: api_key.map(String::from),
            api_url: ApiUrl(Some(api_url.to_string())),
            ..Default::default()
        }
    }

    // --- check_status tests ---

    #[test]
    fn status_not_configured_when_no_key() {
        let profile = mock_profile("http://localhost", None);
        assert_eq!(check_status(&profile), AuthStatus::NotConfigured);
    }

    #[test]
    fn status_not_configured_when_placeholder() {
        let profile = mock_profile("http://localhost", Some("PLACEHOLDER"));
        assert_eq!(check_status(&profile), AuthStatus::NotConfigured);
    }

    #[test]
    fn status_authenticated_with_valid_key() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer valid-key")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), Some("valid-key"));
        assert_eq!(check_status(&profile), AuthStatus::Authenticated);
        mock.assert();
    }

    #[test]
    fn status_invalid_with_bad_key() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(401)
            .create();

        let profile = mock_profile(&server.url(), Some("bad-key"));
        assert_eq!(check_status(&profile), AuthStatus::Invalid(401));
        mock.assert();
    }

    #[test]
    fn status_invalid_with_forbidden() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(403)
            .create();

        let profile = mock_profile(&server.url(), Some("forbidden-key"));
        assert_eq!(check_status(&profile), AuthStatus::Invalid(403));
        mock.assert();
    }

    #[test]
    fn status_connection_error() {
        let profile = mock_profile("http://127.0.0.1:1", Some("key"));
        match check_status(&profile) {
            AuthStatus::ConnectionError(_) => {}
            other => panic!("expected ConnectionError, got {:?}", other),
        }
    }

    // --- is_already_signed_in tests ---

    #[test]
    fn already_signed_in_when_key_valid() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer existing-key")
            .with_status(200)
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let profile = mock_profile(&server.url(), Some("existing-key"));
        assert!(is_already_signed_in(&profile));
        mock.assert();
    }

    #[test]
    fn not_signed_in_when_no_key() {
        let profile = mock_profile("http://localhost", None);
        assert!(!is_already_signed_in(&profile));
    }

    #[test]
    fn not_signed_in_when_key_invalid() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/workspaces")
            .with_status(401)
            .create();

        let profile = mock_profile(&server.url(), Some("expired-key"));
        assert!(!is_already_signed_in(&profile));
        mock.assert();
    }

    // --- exchange_token tests ---

    #[test]
    fn exchange_token_success() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();

        let token_mock = server
            .mock("POST", "/auth/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"token":"new-api-token-xyz"}"#)
            .create();

        let ws_mock = server
            .mock("GET", "/workspaces")
            .match_header("Authorization", "Bearer new-api-token-xyz")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"workspaces":[{"public_id":"ws-123","name":"My Workspace"}]}"#)
            .create();

        let result = exchange_token(&server.url(), "auth-code", "verifier");

        token_mock.assert();
        ws_mock.assert();

        match result {
            LoginResult::Success { token, workspace } => {
                assert_eq!(token, "new-api-token-xyz");
                let ws = workspace.expect("should have a workspace");
                assert_eq!(ws.public_id, "ws-123");
                assert_eq!(ws.name, "My Workspace");
            }
            other => panic!("expected Success, got {:?}", other),
        }

        // Verify token was saved to config
        let profile = config::load("default").unwrap();
        assert_eq!(profile.api_key, Some("new-api-token-xyz".to_string()));
    }

    #[test]
    fn exchange_token_success_no_workspaces() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();

        let token_mock = server
            .mock("POST", "/auth/token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"token":"token-no-ws"}"#)
            .create();

        let ws_mock = server
            .mock("GET", "/workspaces")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"workspaces":[]}"#)
            .create();

        let result = exchange_token(&server.url(), "code", "verifier");

        token_mock.assert();
        ws_mock.assert();

        match result {
            LoginResult::Success { token, workspace } => {
                assert_eq!(token, "token-no-ws");
                assert!(workspace.is_none());
            }
            other => panic!("expected Success, got {:?}", other),
        }
    }

    #[test]
    fn exchange_token_forbidden() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();

        let mock = server
            .mock("POST", "/auth/token")
            .with_status(403)
            .create();

        let result = exchange_token(&server.url(), "code", "verifier");
        mock.assert();
        assert_eq!(result, LoginResult::Forbidden);
    }

    #[test]
    fn exchange_token_unauthorized() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();

        let mock = server
            .mock("POST", "/auth/token")
            .with_status(401)
            .create();

        let result = exchange_token(&server.url(), "code", "verifier");
        mock.assert();
        match result {
            LoginResult::Failed(msg) => assert!(msg.contains("401")),
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn exchange_token_server_error() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();

        let mock = server
            .mock("POST", "/auth/token")
            .with_status(500)
            .create();

        let result = exchange_token(&server.url(), "code", "verifier");
        mock.assert();
        match result {
            LoginResult::Failed(msg) => assert!(msg.contains("500")),
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[test]
    fn exchange_token_connection_error() {
        let (_tmp, _guard) = with_temp_config_dir();

        let result = exchange_token("http://127.0.0.1:1", "code", "verifier");
        match result {
            LoginResult::ConnectionError(_) => {}
            other => panic!("expected ConnectionError, got {:?}", other),
        }
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
