use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Atomically replace the file at `path` with `bytes`: write to a tempfile in
/// the same directory, chmod it to `mode`, then rename over the destination.
/// Concurrent readers never observe a truncated or half-written file. Note
/// that rename replaces the destination entry itself — a symlinked
/// destination becomes a regular file.
pub fn atomic_write(path: &std::path::Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let parent = path.parent().ok_or("path has no parent directory")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    let mut tmp =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| format!("open failed: {e}"))?;
    tmp.write_all(bytes)
        .map_err(|e| format!("write failed: {e}"))?;
    tmp.as_file()
        .set_permissions(std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod failed: {e}"))?;
    tmp.persist(path)
        .map_err(|e| format!("write failed: {e}"))?;
    Ok(())
}

/// Open `$EDITOR` (falling back to `$VISUAL`, then `vi`) on a temp file
/// pre-filled with `initial`, wait for it to exit, and return the file's
/// final contents. `EDITOR`/`VISUAL` may carry arguments (e.g. `code --wait`);
/// the first whitespace-separated token is the program, the rest are passed
/// through before the file path.
///
/// Not unit-tested: spawning a real editor process has no place in a test
/// suite. Callers that need to test compose behavior should test the pure
/// parsing of the returned text instead.
pub fn open_editor(initial: &str) -> Result<String, String> {
    use std::io::Write;

    let mut tmp = tempfile::Builder::new()
        .suffix(".md")
        .tempfile()
        .map_err(|e| format!("creating temp file: {e}"))?;
    tmp.write_all(initial.as_bytes())
        .map_err(|e| format!("writing temp file: {e}"))?;
    tmp.flush().map_err(|e| format!("writing temp file: {e}"))?;
    let path = tmp.path().to_path_buf();

    let editor_cmd = std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| "vi".to_string());
    let mut parts = editor_cmd.split_whitespace();
    let program = parts.next().ok_or("EDITOR/VISUAL is set but empty")?;
    let args: Vec<&str> = parts.collect();

    let status = std::process::Command::new(program)
        .args(&args)
        .arg(&path)
        .status()
        .map_err(|e| format!("launching editor '{editor_cmd}': {e}"))?;
    if !status.success() {
        return Err(format!("editor '{editor_cmd}' exited with {status}"));
    }

    std::fs::read_to_string(&path).map_err(|e| format!("reading composed file: {e}"))
}

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
///
/// Counts and slices by `char`, not byte: real credentials are ASCII, but
/// this also runs over arbitrary `--logs` text, and byte-slicing an
/// arbitrary string panics the moment it lands mid multi-byte character.
pub fn mask_credential(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    if len >= 12 {
        let head: String = chars[..4].iter().collect();
        let tail: String = chars[len - 4..].iter().collect();
        format!("{head}...{tail}")
    } else if len > 4 {
        // Short-ish — still better to show head than nothing, but
        // don't double up on chars by showing a tail.
        let head: String = chars[..4].iter().collect();
        format!("{head}...")
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

/// Color a status string for terminal output. Covers vocabulary from query
/// runs (succeeded/failed/running/queued/pending), results (ready/expired/
/// processing), and ingest (done, plus an open set of in-flight stage states
/// like extracting/loading — which is why the fallback is yellow: terminal
/// statuses are all named here, so anything unknown is in flight).
pub fn color_status(status: &str) -> String {
    use crossterm::style::{Color, Stylize};
    let color = match status {
        "succeeded" | "ready" | "done" => Color::Green,
        "failed" => Color::Red,
        "expired" => Color::DarkGrey,
        _ => Color::Yellow,
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
        // Three shapes in the wild:
        //   {"error": {"code", "message", "details"}} — RuntimeDB / ingest-style
        //   {"error": "snake_case_code"}              — Django-style (e.g. workspace endpoints)
        //   {"detail": "..."}                         — FastAPI's framework-level rejections
        //                                               (malformed body, unhandled 422)
        if let Some(m) = v["error"]["message"].as_str() {
            return m.to_string();
        }
        if let Some(code) = v["error"].as_str() {
            return humanize_error_code(code);
        }
        if let Some(d) = v["detail"].as_str() {
            return d.to_string();
        }
        // Pydantic validation errors: [{"loc": [...], "msg": "..."}, …].
        if let Some(items) = v["detail"].as_array() {
            let msgs: Vec<String> = items
                .iter()
                .filter_map(|i| i["msg"].as_str().map(str::to_string))
                .collect();
            if !msgs.is_empty() {
                return msgs.join("; ");
            }
        }
    }
    if body.trim_start().starts_with('<') {
        return "unexpected server error".to_string();
    }
    // A dropped connection or bodyless 5xx yields an empty body; returning it
    // verbatim would print as a blank (color-codes-only) error line.
    if body.trim().is_empty() {
        return "unexpected empty response from server".to_string();
    }
    body
}

/// The stable machine code from an error envelope
/// (`{"error": {"code", "message", "details"}}`).
///
/// [`api_error`] returns the human half; this returns the half a script — or a
/// follow-up hint — can branch on, so callers that want both print both.
pub fn error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
}

/// The per-field rejections an envelope carries in `details.errors`, as
/// (field path, message) pairs — `[{"field": "table", "message": "…"}]` on the
/// wire, with an empty path for an error about the payload as a whole.
///
/// **The message half of a field-level 422 does not name the field.** It names
/// the payload and the family ("invalid destination for family 'iceberg'"),
/// because which field is wrong is exactly what `details` is for. A caller
/// shown only the message has to guess between every field the payload has —
/// and the guess this is here to prevent is between two fields that differ by
/// one word.
pub fn error_fields(body: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    v["error"]["details"]["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| {
                    let message = e["message"].as_str()?;
                    let field = e["field"].as_str().unwrap_or("").to_string();
                    Some((field, message.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One string field out of an error envelope's `details` object, e.g.
/// `conflicting_ingest_id` on a `destination_table_conflict`.
pub fn error_detail(body: &str, key: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v["error"]["details"][key].as_str().map(str::to_string))
}

/// True when an error response body carries `error.code == "ACCESS_DENIED"` —
/// the gateway/runtimedb signal that the credential's allow-list forbids the
/// operation (e.g. a database API token calling a non-allowed endpoint).
pub fn is_access_denied(body: &str) -> bool {
    error_code(body).as_deref() == Some("ACCESS_DENIED")
}

/// Human-readable byte count in binary units, keeping the exact value in
/// parentheses (table view only; JSON/YAML keep raw integers). Takes a `u64` so
/// the "negative bytes" state is unrepresentable; callers clamp any signed
/// wire value at the boundary.
pub fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {} ({n} B)", UNITS[u])
}

/// Turn a snake_case error code into a human-friendly sentence:
/// ``workspace_not_found`` → ``Workspace not found``. Cheap heuristic — if
/// a code reads badly after this, the server should be the one to fix
/// it by returning a real message.
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
    fn human_bytes_scales_units_and_keeps_exact() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB (1024 B)");
        assert_eq!(human_bytes(98_209_424), "93.7 MiB (98209424 B)");
    }

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
    fn mask_credential_non_ascii_does_not_panic() {
        // Byte-slicing this would panic mid multi-byte char; char-slicing
        // must not. 14 chars total, so the long-form head+tail branch.
        assert_eq!(mask_credential("token€12345678"), "toke...5678");
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
    fn api_error_reads_fastapi_detail_shapes() {
        // Framework-level rejections (malformed body, unhandled 422) never
        // reach the service's own error envelope.
        let body = r#"{"detail": "a drain appears to be running"}"#.to_string();
        assert_eq!(api_error(body), "a drain appears to be running");
        // Pydantic validation errors arrive as a list of {loc, msg, type}.
        let body = r#"{"detail": [{"loc": ["body", "selector"], "msg": "field required"},
                                  {"loc": ["body", "type"], "msg": "unexpected value"}]}"#
            .to_string();
        assert_eq!(api_error(body), "field required; unexpected value");
    }

    #[test]
    fn error_code_and_detail_read_the_envelope() {
        let body = r#"{"error": {"code": "destination_table_conflict",
                       "message": "already owns db_456.public.orders_raw",
                       "details": {"conflicting_ingest_id": "ing_old"}}}"#;
        assert_eq!(
            error_code(body).as_deref(),
            Some("destination_table_conflict")
        );
        assert_eq!(
            error_detail(body, "conflicting_ingest_id").as_deref(),
            Some("ing_old")
        );
        assert_eq!(error_detail(body, "missing"), None);
        // The message half stays with api_error — the two are complements.
        assert_eq!(
            api_error(body.to_string()),
            "already owns db_456.public.orders_raw"
        );
        // Shapes with no code yield none rather than a guess.
        assert_eq!(error_code(r#"{"detail": "nope"}"#), None);
        assert_eq!(error_code("not json"), None);
    }

    /// A field-level rejection: the message names the payload and the family,
    /// the details name the field. Reading only the first is how "invalid
    /// destination for family 'iceberg'" reaches a user who sent `table` and
    /// has to guess which of four fields the service meant.
    #[test]
    fn error_fields_read_the_per_field_rejections() {
        let body = r#"{"error": {"code": "invalid_destination",
                       "message": "invalid destination for family 'iceberg'",
                       "details": {"errors": [
                         {"field": "table", "message": "Extra inputs are not permitted"},
                         {"field": "database_id", "message": "Field required"}]}}}"#;
        assert_eq!(
            error_fields(body),
            vec![
                (
                    "table".to_string(),
                    "Extra inputs are not permitted".to_string()
                ),
                ("database_id".to_string(), "Field required".to_string()),
            ]
        );
        // An error about the payload as a whole has no field path; it is still
        // an error worth printing.
        let whole = r#"{"error": {"code": "invalid_destination", "message": "invalid",
                        "details": {"errors": [{"field": "", "message": "not an object"}]}}}"#;
        assert_eq!(
            error_fields(whole),
            vec![(String::new(), "not an object".to_string())]
        );
        // Envelopes with no details, and bodies that are not JSON at all.
        assert!(error_fields(r#"{"error": {"code": "x", "message": "y"}}"#).is_empty());
        assert!(error_fields("not json").is_empty());
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
    fn api_error_never_returns_blank_for_empty_body() {
        // A dropped connection or bodyless 5xx used to echo the empty body,
        // printing an error line that was nothing but color codes.
        assert_eq!(
            api_error(String::new()),
            "unexpected empty response from server"
        );
        assert_eq!(
            api_error("  \n".to_string()),
            "unexpected empty response from server"
        );
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
