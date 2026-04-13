use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Create a steady-ticking spinner with a cyan glyph and the given message.
/// Writes to stderr so stdout (json/yaml output) stays clean.
pub fn spinner(msg: &str) -> indicatif::ProgressBar {
    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::with_template("{spinner:.cyan} {msg}").unwrap(),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

static DEBUG: AtomicBool = AtomicBool::new(false);

pub fn set_debug(enabled: bool) {
    DEBUG.store(enabled, Ordering::Relaxed);
}

pub fn is_debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// Log request details when debug mode is enabled.
pub fn debug_request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&serde_json::Value>) {
    if !is_debug() { return; }
    use crossterm::style::Stylize;
    eprintln!("{}", format!(">>> {method} {url}").dark_cyan());
    for (k, v) in headers {
        eprintln!("{}", format!("  {k}: {v}").dark_grey());
    }
    if let Some(b) = body {
        eprintln!("{}", colorize_json(&serde_json::to_string_pretty(b).unwrap()));
    }
}

/// Log response status and body when debug mode is enabled.
/// Consumes the response and returns the status + body text for the caller to parse.
pub fn debug_response(resp: reqwest::blocking::Response) -> (reqwest::StatusCode, String) {
    let status = resp.status();
    let body = resp.text().unwrap_or_default();

    if is_debug() {
        use crossterm::style::Stylize;
        let status_str = format!("<<< {} {}", status.as_u16(), status.canonical_reason().unwrap_or(""));
        if status.is_success() {
            eprintln!("{}", status_str.dark_green());
        } else {
            eprintln!("{}", status_str.dark_red());
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
            eprintln!("{}", colorize_json(&serde_json::to_string_pretty(&v).unwrap()));
        } else if !body.is_empty() {
            eprintln!("{}", body.to_string().dark_grey());
        }
    }

    (status, body)
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
        } else if trimmed.starts_with('{') || trimmed.starts_with('}')
            || trimmed.starts_with('[') || trimmed.starts_with(']')
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
    if !s.starts_with('"') { return None; }
    let mut i = 1;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        if bytes[i] == b'\\' { i += 2; continue; }
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
    let comma = if v.ends_with(',') { ",".dark_grey().to_string() } else { String::new() };

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

/// Format an ISO date string compactly: "2024-03-15 14:23" (no seconds, no timezone).
pub fn format_date(s: &str) -> String {
    let s = s.split('.').next().unwrap_or(s).trim_end_matches('Z');
    let s = s.replace('T', " ");
    s.chars().take(16).collect()
}

pub fn api_error(body: String) -> String {
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            if body.trim_start().starts_with('<') {
                "unexpected server error".to_string()
            } else {
                body
            }
        })
}
