//! `hotdata support report` — file a ticket through the API's support
//! intake (`POST {api_url}/v1/support/issues`). `client::support` owns the
//! HTTP call; this module owns composing the request and rendering the
//! result.
//!
//! Validation (`build_request`, and everything it calls) is deliberately
//! `Result`-returning rather than `eprintln!` + `process::exit` directly, so
//! every "reject before any HTTP call" path is a plain function call in
//! tests, not a process-terminating one.

use crate::client;
use crate::client::support::{SupportError, SupportIssue, SupportIssueRequest};
use crate::config;
use crate::util;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_LOGS_BYTES: usize = 256 * 1024;
const MAX_CONTEXT_VALUE_CHARS: usize = 500;
const MAX_USER_CONTEXT_PAIRS: usize = 20;
const MAX_SUBJECT_CHARS: usize = 200;
/// Cap on the server text folded into a generic error-message fallback — an
/// HTML page is already short-circuited by `util::api_error`, but a plain
/// non-JSON body (a raw stack trace, say) is echoed back verbatim.
const MAX_ERROR_BODY_CHARS: usize = 200;
/// The one non-error exit message ("aborted, nothing sent") shares the error
/// channel (`Result<_, String>`) with real validation failures; this marker
/// is how the top-level caller tells them apart to skip the "error: " prefix.
const ABORTED: &str = "aborted, nothing sent";

