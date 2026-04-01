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

    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => {
            print_row("Authenticated", &"No".red().to_string());
            print_row("API Key", &"Not configured".red().to_string());
            return;
        }
    };

    let url = format!("{}/workspaces", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
    {
        Ok(resp) if resp.status().is_success() => {
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
        Ok(resp) => {
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row("Authenticated", &"No".red().to_string());
            print_row(
                "API Key",
                &format!(
                    "{}{source_label}",
                    format!("Invalid (HTTP {})", resp.status()).red()
                ),
            );
        }
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    }
}

pub fn login() {
    let profile_config = config::load("default").unwrap_or_default();
    let api_url = profile_config.api_url.to_string();
    let app_url = profile_config.app_url.to_string();

    // Check if already authenticated
    if let Some(api_key) = &profile_config.api_key {
        if api_key != "PLACEHOLDER" {
            let client = reqwest::blocking::Client::new();
            if let Ok(resp) = client
                .get(format!("{api_url}/workspaces"))
                .header("Authorization", format!("Bearer {api_key}"))
                .send()
            {
                if resp.status().is_success() {
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
            }
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

    let request = server.recv().expect("failed to receive callback request");
    let raw_url = request.url().to_string();
    let params = parse_query_params(&raw_url);

    // Verify state to prevent CSRF
    if params.get("state").map(String::as_str) != Some(state.as_str()) {
        let _ = request.respond(tiny_http::Response::from_string(
            "Login failed: state mismatch",
        ));
        eprintln!("error: state mismatch — possible CSRF attack");
        std::process::exit(1);
    }

    let code = match params.get("code") {
        Some(c) => c.clone(),
        None => {
            let _ = request.respond(tiny_http::Response::from_string("Login failed: no code"));
            eprintln!("error: no authorization code received in callback");
            std::process::exit(1);
        }
    };

    // Respond to the browser before making the token exchange request
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

    // Exchange the authorization code + verifier for the real API token
    #[derive(Deserialize)]
    struct TokenResponse {
        token: String,
    }

    let token_url = format!("{api_url}/auth/token");
    let client = reqwest::blocking::Client::new();

    let resp: Result<reqwest::blocking::Response, _> = client
        .post(&token_url)
        .json(&serde_json::json!({ "code": code, "code_verifier": code_verifier }))
        .send();

    match resp {
        Ok(r) if r.status().is_success() => {
            let body: TokenResponse = match r.json() {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("error parsing token response: {e}");
                    std::process::exit(1);
                }
            };

            if let Err(e) = config::save_api_key("default", &body.token) {
                eprintln!("error saving token: {e}");
                std::process::exit(1);
            }

            // Fetch and cache workspace IDs for use as default
            #[derive(Deserialize)]
            struct WsListResponse { workspaces: Vec<WsItem> }
            #[derive(Deserialize)]
            struct WsItem { public_id: String, name: String }

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

            stdout()
                .execute(SetForegroundColor(Color::Green))
                .unwrap()
                .execute(Print("Logged in successfully.\n"))
                .unwrap()
                .execute(ResetColor)
                .unwrap();

            match default_workspace {
                Some(w) => {
                    print_row("Workspace", &format!("{} {}", w.name.as_str().cyan(), format!("({})", w.public_id).dark_grey()));
                    print_row("", &"use 'hotdata workspaces set' to switch workspaces".dark_grey().to_string());
                }
                None => print_row("Workspace", &"None".dark_grey().to_string()),
            }
        }
        Ok(r) if r.status() == reqwest::StatusCode::FORBIDDEN => {
            eprintln!("{}", "You are not authorized to create a new API token.".red());
            std::process::exit(1);
        }
        Ok(r) => {
            eprintln!("token exchange failed: HTTP {}", r.status());
            std::process::exit(1);
        }
        Err(e) => {
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
