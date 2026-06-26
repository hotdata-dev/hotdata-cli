use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Create a steady-ticking spinner with a cyan glyph and the given message.
/// Writes to stderr so stdout (json/yaml output) stays clean.
pub fn spinner(msg: &str) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap());
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

static NO_INPUT: AtomicBool = AtomicBool::new(false);

pub fn set_no_input(enabled: bool) {
    NO_INPUT.store(enabled, Ordering::Relaxed);
}

/// Returns true if interactive prompts are usable. Returns false when:
/// - the global `--no-input` flag was passed,
/// - the `CI` env var is set (most CI runners set this),
/// - stdin is not a TTY (piped, redirected, or invoked by an agent harness).
pub fn is_interactive() -> bool {
    if NO_INPUT.load(Ordering::Relaxed) {
        return false;
    }
    if std::env::var_os("CI").is_some() {
        return false;
    }
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

pub fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Log request details when debug mode is enabled.
pub fn debug_request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&serde_json::Value>,
) {
    if !is_debug() {
        return;
    }
    use crossterm::style::Stylize;
    eprintln!("{}", format!(">>> {method} {url}").dark_cyan());
    for (k, v) in headers {
        eprintln!("{}", format!("  {k}: {v}").dark_grey());
    }
    if let Some(b) = body {
        eprintln!(
            "{}",
            colorize_json(&serde_json::to_string_pretty(b).unwrap())
        );
    }
}

/// Log response status and body when debug mode is enabled. Consumes
/// the response and returns the status + body text for the caller to
/// parse. `redact_keys` masks the named JSON fields in the printed
/// body (last 4 chars only) — pass `&[]` for no redaction. The
/// returned body string is *unredacted* so the caller can still parse
/// real values out of it.
pub fn debug_response_redacted(
    resp: reqwest::blocking::Response,
    redact_keys: &[&str],
) -> (reqwest::StatusCode, String) {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    if is_debug() {
        use crossterm::style::Stylize;
        let status_str = format!(
            "<<< {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        if status.is_success() {
            eprintln!("{}", status_str.dark_green());
        } else {
            eprintln!("{}", status_str.dark_red());
        }
        if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&body) {
            if !redact_keys.is_empty() {
                redact_json_fields(&mut v, redact_keys);
            }
            eprintln!(
                "{}",
                colorize_json(&serde_json::to_string_pretty(&v).unwrap())
            );
        } else if !body.is_empty() {
            eprintln!("{}", body.to_string().dark_grey());
        }
    }

    (status, body)
}

/// Mask a credential to its first + last 4 characters
/// (`XXXX...YYYY`), or `***` if it's too short to reveal anything
/// safely. The tail makes it easy to distinguish which token is on
/// the wire (e.g. user JWT vs database-scoped JWT vs opaque API token).
pub fn mask_credential(s: &str) -> String {
    if s.len() >= 12 {
        format!("{}...{}", &s[..4], &s[s.len() - 4..])
    } else if s.len() > 4 {
        // Short-ish — still better to show head than nothing, but
        // don't double up on bytes by showing a tail.
        format!("{}...", &s[..4])
    } else {
        "***".into()
    }
}

/// Canonical wrapper for every outgoing HTTP call in the CLI. Builds
/// the request, logs it under `--debug` (with `Authorization` auto-
/// masked), executes, and prints + returns the response. Callers stay
/// minimal:
///
/// ```ignore
/// let req = client.get(&url).header("Authorization", bearer);
/// let (status, body) = util::send_debug(&client, req, None)?;
/// ```
///
/// `body_for_log` is the *printable* form of the request body — pass
/// `None` for GET, the JSON `Value` for `.json(...)` calls, or a hand-
/// rolled redacted `Value` for form bodies.
pub fn send_debug(
    client: &reqwest::blocking::Client,
    builder: reqwest::blocking::RequestBuilder,
    body_for_log: Option<&serde_json::Value>,
) -> reqwest::Result<(reqwest::StatusCode, String)> {
    send_debug_with_redaction(client, builder, body_for_log, &[])
}

/// Like `send_debug` but masks the named JSON keys in the printed
/// response body. The returned body string is always unredacted.
pub fn send_debug_with_redaction(
    client: &reqwest::blocking::Client,
    builder: reqwest::blocking::RequestBuilder,
    body_for_log: Option<&serde_json::Value>,
    response_redact_keys: &[&str],
) -> reqwest::Result<(reqwest::StatusCode, String)> {
    let request = builder.build()?;
    if is_debug() {
        log_request_struct(&request, body_for_log);
    }
    let resp = client.execute(request)?;
    Ok(debug_response_redacted(resp, response_redact_keys))
}