/// Subcommands for `hotdata support`.
#[derive(clap::Subcommand)]
pub enum SupportCommands {
    /// File a support ticket
    Report {
        /// Report body. Omit to compose in $EDITOR (TTY only)
        #[arg(short = 'm', long = "message")]
        message: Option<String>,

        /// Subject line (<= 200 chars). Required with -m; when composing in
        /// $EDITOR the first non-comment line of the file is the subject
        #[arg(long)]
        subject: Option<String>,

        /// Kind of report
        #[arg(long, default_value = "other", value_parser = ["bug", "question", "billing", "feature", "account", "other"])]
        kind: String,

        /// Severity
        #[arg(long, default_value = "medium", value_parser = ["urgent", "high", "medium", "low"])]
        severity: String,

        /// Workspace to attach (defaults to the active workspace from config)
        #[arg(short = 'w', long = "workspace-id", conflicts_with = "no_workspace")]
        workspace_id: Option<String>,

        /// Do not attach a workspace
        #[arg(long)]
        no_workspace: bool,

        /// Attach a text file as logs ('-' reads stdin); client-side cap 256 KiB
        #[arg(long)]
        logs: Option<String>,

        /// Extra context pair KEY=VALUE, repeatable (max 20)
        #[arg(long = "context")]
        context: Vec<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

#[allow(clippy::too_many_arguments)]
pub fn report(
    message: Option<String>,
    subject: Option<String>,
    kind: String,
    severity: String,
    workspace_id: Option<String>,
    no_workspace: bool,
    logs_path: Option<String>,
    context_pairs: Vec<String>,
    output: &str,
) {
    let profile = config::load("default").unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    report_with_profile(
        &profile,
        message,
        subject,
        kind,
        severity,
        workspace_id,
        no_workspace,
        logs_path,
        context_pairs,
        output,
    );
}

#[allow(clippy::too_many_arguments)]
fn report_with_profile(
    profile: &config::ProfileConfig,
    message: Option<String>,
    subject: Option<String>,
    kind: String,
    severity: String,
    workspace_id: Option<String>,
    no_workspace: bool,
    logs_path: Option<String>,
    context_pairs: Vec<String>,
    output: &str,
) {
    let (req, workspace_id, from_editor) = build_request(
        profile,
        message,
        subject,
        kind,
        severity,
        workspace_id,
        no_workspace,
        logs_path,
        context_pairs,
    )
    .unwrap_or_else(|msg| {
        if msg == ABORTED {
            eprintln!("{msg}");
        } else {
            eprintln!("error: {msg}");
        }
        std::process::exit(1);
    });

    send_and_report(profile, req, workspace_id, from_editor, output);
}

/// Send the built request and render the result. Split out from
/// `report_with_profile` so a test can drive it directly with a hand-built
/// request and an explicit `from_editor`, without spawning `$EDITOR`.
fn send_and_report(
    profile: &config::ProfileConfig,
    req: SupportIssueRequest,
    workspace_id: Option<String>,
    from_editor: bool,
    output: &str,
) {
    let result = client::support::post_support_issue(profile, workspace_id.as_deref(), &req);
    persist_on_editor_failure(&result, &req, from_editor);
    match result {
        Ok((issue, replay)) => print_success(&issue, replay, output),
        Err(e) => handle_error(&e, workspace_id.as_deref()),
    }
}

/// If the report was composed in `$EDITOR` and the send failed, persist it —
/// `open_editor`'s own temp file is already gone by now, so a lost send
/// would otherwise lose the text the user just wrote. A no-op for the
/// `-m`/`--subject` path (that text is still in the caller's shell history)
/// and for a successful send. Never touches `result`; it only adds a side
/// effect alongside it.
fn persist_on_editor_failure(
    result: &Result<(SupportIssue, bool), SupportError>,
    req: &SupportIssueRequest,
    from_editor: bool,
) {
    if from_editor && result.is_err() {
        persist_composed_report(&req.subject, &req.body);
    }
}

/// Save the just-composed report and tell the user how to re-file it, or —
/// if even that fails — print the whole thing to stderr so nothing is lost.
fn persist_composed_report(subject: &str, body: &str) {
    match save_draft(subject, body) {
        Ok(path) => {
            let path = path.display();
            eprintln!(
                "Your report was saved to {path}. Re-file it with: hotdata support report --subject '{subject}' -m \"$(tail -n +3 {path})\""
            );
        }
        Err(e) => {
            eprintln!("warning: could not save your report to disk: {e}");
            eprintln!("--- your report, so nothing is lost ---");
            eprintln!("Subject: {subject}");
            eprintln!();
            eprintln!("{body}");
        }
    }
}

/// Persist a composed report to disk as `support-draft-<unix-seconds>.md`
/// under the CLI config dir (mode 0600, same as the session file — the
/// content is the user's own report, not a credential, but there is no
/// reason to make it more visible than that). Format is `"<subject>\n\n
/// <body>\n"`, so `tail -n +3 <path>` recovers the body alone (matching the
/// re-file hint in [`persist_composed_report`]).
fn save_draft(subject: &str, body: &str) -> Result<PathBuf, String> {
    let dir = config::config_dir()?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("support-draft-{now}.md"));
    let content = format!("{subject}\n\n{body}\n");
    util::atomic_write(&path, content.as_bytes(), 0o600)?;
    Ok(path)
}

/// Resolve the workspace, compose the report text, build context, and load
/// logs — everything that can be rejected before an HTTP call is ever made.
/// Returns the request, the resolved workspace id (needed to render a
/// `workspace_not_found` error message later), and whether the text came
/// from `$EDITOR` (needed to decide whether a failed send should be
/// persisted to disk).
#[allow(clippy::too_many_arguments)]
fn build_request(
    profile: &config::ProfileConfig,
    message: Option<String>,
    subject: Option<String>,
    kind: String,
    severity: String,
    workspace_id: Option<String>,
    no_workspace: bool,
    logs_path: Option<String>,
    context_pairs: Vec<String>,
) -> Result<(SupportIssueRequest, Option<String>, bool), String> {
    let (workspace_id, workspace_locked) =
        resolve_optional_workspace(profile, workspace_id, no_workspace)?;
    let (subject, body, from_editor) = compose(message, subject)?;
    validate_subject(&subject)?;

    let mut context = default_context(profile, workspace_locked);
    merge_user_context(&mut context, &context_pairs)?;

    let logs = match logs_path {
        Some(path) => Some(load_logs(&path)?),
        None => None,
    };

    let req = SupportIssueRequest {
        subject,
        body,
        kind,
        severity,
        context,
        logs,
        idempotency_key: generate_idempotency_key(),
    };
    Ok((req, workspace_id, from_editor))
}

/// Resolve the workspace to attach. Unlike `main::resolve_workspace`, an
/// unconfigured default is not an error here — proceed with none rather than
/// block the one caller most likely to have a broken setup. Still honors the
/// `HOTDATA_WORKSPACE` lock: an explicit `--workspace-id` that disagrees with
/// it is still rejected, same as every other command.
///
/// Returns `(workspace_id, locked)`; `locked` feeds the `workspace_locked`
/// context key.
fn resolve_optional_workspace(
    profile: &config::ProfileConfig,
    provided: Option<String>,
    no_workspace: bool,
) -> Result<(Option<String>, bool), String> {
    if no_workspace {
        return Ok((None, false));
    }
    if let Ok(ws) = std::env::var("HOTDATA_WORKSPACE") {
        if let Some(flag) = &provided
            && flag != &ws
        {
            return Err(format!(
                "cannot override workspace -- locked by HOTDATA_WORKSPACE environment variable ({ws})"
            ));
        }
        return Ok((Some(ws), true));
    }
    if let Some(id) = provided {
        return Ok((Some(id), false));
    }
    // Deliberately NOT `client::credentials::default_workspace_id`: for an
    // api-key credential (`--api-key`/`HOTDATA_API_KEY`) that helper probes
    // `GET /workspaces` to discover scope. An exact workspace is optional for
    // filing a report, and the API being slow or down is exactly the
    // situation this command exists for — it must never block on a network
    // round trip just to guess a default. Read only the saved default
    // (`workspaces set` / a prior login moves one to the front); if there is
    // none, or the current credential can't actually reach it, file with no
    // workspace instead of guessing.
    Ok((
        profile.workspaces.first().map(|w| w.public_id.clone()),
        false,
    ))
}

/// Produce (subject, body, from_editor) from `-m`/`--subject`, or by
/// composing in `$EDITOR` when neither is usable. `from_editor` is what lets
/// a failed send later decide whether to persist a draft: `-m` text is still
/// in the caller's shell history, but `$EDITOR`'s own temp file is gone by
/// the time a send fails, so that text has nowhere else to live. The abort
/// case (empty compose) is signaled via the literal [`ABORTED`] string so the
/// caller skips the "error: " prefix on it.
fn compose(
    message: Option<String>,
    subject: Option<String>,
) -> Result<(String, String, bool), String> {
    if let Some(body) = message {
        let Some(subject) = subject else {
            return Err("--subject is required when using -m/--message".to_string());
        };
        return Ok((subject, body, false));
    }

    if !util::is_interactive() {
        return Err(
            "stdin is not a TTY; pass -m/--message and --subject to file a report non-interactively"
                .to_string(),
        );
    }

    let template = format!(
        "{}\n\n\
         # Lines starting with '#' are ignored. First non-comment line is the subject,\n\
         # the rest is the report body. Save and quit to send; empty to abort.\n",
        subject.unwrap_or_default()
    );
    let edited = util::open_editor(&template)?;
    let (subject, body) = parse_composed(&edited).ok_or_else(|| ABORTED.to_string())?;
    Ok((subject, body, true))
}

/// Pure parse of an edited compose file: strip `#`-comment lines, take the
/// first non-blank remaining line as the subject and everything after as the
/// body. `None` when either comes up empty — the abort case.
fn parse_composed(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines().filter(|l| !l.trim_start().starts_with('#'));
    let subject = loop {
        match lines.next() {
            Some(l) if l.trim().is_empty() => continue,
            Some(l) => break l.trim().to_string(),
            None => return None,
        }
    };
    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    if subject.is_empty() || body.is_empty() {
        return None;
    }
    Some((subject, body))
}

/// Truncate to `max` chars (not bytes), respecting UTF-8 boundaries.
fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Reject an over-long subject before any HTTP call — the server enforces
/// the same 200-char limit (`subject_too_long`), but there is no reason to
/// round-trip a request we already know it will refuse.
fn validate_subject(subject: &str) -> Result<(), String> {
    let len = subject.chars().count();
    if len > MAX_SUBJECT_CHARS {
        return Err(format!(
            "subject is too long ({len} chars; limit {MAX_SUBJECT_CHARS})"
        ));
    }
    Ok(())
}

fn default_context(
    profile: &config::ProfileConfig,
    workspace_locked: bool,
) -> BTreeMap<String, String> {
    let mut context = BTreeMap::new();
    context.insert(
        "cli_version".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    );
    context.insert(
        "os".to_string(),
        format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
    );
    context.insert("api_url".to_string(), profile.api_url.to_string());
    // No profile-name context key: ProfileConfig carries no name of its own
    // (it's looked up by an external string key, e.g. "default"), so there is
    // nothing to report here — the spec's "skip otherwise".
    context.insert("workspace_locked".to_string(), workspace_locked.to_string());
    context.insert(
        "no_input".to_string(),
        (!util::is_interactive()).to_string(),
    );
    context
}

/// Merge user `--context KEY=VALUE` pairs over the defaults (user wins).
/// Rejects a malformed pair, an empty key, or too many pairs — before any of
/// them are ever sent.
fn merge_user_context(
    context: &mut BTreeMap<String, String>,
    pairs: &[String],
) -> Result<(), String> {
    if pairs.len() > MAX_USER_CONTEXT_PAIRS {
        return Err(format!(
            "too many --context pairs ({} given, max {MAX_USER_CONTEXT_PAIRS})",
            pairs.len()
        ));
    }
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(format!("--context '{pair}' is not in KEY=VALUE form"));
        };
        if key.is_empty() {
            return Err(format!("--context '{pair}' has an empty key"));
        }
        context.insert(
            key.to_string(),
            truncate_chars(value, MAX_CONTEXT_VALUE_CHARS),
        );
    }
    Ok(())
}

/// Read `--logs`' file (or stdin for `-`), enforcing the 256 KiB client-side
/// cap before any HTTP call, then apply the one client-side redaction the
/// spec asks for: mask the value on a line that looks like an `Authorization:`
/// header. The server does the real redaction; this just keeps an obvious
/// credential out of `--debug` output and off the wire in the clear case
/// where the user pasted a raw curl invocation into their saved log.
fn load_logs(path: &str) -> Result<String, String> {
    let bytes = if path == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| format!("reading stdin for --logs: {e}"))?;
        buf
    } else {
        std::fs::read(path).map_err(|e| format!("reading --logs file '{path}': {e}"))?
    };
    if bytes.len() > MAX_LOGS_BYTES {
        return Err(format!(
            "--logs is {} bytes; the client-side cap is {MAX_LOGS_BYTES} bytes (256 KiB)",
            bytes.len()
        ));
    }
    Ok(redact_logs(&String::from_utf8_lossy(&bytes)))
}

fn redact_logs(text: &str) -> String {
    text.lines()
        .map(redact_log_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A saved log can carry a credential anywhere on the line, not just after a
/// header name at the start (a pasted `curl -H "Authorization: Bearer ..."`,
/// a timestamp-prefixed access log). Mask every `bearer <token>` found
/// case-insensitively at any position; fall back to the plain
/// `Authorization:` header case (no `Bearer` scheme) only when no token was
/// found that way, so a Bearer-scheme value is never masked twice.
fn redact_log_line(line: &str) -> String {
    if let Some(masked) = mask_bearer_tokens(line) {
        return masked;
    }
    mask_authorization_header_value(line).unwrap_or_else(|| line.to_string())
}

/// Find every case-insensitive `bearer ` in `line` and mask the token that
/// follows it — the run of chars up to whitespace, a quote, or end of line —
/// keeping "Bearer" (in whatever case it was written) and everything else on
/// the line untouched. `None` when the line has no `bearer ` at all.
fn mask_bearer_tokens(line: &str) -> Option<String> {
    // `to_ascii_lowercase` only rewrites ASCII bytes in place, so `lower` and
    // `line` share byte offsets even over multi-byte UTF-8 text — safe to
    // search one and slice the other.
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut pos = 0usize;
    let mut found_any = false;
    while let Some(rel) = lower[pos..].find("bearer ") {
        found_any = true;
        let keep_end = pos + rel + "bearer ".len();
        out.push_str(&line[pos..keep_end]);
        let token_start = keep_end;
        let token_len = line[token_start..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .unwrap_or(line.len() - token_start);
        out.push_str(&util::mask_credential(
            &line[token_start..token_start + token_len],
        ));
        pos = token_start + token_len;
    }
    if !found_any {
        return None;
    }
    out.push_str(&line[pos..]);
    Some(out)
}

/// A bare `Authorization: <value>` with no `Bearer` scheme (e.g. a raw
/// `hd_...` token) — mask the whole value, keeping the header name
/// canonically capitalized (matching prior behavior) and everything before
/// it on the line untouched. `authorization:` is located anywhere in the
/// line, not just at its start.
fn mask_authorization_header_value(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("authorization:")?;
    let indent = &line[..idx];
    let value_start = idx + "authorization:".len();
    let masked = util::mask_credential(line[value_start..].trim());
    Some(format!("{indent}Authorization: {masked}"))
}

fn generate_idempotency_key() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Serialize)]
struct ReportOutput<'a> {
    #[serde(flatten)]
    issue: &'a SupportIssue,
    replay: bool,
}