fn log_request_struct(req: &reqwest::blocking::Request, body: Option<&serde_json::Value>) {
    let method = req.method().as_str();
    let url = req.url().as_str();
    // Materialize masked header pairs as owned strings, then re-borrow
    // for `debug_request` (which takes &[(&str, &str)]).
    let pairs: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            v.to_str().ok().map(|s| {
                let key = k.as_str();
                let val = if key.eq_ignore_ascii_case("authorization") {
                    mask_auth_value(s)
                } else {
                    s.to_string()
                };
                (key.to_string(), val)
            })
        })
        .collect();
    let refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    debug_request(method, url, &refs, body);
}

/// Mask an `Authorization` header value. Preserves the scheme prefix
/// (`Bearer`, `Basic`, …) so the log still makes sense.
fn mask_auth_value(value: &str) -> String {
    if let Some(token) = value.strip_prefix("Bearer ") {
        format!("Bearer {}", mask_credential(token))
    } else {
        mask_credential(value)
    }
}

/// Walk a JSON value and replace string values under any of the named
/// keys with their masked form. Recurses into nested objects/arrays so
/// callers don't have to know the shape of the response.
fn redact_json_fields(v: &mut serde_json::Value, keys: &[&str]) {
    match v {
        serde_json::Value::Object(map) => {
            for (k, val) in map.iter_mut() {
                if keys.contains(&k.as_str()) {
                    if let Some(s) = val.as_str() {
                        *val = serde_json::Value::String(mask_credential(s));
                    }
                } else {
                    redact_json_fields(val, keys);
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr.iter_mut() {
                redact_json_fields(item, keys);
            }
        }
        _ => {}
    }
}

/// Colorize a pretty-printed JSON string for terminal output.
fn colorize_json(json: &str) -> String {
    use crossterm::style::Stylize;
    let mut result = String::with_capacity(json.len() * 2);

    for line in json.lines() {
        let trimmed = line.trim_start();

        if trimmed.starts_with('"') {
            // Key-value line or string in array
            if let Some(colon_pos) = find_key_colon(trimmed) {
                // Key: value line
                let indent = &line[..line.len() - trimmed.len()];
                let key = &trimmed[..colon_pos];
                let sep = ": ";
                let value = trimmed[colon_pos + 2..].trim();
                result.push_str(indent);
                result.push_str(&key.dark_cyan().to_string());
                result.push_str(&sep.dark_grey().to_string());
                result.push_str(&colorize_json_value(value));
            } else {
                // String value in array
                result.push_str(&line.yellow().to_string());
            }
        } else if trimmed.starts_with('{')
            || trimmed.starts_with('}')
            || trimmed.starts_with('[')
            || trimmed.starts_with(']')
        {
            result.push_str(&line.dark_grey().to_string());
        } else {
            // Bare value in array
            let indent = &line[..line.len() - trimmed.len()];
            result.push_str(indent);
            result.push_str(&colorize_json_value(trimmed));
        }
        result.push('\n');
    }

    // Remove trailing newline
    if result.ends_with('\n') {
        result.pop();
    }
    result
}

/// Find the colon separating a JSON key from its value, skipping the key string.
fn find_key_colon(s: &str) -> Option<usize> {
    // Expect: "key": value
    if !s.starts_with('"') {
        return None;
    }
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
            continue;
        }
        if bytes[i] == b'"' {
            // Found end of key, look for ": "
            if s.get(i + 1..i + 3) == Some(": ") {
                return Some(i + 1);
            }
            return None;
        }
        i += 1;
    }
    None
}

/// Colorize a JSON value (right side of colon, or bare array element).
fn colorize_json_value(v: &str) -> String {
    use crossterm::style::Stylize;
    let stripped = v.trim_end_matches(',');
    let comma = if v.ends_with(',') {
        ",".dark_grey().to_string()
    } else {
        String::new()
    };

    let colored = if stripped == "null" {
        stripped.dark_grey().to_string()
    } else if stripped == "true" || stripped == "false" {
        stripped.yellow().to_string()
    } else if stripped.starts_with('"') {
        stripped.green().to_string()
    } else {
        // number
        stripped.cyan().to_string()
    };

    format!("{colored}{comma}")
}

/// Color a status string for terminal output. Covers vocabulary from both
/// query runs (succeeded/failed/running/queued/pending) and results (ready/expired/processing).
pub fn color_status(status: &str) -> String {
    use crossterm::style::{Color, Stylize};
    let color = match status {
        "succeeded" | "ready" => Color::Green,
        "failed" => Color::Red,
        "running" | "queued" | "pending" | "processing" => Color::Yellow,
        "expired" => Color::DarkGrey,
        _ => Color::Reset,
    };
    status.with(color).to_string()
}

/// Format an ISO date string compactly: "2024-03-15 14:23" (no seconds, no timezone).
pub fn format_date(s: &str) -> String {
    let s = s.split('.').next().unwrap_or(s).trim_end_matches('Z');
    let s = s.replace('T', " ");
    s.chars().take(16).collect()
}