fn print_success(issue: &SupportIssue, replay: bool, output: &str) {
    match output {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&ReportOutput { issue, replay }).unwrap()
        ),
        "yaml" => print!(
            "{}",
            serde_yaml::to_string(&ReportOutput { issue, replay }).unwrap()
        ),
        "table" => {
            use crossterm::style::Stylize;
            println!(
                "Support request filed: {}",
                issue.public_id.as_str().green()
            );
            println!("Subject:   {}", issue.subject);
            let workspace = issue.workspace_public_id.as_deref().unwrap_or("none");
            println!(
                "Severity:  {}   Kind: {}   Workspace: {}",
                issue.severity, issue.kind, workspace
            );
            println!("Replies go to the email on your HotData account.");
            if replay {
                println!("(already filed; nothing new was sent)");
            }
        }
        _ => unreachable!(),
    }
}

/// The support endpoint's error envelope is Django-flat (`{"error":
/// "workspace_not_found"}`) — the code IS the string, unlike the nested
/// `{"error": {"code", "message"}}` shape `util::error_code` expects
/// (RuntimeDB/ingest style). Falls back to the nested shape too, in case the
/// webapp ever wraps it that way instead.
fn support_error_code(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v["error"]
        .as_str()
        .map(str::to_string)
        .or_else(|| v["error"]["code"].as_str().map(str::to_string))
}

/// The human message for a failed call — pure, so error-code mapping is
/// tested directly rather than by spawning the binary.
fn error_message(e: &SupportError, workspace_id: Option<&str>) -> String {
    match e {
        SupportError::Auth(m) => format!("{m}\nRun 'hotdata auth login' to authenticate."),
        SupportError::Connection(m) => format!("connection error: {m}"),
        SupportError::Decode(m) => format!("malformed response: {m}"),
        SupportError::Http { status, body } => {
            let code = support_error_code(body);
            match code.as_deref() {
                Some("not_found") => {
                    "support reporting is not enabled for your organization yet; email support@hotdata.dev"
                        .to_string()
                }
                Some("rate_limited") => {
                    "too many reports in the last hour; try again later or email support@hotdata.dev"
                        .to_string()
                }
                Some("workspace_not_found") => {
                    let id = workspace_id.unwrap_or("<unknown>");
                    format!(
                        "workspace '{id}' not found or not accessible; pass --no-workspace to file without one"
                    )
                }
                Some("missing_authorization") | Some("invalid_api_key") => {
                    format!(
                        "{}\nRun 'hotdata auth login' to authenticate.",
                        util::api_error(body.clone())
                    )
                }
                Some("body_too_long") => {
                    "report body is too long (limit 20000 characters)".to_string()
                }
                Some("subject_too_long") => {
                    "subject is too long (limit 200 characters)".to_string()
                }
                Some("subject_required") => "subject is required".to_string(),
                Some("body_required") => "report body is required".to_string(),
                Some(code) => format!("support request failed ({status} {code})"),
                // No stable code at all (an upstream 5xx, a framework-level
                // rejection) — still surface whatever the server said rather
                // than a bare status. `api_error` echoes a non-JSON,
                // non-HTML body verbatim, which could be an unbounded
                // stack trace; truncate so that can't flood the terminal.
                None => format!(
                    "support request failed ({status}): {}",
                    truncate_chars(&util::api_error(body.clone()), MAX_ERROR_BODY_CHARS)
                ),
            }
        }
    }
}

fn handle_error(e: &SupportError, workspace_id: Option<&str>) -> ! {
    use crossterm::style::Stylize;

    let message = error_message(e, workspace_id);
    eprintln!("{}", format!("error: {message}").red());
    if !message.contains("support@hotdata.dev") {
        eprintln!(
            "{}",
            "If this keeps happening, email support@hotdata.dev.".dark_grey()
        );
    }
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ApiUrl, ProfileConfig, test_helpers::with_temp_config_dir};

    fn mock_profile(url: &str) -> ProfileConfig {
        ProfileConfig {
            api_key: Some("hd_test_key".to_string()),
            api_url: ApiUrl(Some(url.to_string())),
            ..Default::default()
        }
    }

    // --- parse_composed (editor compose, pure) -----------------------------

    #[test]
    fn parse_composed_strips_comments_and_splits_subject_body() {
        let text = "\
My subject line

# Lines starting with '#' are ignored. First non-comment line is the subject,
# the rest is the report body. Save and quit to send; empty to abort.
First body paragraph.

Second paragraph.
";
        let (subject, body) = parse_composed(text).unwrap();
        assert_eq!(subject, "My subject line");
        assert_eq!(body, "First body paragraph.\n\nSecond paragraph.");
    }

    #[test]
    fn parse_composed_empty_subject_aborts() {
        assert!(parse_composed("\n# just a comment\n").is_none());
    }

    #[test]
    fn parse_composed_subject_only_no_body_aborts() {
        assert!(parse_composed("Just a subject\n\n# comment only\n").is_none());
    }

    #[test]
    fn parse_composed_ignores_comment_lines_inside_body() {
        let text = "Subject\n\nBody line one\n# an ignored comment mid-body\nBody line two";
        let (subject, body) = parse_composed(text).unwrap();
        assert_eq!(subject, "Subject");
        assert_eq!(body, "Body line one\nBody line two");
    }

    // --- compose (message/subject validation) -------------------------------

    #[test]
    fn compose_with_message_but_no_subject_errors() {
        let err = compose(Some("body text".to_string()), None).unwrap_err();
        assert!(err.contains("--subject"), "got: {err}");
    }

    #[test]
    fn compose_with_message_and_subject_succeeds_without_editor() {
        let (subject, body, from_editor) =
            compose(Some("body text".to_string()), Some("Subj".to_string())).unwrap();
        assert_eq!(subject, "Subj");
        assert_eq!(body, "body text");
        assert!(!from_editor);
    }

    #[test]
    fn compose_non_interactive_without_message_errors_before_editor() {
        // Forcing non-interactive means this returns before ever trying to
        // spawn $EDITOR — safe to call from a test.
        util::set_no_input(true);
        let err = compose(None, None).unwrap_err();
        util::set_no_input(false);
        assert!(err.contains("TTY"), "got: {err}");
        assert_ne!(err, ABORTED);
    }

    // --- context --------------------------------------------------------------

    #[test]
    fn default_context_carries_the_documented_keys() {
        let ctx = default_context(&mock_profile("https://api.example.test"), true);
        assert_eq!(ctx.get("cli_version").unwrap(), env!("CARGO_PKG_VERSION"));
        assert_eq!(ctx.get("api_url").unwrap(), "https://api.example.test");
        assert_eq!(ctx.get("workspace_locked").unwrap(), "true");
        assert!(ctx.contains_key("os"));
        assert!(ctx.contains_key("no_input"));
        assert!(!ctx.contains_key("profile"));
    }

    #[test]
    fn user_context_overrides_default_and_truncates_long_values() {
        let mut ctx = default_context(&mock_profile("https://api.example.test"), false);
        let long_value = "x".repeat(600);
        merge_user_context(
            &mut ctx,
            &[
                "api_url=overridden".to_string(),
                format!("extra={long_value}"),
            ],
        )
        .unwrap();
        assert_eq!(ctx.get("api_url").unwrap(), "overridden");
        assert_eq!(ctx.get("extra").unwrap().chars().count(), 500);
    }

    #[test]
    fn user_context_rejects_pair_without_equals() {
        let mut ctx = BTreeMap::new();
        let err = merge_user_context(&mut ctx, &["no-equals-sign".to_string()]).unwrap_err();
        assert!(err.contains("no-equals-sign"), "got: {err}");
        assert!(ctx.is_empty(), "a rejected pair must not be applied");
    }

    #[test]
    fn user_context_rejects_empty_key() {
        let mut ctx = BTreeMap::new();
        let err = merge_user_context(&mut ctx, &["=value".to_string()]).unwrap_err();
        assert!(err.contains("empty key"), "got: {err}");
    }

    #[test]
    fn user_context_rejects_more_than_twenty_pairs() {
        let pairs: Vec<String> = (0..21).map(|i| format!("k{i}=v")).collect();
        let mut ctx = BTreeMap::new();
        let err = merge_user_context(&mut ctx, &pairs).unwrap_err();
        assert!(err.contains("21"), "got: {err}");
    }

    // --- logs -------------------------------------------------------------------

    #[test]
    fn load_logs_under_cap_reads_and_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        std::fs::write(&path, "line one\nAuthorization: Bearer supersecrettoken\n").unwrap();
        let out = load_logs(path.to_str().unwrap()).unwrap();
        assert!(out.contains("line one"));
        assert!(!out.contains("supersecrettoken"));
    }

    #[test]
    fn load_logs_over_cap_errors_with_size_and_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        std::fs::write(&path, vec![b'x'; MAX_LOGS_BYTES + 1]).unwrap();
        let err = load_logs(path.to_str().unwrap()).unwrap_err();
        assert!(
            err.contains(&(MAX_LOGS_BYTES + 1).to_string()),
            "got: {err}"
        );
        assert!(err.contains("256 KiB"), "got: {err}");
    }

    #[test]
    fn redact_logs_masks_authorization_header_lines_only() {
        let input = "GET /v1/foo\nAuthorization: Bearer abcdefghijklmnop\nX-Other: fine\n";
        let out = redact_logs(input);
        assert!(out.contains("Authorization: Bearer abcd...mnop"));
        assert!(out.contains("X-Other: fine"));
        assert!(!out.contains("abcdefghijklmnop"));
    }

    #[test]
    fn redact_logs_preserves_indent_and_non_bearer_scheme() {
        let input = "  authorization: hd_abcdefghijkl\n";
        let out = redact_logs(input);
        assert_eq!(out, "  Authorization: hd_a...ijkl");
    }

    #[test]
    fn redact_logs_masks_a_curl_dash_h_bearer_token_mid_line() {
        let input = r#"curl -H "Authorization: Bearer hd_live_x123456789" https://api.hotdata.dev/v1/query"#;
        let out = redact_logs(input);
        assert!(
            out.contains(r#"Authorization: Bearer hd_l...6789""#),
            "got: {out}"
        );
        assert!(out.contains("https://api.hotdata.dev/v1/query"));
        assert!(!out.contains("hd_live_x123456789"));
    }

    #[test]
    fn redact_logs_masks_a_timestamp_prefixed_bearer_line() {
        let input = "2026-09-05T10:00:00Z Authorization: Bearer supersecrettoken1234";
        let out = redact_logs(input);
        assert!(out.starts_with("2026-09-05T10:00:00Z Authorization: Bearer "));
        assert!(!out.contains("supersecrettoken1234"));
    }

    #[test]
    fn redact_logs_line_with_no_credential_is_unchanged() {
        let input = "GET /v1/foo 200 12ms";
        assert_eq!(redact_logs(input), input);
    }

    // --- idempotency key --------------------------------------------------------

    #[test]
    fn generate_idempotency_key_is_32_lowercase_hex_chars() {
        let key = generate_idempotency_key();
        assert_eq!(key.len(), 32);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn generate_idempotency_key_is_not_constant() {
        assert_ne!(generate_idempotency_key(), generate_idempotency_key());
    }

    // --- workspace resolution -----------------------------------------------------

    #[test]
    fn no_workspace_flag_wins_even_with_a_saved_default() {
        let profile = ProfileConfig {
            workspaces: vec![config::WorkspaceEntry {
                public_id: "work_saved".into(),
                name: "Saved".into(),
            }],
            ..Default::default()
        };
        let (id, locked) = resolve_optional_workspace(&profile, None, true).unwrap();
        assert_eq!(id, None);
        assert!(!locked);
    }

    #[test]
    fn explicit_workspace_id_is_used_untouched() {
        let profile = ProfileConfig::default();
        let (id, locked) =
            resolve_optional_workspace(&profile, Some("work_explicit".to_string()), false).unwrap();
        assert_eq!(id.as_deref(), Some("work_explicit"));
        assert!(!locked);
    }

    #[test]
    fn no_configured_default_resolves_to_none_without_erroring() {
        // The behavior that differs from main::resolve_workspace: an
        // unconfigured profile must not error here.
        let profile = ProfileConfig::default();
        let (id, locked) = resolve_optional_workspace(&profile, None, false).unwrap();
        assert_eq!(id, None);
        assert!(!locked);
    }

    #[test]
    fn build_request_with_env_api_key_and_no_configured_default_makes_zero_http_calls() {
        // `client::credentials::default_workspace_id` would probe `GET
        // /workspaces` for an env/flag-sourced api key with no single-
        // workspace answer already known -- resolve_optional_workspace must
        // never do that. A report is exactly what gets filed when the API
        // is slow or down, so filing one must never block on it.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let probe = server.mock("GET", "/workspaces").expect(0).create();

        let mut profile = mock_profile(&server.url());
        profile.api_key_source = config::ApiKeySource::Env;
        assert!(
            profile.workspaces.is_empty(),
            "test setup: no saved default"
        );

        let (_req, id, _from_editor) = build_request(
            &profile,
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            None,
            false,
            None,
            vec![],
        )
        .unwrap();
        assert_eq!(id, None);
        probe.assert();
    }

    #[test]
    fn saved_default_is_used_when_no_flag_given() {
        let profile = ProfileConfig {
            workspaces: vec![config::WorkspaceEntry {
                public_id: "work_saved".into(),
                name: "Saved".into(),
            }],
            ..Default::default()
        };
        let (id, locked) = resolve_optional_workspace(&profile, None, false).unwrap();
        assert_eq!(id.as_deref(), Some("work_saved"));
        assert!(!locked);
    }

    // --- build_request: validation ordering / zero-HTTP guarantees ---------------

    #[test]
    fn build_request_context_without_equals_errors_before_any_http_call() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/v1/support/issues").expect(0).create();

        let err = build_request(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            None,
            vec!["broken".to_string()],
        )
        .unwrap_err();
        assert!(err.contains("KEY=VALUE"), "got: {err}");
        // build_request never talks to the network at all -- workspace
        // resolution reads only the saved default, never probes -- so this
        // always holds regardless of which validation failed; asserted
        // anyway as the documented guarantee.
        m.assert();
    }

    #[test]
    fn build_request_over_long_subject_errors_before_any_http_call() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/v1/support/issues").expect(0).create();

        let subject = "x".repeat(MAX_SUBJECT_CHARS + 1);
        let err = build_request(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some(subject),
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            None,
            vec![],
        )
        .unwrap_err();
        assert!(err.contains("201"), "got: {err}");
        assert!(err.contains("limit 200"), "got: {err}");
        m.assert();
    }

    #[test]
    fn build_request_logs_over_cap_errors_before_any_http_call() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/v1/support/issues").expect(0).create();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.log");
        std::fs::write(&path, vec![b'x'; MAX_LOGS_BYTES + 1]).unwrap();

        let err = build_request(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            Some(path.to_str().unwrap().to_string()),
            vec![],
        )
        .unwrap_err();
        assert!(err.contains("256 KiB"), "got: {err}");
        m.assert();
    }

    #[test]
    fn build_request_non_tty_without_message_errors_before_any_http_call() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server.mock("POST", "/v1/support/issues").expect(0).create();

        util::set_no_input(true);
        let err = build_request(
            &mock_profile(&server.url()),
            None,
            None,
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            None,
            vec![],
        )
        .unwrap_err();
        util::set_no_input(false);
        assert!(err.contains("TTY"), "got: {err}");
        m.assert();
    }

    #[test]
    fn build_request_no_workspace_resolves_to_none() {
        // The request itself never carries a workspace field (see
        // `SupportIssueRequest` — no such field exists to omit); the resolved
        // id travels alongside the request instead, for the `X-Workspace-Id`
        // header and for a `workspace_not_found` error message.
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1");
        let (_req, id, _from_editor) = build_request(
            &profile,
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            Some("work_ignored".to_string()),
            true,
            None,
            vec![],
        )
        .unwrap();
        assert_eq!(id, None);
    }

    #[test]
    fn build_request_resolves_the_provided_workspace() {
        let (_tmp, _guard) = with_temp_config_dir();
        let profile = mock_profile("http://127.0.0.1:1");
        let (_req, id, _from_editor) = build_request(
            &profile,
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            Some("work_abc".to_string()),
            false,
            None,
            vec![],
        )
        .unwrap();
        assert_eq!(id.as_deref(), Some("work_abc"));
    }

    // --- error message mapping (pure) ---------------------------------------------

    #[test]
    fn error_message_maps_not_found() {
        let e = SupportError::Http {
            status: 404,
            body: r#"{"error":"not_found"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("not enabled"));
        assert!(msg.contains("support@hotdata.dev"));
    }

    #[test]
    fn error_message_maps_rate_limited() {
        let e = SupportError::Http {
            status: 429,
            body: r#"{"error":"rate_limited"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("too many reports"));
    }

    #[test]
    fn error_message_maps_workspace_not_found_with_the_attempted_id() {
        let e = SupportError::Http {
            status: 404,
            body: r#"{"error":"workspace_not_found"}"#.to_string(),
        };
        let msg = error_message(&e, Some("work_bad"));
        assert!(msg.contains("work_bad"), "got: {msg}");
        assert!(msg.contains("--no-workspace"));
    }

    #[test]
    fn error_message_maps_401_codes_with_reauth_hint() {
        for code in ["missing_authorization", "invalid_api_key"] {
            let e = SupportError::Http {
                status: 401,
                body: format!(r#"{{"error":"{code}"}}"#),
            };
            let msg = error_message(&e, None);
            assert!(msg.contains("hotdata auth login"), "got: {msg}");
        }
    }

    #[test]
    fn error_message_maps_body_too_long_with_the_limit() {
        let e = SupportError::Http {
            status: 422,
            body: r#"{"error":"body_too_long"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("20000"));
    }

    #[test]
    fn error_message_maps_subject_too_long_with_the_limit() {
        let e = SupportError::Http {
            status: 422,
            body: r#"{"error":"subject_too_long"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("200"), "got: {msg}");
    }

    #[test]
    fn error_message_maps_subject_required() {
        let e = SupportError::Http {
            status: 422,
            body: r#"{"error":"subject_required"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("subject"), "got: {msg}");
        assert!(msg.contains("required"), "got: {msg}");
    }

    #[test]
    fn error_message_maps_body_required() {
        let e = SupportError::Http {
            status: 422,
            body: r#"{"error":"body_required"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("body"), "got: {msg}");
        assert!(msg.contains("required"), "got: {msg}");
    }

    #[test]
    fn error_message_falls_back_to_generic_with_status_and_code() {
        let e = SupportError::Http {
            status: 403,
            body: r#"{"error":"not_a_member"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("403"), "got: {msg}");
        assert!(msg.contains("not_a_member"), "got: {msg}");
    }

    #[test]
    fn error_message_falls_back_to_generic_with_no_code() {
        let e = SupportError::Http {
            status: 502,
            body: String::new(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("502"), "got: {msg}");
    }

    #[test]
    fn error_message_no_code_includes_the_server_text() {
        // No stable `error.code`/flat code at all, but the body still says
        // something useful (a FastAPI-style `detail`) — don't drop it.
        let e = SupportError::Http {
            status: 502,
            body: r#"{"detail":"upstream timeout"}"#.to_string(),
        };
        let msg = error_message(&e, None);
        assert!(msg.contains("502"), "got: {msg}");
        assert!(msg.contains("upstream timeout"), "got: {msg}");
    }

    #[test]
    fn error_message_no_code_truncates_a_long_raw_body() {
        // A non-JSON, non-HTML body is echoed verbatim by `util::api_error`
        // — an unbounded stack trace must not flood the terminal.
        let e = SupportError::Http {
            status: 500,
            body: "x".repeat(MAX_ERROR_BODY_CHARS + 500),
        };
        let msg = error_message(&e, None);
        assert!(
            msg.chars().count() < MAX_ERROR_BODY_CHARS + 100,
            "message not truncated, got {} chars",
            msg.chars().count()
        );
    }

    // --- editor draft persistence -------------------------------------------------

    #[test]
    fn save_draft_writes_subject_blank_line_body_at_0600() {
        use std::os::unix::fs::PermissionsExt;
        let (_tmp, _guard) = with_temp_config_dir();

        let path = save_draft("My subject", "My body\nsecond line").unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "My subject\n\nMy body\nsecond line\n");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn save_draft_filename_carries_a_unix_timestamp() {
        let (_tmp, _guard) = with_temp_config_dir();
        let path = save_draft("s", "b").unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with("support-draft-"), "got: {name}");
        assert!(name.ends_with(".md"), "got: {name}");
    }

    #[test]
    fn persist_on_editor_failure_writes_a_draft_when_send_failed_and_editor_composed() {
        // Drives the exact same two steps `send_and_report` performs on a
        // failed send (post, then the persist decision) without going
        // through the process-exiting `handle_error` — so this can run
        // in-process. The mock returning 503 twice exercises the real
        // retry-once-then-give-up path in `client::support`, via the
        // `pub(crate)` delay seam with `Duration::ZERO` so this doesn't eat
        // the real 2s `RETRY_DELAY` on every test run.
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .with_status(503)
            .expect(2)
            .create();

        let profile = mock_profile(&server.url());
        let req = SupportIssueRequest {
            subject: "Composed subject".to_string(),
            body: "Composed body\nsecond line".to_string(),
            kind: "bug".to_string(),
            severity: "high".to_string(),
            context: BTreeMap::new(),
            logs: None,
            idempotency_key: generate_idempotency_key(),
        };

        let result = client::support::post_support_issue_with_delay(
            &profile,
            None,
            &req,
            std::time::Duration::ZERO,
        );
        assert!(result.is_err(), "test setup: the mock must fail the send");
        m.assert();

        persist_on_editor_failure(&result, &req, true);

        let dir = config::config_dir().unwrap();
        let drafts: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("support-draft-")
            })
            .collect();
        assert_eq!(drafts.len(), 1, "expected exactly one draft file");
        let content = std::fs::read_to_string(drafts[0].path()).unwrap();
        assert_eq!(content, "Composed subject\n\nComposed body\nsecond line\n");
    }

    #[test]
    fn persist_on_editor_failure_is_a_noop_when_not_editor_composed() {
        let (_tmp, _guard) = with_temp_config_dir();
        let req = SupportIssueRequest {
            subject: "s".to_string(),
            body: "b".to_string(),
            kind: "bug".to_string(),
            severity: "high".to_string(),
            context: BTreeMap::new(),
            logs: None,
            idempotency_key: "k".to_string(),
        };
        let result: Result<(SupportIssue, bool), SupportError> =
            Err(SupportError::Connection("boom".to_string()));

        persist_on_editor_failure(&result, &req, false);

        let dir = config::config_dir().unwrap();
        let has_draft = std::fs::read_dir(&dir)
            .map(|mut entries| {
                entries.any(|e| {
                    e.ok().is_some_and(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("support-draft-")
                    })
                })
            })
            .unwrap_or(false);
        assert!(!has_draft, "-m path must never write a draft");
    }

    #[test]
    fn persist_on_editor_failure_is_a_noop_when_the_send_succeeded() {
        let (_tmp, _guard) = with_temp_config_dir();
        let req = SupportIssueRequest {
            subject: "s".to_string(),
            body: "b".to_string(),
            kind: "bug".to_string(),
            severity: "high".to_string(),
            context: BTreeMap::new(),
            logs: None,
            idempotency_key: "k".to_string(),
        };
        let issue = SupportIssue {
            public_id: "supp_1".to_string(),
            status: "queued".to_string(),
            subject: "s".to_string(),
            kind: "bug".to_string(),
            severity: "high".to_string(),
            workspace_public_id: None,
            created_at: "2026-09-05T00:00:00Z".to_string(),
        };
        let result = Ok((issue, false));

        persist_on_editor_failure(&result, &req, true);

        let dir = config::config_dir().unwrap();
        let has_draft = std::fs::read_dir(&dir)
            .map(|mut entries| {
                entries.any(|e| {
                    e.ok().is_some_and(|e| {
                        e.file_name()
                            .to_string_lossy()
                            .starts_with("support-draft-")
                    })
                })
            })
            .unwrap_or(false);
        assert!(!has_draft, "a successful send must never write a draft");
    }

    // --- report_with_profile: end-to-end against a mock server --------------------

    #[test]
    fn report_happy_path_posts_the_expected_body_and_workspace_header() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .match_header("Authorization", "Bearer hd_test_key")
            .match_header("X-Workspace-Id", "work_abc")
            .match_body(mockito::Matcher::AllOf(vec![
                mockito::Matcher::PartialJson(serde_json::json!({
                    "subject": "CLI hangs on query",
                    "body": "Every query against work_abc times out after 30s.",
                    "kind": "bug",
                    "severity": "high",
                    "context": {
                        "cli_version": env!("CARGO_PKG_VERSION"),
                        "priority": "urgent",
                    },
                })),
                mockito::Matcher::Regex(r#""idempotency_key":"[0-9a-f]{32}""#.to_string()),
                mockito::Matcher::Regex(r#""os":"[a-z]+/[a-z0-9_]+""#.to_string()),
            ]))
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_happy","status":"queued","subject":"CLI hangs on query","kind":"bug","severity":"high","workspace_public_id":"work_abc","created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        report_with_profile(
            &mock_profile(&server.url()),
            Some("Every query against work_abc times out after 30s.".to_string()),
            Some("CLI hangs on query".to_string()),
            "bug".to_string(),
            "high".to_string(),
            Some("work_abc".to_string()),
            false,
            None,
            vec!["priority=urgent".to_string()],
            "table",
        );

        m.assert();
    }

    #[test]
    fn report_no_workspace_sends_no_workspace_header() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .match_header("X-Workspace-Id", mockito::Matcher::Missing)
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_none","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        report_with_profile(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            Some("work_ignored".to_string()),
            true,
            None,
            vec![],
            "table",
        );

        m.assert();
    }

    #[test]
    fn report_with_logs_sends_the_redacted_logs_field() {
        let (_tmp, _guard) = with_temp_config_dir();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.log");
        let raw_logs = "boom\nAuthorization: Bearer topsecrettoken\n";
        std::fs::write(&path, raw_logs).unwrap();
        // What the request must carry: the secret masked, everything else
        // untouched. Computed via the function under test rather than
        // hand-typed, so this stays in sync with `redact_log_line`'s exact
        // masking width.
        let expected_logs = redact_logs(raw_logs);
        assert!(
            !expected_logs.contains("topsecrettoken"),
            "test setup bug: expected value still carries the raw secret"
        );

        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "logs": expected_logs,
            })))
            .with_status(202)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_logs","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        report_with_profile(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            Some(path.to_str().unwrap().to_string()),
            vec![],
            "table",
        );

        // The mock only matches a body carrying the *redacted* logs text, so
        // a hit here proves the raw secret never reached the wire.
        m.assert();
    }

    #[test]
    fn report_200_replay_completes_without_error() {
        let (_tmp, _guard) = with_temp_config_dir();
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/support/issues")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"ok":true,"issue":{"public_id":"supp_replay","status":"queued","subject":"s","kind":"bug","severity":"high","workspace_public_id":null,"created_at":"2026-09-05T00:00:00Z"}}"#,
            )
            .create();

        report_with_profile(
            &mock_profile(&server.url()),
            Some("body".to_string()),
            Some("Subj".to_string()),
            "bug".to_string(),
            "high".to_string(),
            None,
            true,
            None,
            vec![],
            "json",
        );

        m.assert();
    }
}