pub fn api_error(body: String) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
        // Two shapes in the wild:
        //   {"error": {"message": "..."}}   — RuntimeDB-style
        //   {"error": "snake_case_code"}    — Django-style (e.g. workspace endpoints)
        if let Some(m) = v["error"]["message"].as_str() {
            return m.to_string();
        }
        if let Some(code) = v["error"].as_str() {
            return humanize_error_code(code);
        }
    }
    if body.trim_start().starts_with('<') {
        return "unexpected server error".to_string();
    }
    body
}

/// Turn a snake_case error code into a human-friendly sentence:
/// ``workspace_not_found`` → ``Workspace not found``. Cheap heuristic — if
/// a code reads badly after this, the server should be the one to fix
/// it by returning a real message.
/// True when an error response body carries `error.code == "ACCESS_DENIED"` —
/// the gateway/runtimedb signal that the credential's allow-list forbids the
/// operation (e.g. a database API token calling a non-allowed endpoint).
pub fn is_access_denied(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["code"]
                .as_str()
                .map(|code| code == "ACCESS_DENIED")
        })
        .unwrap_or(false)
}

fn humanize_error_code(code: &str) -> String {
    let spaced = code.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mask_credential_long_shows_prefix_and_suffix() {
        // 12+ chars: show both ends so the user can tell which token
        // is on the wire (database JWT vs user JWT vs opaque API token).
        assert_eq!(mask_credential("abcdefghijkl"), "abcd...ijkl");
        assert_eq!(mask_credential("eyJhMIDDLEYwxyz"), "eyJh...wxyz");
    }

    #[test]
    fn mask_credential_medium_falls_back_to_head_only() {
        // Between 5 and 11 chars: showing both ends would overlap.
        assert_eq!(mask_credential("abcdefgh"), "abcd...");
    }

    #[test]
    fn mask_credential_short() {
        assert_eq!(mask_credential("abcd"), "***");
        assert_eq!(mask_credential(""), "***");
    }

    #[test]
    fn api_error_humanizes_snake_case_code() {
        // Django-style flat shape — `workspace_not_found` should render
        // as a readable sentence, not a raw JSON blob.
        let body = r#"{"error": "workspace_not_found"}"#.to_string();
        assert_eq!(api_error(body), "Workspace not found");
    }

    #[test]
    fn api_error_prefers_nested_message_over_code() {
        // RuntimeDB-style nested shape — use the human message verbatim.
        let body = r#"{"error": {"message": "Query qrun_x not found"}}"#.to_string();
        assert_eq!(api_error(body), "Query qrun_x not found");
    }

    #[test]
    fn api_error_falls_through_for_plain_body() {
        let body = "raw text body".to_string();
        assert_eq!(api_error(body), "raw text body");
    }

    #[test]
    fn api_error_handles_html_body() {
        let body = "<html>500</html>".to_string();
        assert_eq!(api_error(body), "unexpected server error");
    }

    #[test]
    fn is_access_denied_detects_code() {
        assert!(is_access_denied(
            r#"{"error":{"code":"ACCESS_DENIED","message":"nope"}}"#
        ));
        // Other error codes / shapes are not access-denied.
        assert!(!is_access_denied(
            r#"{"error":{"code":"NOT_FOUND","message":"x"}}"#
        ));
        assert!(!is_access_denied(r#"{"error":{"message":"no code"}}"#));
        assert!(!is_access_denied("not json"));
    }

    #[test]
    fn redact_json_fields_top_level() {
        let mut v = json!({
            "access_token": "long-secret-token",
            "expires_in": 300,
            "refresh_token": "another-secret"
        });
        redact_json_fields(&mut v, &["access_token", "refresh_token"]);
        assert_eq!(v["access_token"], "long...oken");
        assert_eq!(v["refresh_token"], "anot...cret");
        // Non-redacted keys untouched.
        assert_eq!(v["expires_in"], 300);
    }

    #[test]
    fn redact_json_fields_recurses_into_nested_objects_and_arrays() {
        let mut v = json!({
            "data": {
                "access_token": "secret-1234",
                "items": [
                    {"access_token": "nested-secret"}
                ]
            }
        });
        redact_json_fields(&mut v, &["access_token"]);
        // "secret-1234" is 11 chars — falls into the head-only branch.
        assert_eq!(v["data"]["access_token"], "secr...");
        // "nested-secret" is 13 chars — head + tail.
        assert_eq!(v["data"]["items"][0]["access_token"], "nest...cret");
    }

    #[test]
    fn redact_json_fields_no_match_is_noop() {
        let mut v = json!({"foo": "bar"});
        let original = v.clone();
        redact_json_fields(&mut v, &["access_token"]);
        assert_eq!(v, original);
    }

    #[test]
    fn redact_json_fields_skips_non_string_values() {
        // If a key matches but the value isn't a string, leave it
        // alone — we can't meaningfully mask a number/null/object.
        let mut v = json!({"access_token": null, "refresh_token": 123});
        redact_json_fields(&mut v, &["access_token", "refresh_token"]);
        assert_eq!(v["access_token"], serde_json::Value::Null);
        assert_eq!(v["refresh_token"], 123);
    }
}
