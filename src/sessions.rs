use crate::api::ApiClient;
use crate::config;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Session {
    public_id: String,
    name: String,
    markdown: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    sessions: Vec<Session>,
}

#[derive(Deserialize)]
struct DetailResponse {
    session: Session,
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/sessions");

    let current_session = std::env::var("HOTDATA_SESSION")
        .ok()
        .or_else(|| config::load("default").ok().and_then(|p| p.session));

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.sessions).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.sessions).unwrap()),
        "table" => {
            if body.sessions.is_empty() {
                eprintln!("{}", "No sessions found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.sessions.iter().map(|s| {
                    let marker = if current_session.as_deref() == Some(&s.public_id) { "*" } else { "" };
                    vec![
                        marker.to_string(),
                        s.public_id.clone(),
                        s.name.clone(),
                        crate::util::format_date(&s.updated_at),
                    ]
                }).collect();
                crate::table::print(&["ACTIVE", "ID", "NAME", "UPDATED"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(session_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sessions/{session_id}");
    let body: DetailResponse = api.get(&path);
    let s = &body.session;

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(s).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(s).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), s.public_id);
            println!("{}{}", label("name:"), s.name);
            println!("{}{}", label("created:"), crate::util::format_date(&s.created_at));
            println!("{}{}", label("updated:"), crate::util::format_date(&s.updated_at));
            if !s.markdown.is_empty() {
                println!();
                println!("{}", "Markdown:".dark_grey());
                println!("{}", s.markdown);
            }
        }
        _ => unreachable!(),
    }
}

/// Resolve which session id to operate on for read-style commands, enforcing
/// the "can only read the active session" rule. Exits the process with an
/// error if the rule is violated or no session can be determined.
pub fn resolve_read_target(requested: Option<String>) -> String {
    let active = std::env::var("HOTDATA_SESSION").ok()
        .or_else(|| config::load("default").ok().and_then(|p| p.session));
    match (active, requested) {
        (Some(active_sid), Some(req)) if req != active_sid => {
            eprintln!(
                "error: cannot read other session while in an active session ({active_sid}).\n\
                 remove the active session with 'hotdata sessions set', or switch with 'hotdata sessions set <id>'."
            );
            std::process::exit(1);
        }
        (Some(active_sid), _) => active_sid,
        (None, Some(req)) => req,
        (None, None) => {
            eprintln!("error: no session ID provided and no active session. Provide <id> or use 'sessions new' / 'sessions set <id>'.");
            std::process::exit(1);
        }
    }
}

fn fetch_session(session_id: &str, workspace_id: &str) -> Session {
    let api = ApiClient::new(Some(workspace_id));
    let body: DetailResponse = api.get(&format!("/sessions/{session_id}"));
    body.session
}

fn print_updated(updated_at: &str) {
    eprintln!("updated: {updated_at}");
}

/// Interpret common C-style backslash escapes in CLI input. Lets users (and
/// agents) pass `\n`, `\t`, `\r`, `\\`, `\"` in a double-quoted shell arg and
/// get real control characters, matching `echo -e` / JSON conventions.
/// Unknown escapes (e.g. `\q`) pass through as literal `\q` unchanged.
fn unescape_cli(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('t') => out.push('\t'),
            Some('\\') => out.push('\\'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('0') => out.push('\0'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod unescape_tests {
    use super::unescape_cli;

    #[test]
    fn decodes_standard_escapes() {
        assert_eq!(unescape_cli(r"a\nb"), "a\nb");
        assert_eq!(unescape_cli(r"a\r\nb"), "a\r\nb");
        assert_eq!(unescape_cli(r"\t\\"), "\t\\");
    }

    #[test]
    fn passes_unknown_escapes_through() {
        assert_eq!(unescape_cli(r"\q"), r"\q");
    }

    #[test]
    fn handles_trailing_backslash() {
        assert_eq!(unescape_cli(r"abc\"), r"abc\");
    }
}

/// Parse ATX markdown headings (1–6 `#` prefix), skipping fenced code blocks.
/// Returns (line_number_1_indexed, level, text).
fn parse_headings(md: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut in_fence = false;
    for (i, line) in md.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence { continue; }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if level == 0 || level > 6 { continue; }
        let rest = &trimmed[level..];
        if !rest.is_empty() && !rest.starts_with(' ') { continue; }
        out.push((i + 1, level, rest.trim().to_string()));
    }
    out
}

/// Locate the start and end line (1-indexed, inclusive) of the section whose
/// heading text exactly matches `needle`. Section ends just before the next
/// heading at the same-or-higher level (= smaller or equal `#` count).
fn find_section(md: &str, needle: &str) -> Option<(usize, usize)> {
    let headings = parse_headings(md);
    let idx = headings.iter().position(|(_, _, t)| t == needle)?;
    let (start, level, _) = &headings[idx];
    let end = headings[idx + 1..]
        .iter()
        .find(|(_, lvl, _)| *lvl <= *level)
        .map(|(n, _, _)| n - 1)
        .unwrap_or_else(|| md.lines().count());
    Some((*start, end))
}

/// Parse a "--lines A:B" spec into an inclusive 1-indexed range, clamped to
/// the file length. Empty A -> 1, empty B -> total, negative A -> last N.
fn parse_line_range(spec: &str, total: usize) -> Result<(usize, usize), String> {
    let (a, b) = spec.split_once(':').ok_or_else(|| {
        format!("--lines must be in the form A:B (got {spec:?})")
    })?;
    let start = if a.is_empty() {
        1
    } else if let Some(n) = a.strip_prefix('-') {
        let n: usize = n.parse().map_err(|_| format!("invalid start: {a}"))?;
        total.saturating_sub(n).saturating_add(1).max(1)
    } else {
        a.parse::<usize>().map_err(|_| format!("invalid start: {a}"))?.max(1)
    };
    let end = if b.is_empty() {
        total
    } else {
        b.parse::<usize>().map_err(|_| format!("invalid end: {b}"))?
    };
    Ok((start, end.min(total)))
}

fn select_slice(md: &str, lines: Option<&str>, section: Option<&str>) -> String {
    match (lines, section) {
        (Some(_), Some(_)) => {
            eprintln!("error: --lines and --section are mutually exclusive");
            std::process::exit(1);
        }
        (Some(spec), None) => {
            let total = md.lines().count();
            match parse_line_range(spec, total) {
                Ok((start, end)) => slice_inclusive(md, start, end),
                Err(e) => { eprintln!("error: {e}"); std::process::exit(1); }
            }
        }
        (None, Some(heading)) => {
            match find_section(md, heading) {
                Some((start, end)) => slice_inclusive(md, start, end),
                None => {
                    eprintln!("error: no heading matched {heading:?}. Use 'sessions outline' to list headings.");
                    std::process::exit(1);
                }
            }
        }
        (None, None) => md.to_string(),
    }
}

fn slice_inclusive(md: &str, start: usize, end: usize) -> String {
    md.lines()
        .enumerate()
        .filter(|(i, _)| {
            let n = i + 1;
            n >= start && n <= end
        })
        .map(|(_, line)| line)
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn read(
    session_id: &str,
    workspace_id: &str,
    lines: Option<&str>,
    section: Option<&str>,
    output: &str,
) {
    let s = fetch_session(session_id, workspace_id);
    print_updated(&s.updated_at);

    if s.markdown.is_empty() {
        eprintln!("{}", "Session markdown is empty.".dark_grey());
        return;
    }

    let content = select_slice(&s.markdown, lines, section);
    if crate::markdown::should_style(output) {
        crate::markdown::render(&content);
    } else {
        print!("{content}");
        if !content.ends_with('\n') {
            println!();
        }
    }
}

pub fn outline(session_id: &str, workspace_id: &str, output: &str) {
    let s = fetch_session(session_id, workspace_id);
    print_updated(&s.updated_at);

    let headings = parse_headings(&s.markdown);
    if headings.is_empty() {
        eprintln!("{}", "No headings found.".dark_grey());
        return;
    }
    let styled = crate::markdown::should_style(output);
    for (line_num, level, text) in headings {
        if styled {
            let line_num_str = format!("{line_num:>4}").dark_grey();
            let heading = crate::markdown::style_heading(&text, level);
            println!("{line_num_str}  {heading}");
        } else {
            let hashes = "#".repeat(level);
            println!("{line_num:>4}  {hashes} {text}");
        }
    }
}

fn check_session_lock() {
    if std::env::var("HOTDATA_SESSION").is_ok() || find_session_run_ancestor().is_some() {
        eprintln!("error: session is locked");
        std::process::exit(1);
    }
}

pub fn find_session_run_ancestor() -> Option<sysinfo::Pid> {
    static CACHED: std::sync::OnceLock<Option<sysinfo::Pid>> = std::sync::OnceLock::new();
    *CACHED.get_or_init(find_session_run_ancestor_inner)
}

fn find_session_run_ancestor_inner() -> Option<sysinfo::Pid> {
    use sysinfo::{ProcessRefreshKind, RefreshKind, System, UpdateKind};

    let sys = System::new_with_specifics(
        RefreshKind::nothing().with_processes(
            ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        ),
    );

    let current_pid = sysinfo::get_current_pid().ok()?;
    let mut pid = sys.process(current_pid)?.parent()?;

    for _ in 0..64 {
        let proc = sys.process(pid)?;
        let name = proc.name().to_string_lossy();
        if name == "hotdata" {
            if proc.cmd().iter().any(|a| a == "sessions")
                && proc.cmd().iter().any(|a| a == "run")
            {
                return Some(pid);
            }
        }
        pid = proc.parent()?;
    }
    None
}

pub fn new(workspace_id: &str, name: Option<&str>, format: &str) {
    check_session_lock();
    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({});
    if let Some(n) = name {
        body["name"] = serde_json::json!(n);
    }

    let resp: DetailResponse = api.post("/sessions", &body);
    let s = &resp.session;

    // Set as the active session in config
    if let Err(e) = config::save_session("default", &s.public_id) {
        eprintln!("warning: could not save session to config: {e}");
    }

    println!("{}", "Session created".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(s).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(s).unwrap()),
        "table" => {
            println!("id:   {}", s.public_id);
            if !s.name.is_empty() {
                println!("name: {}", s.name);
            }
        }
        _ => unreachable!(),
    }
}

pub fn update(workspace_id: &str, session_id: &str, name: Option<&str>, markdown: Option<&str>, format: &str) {
    if name.is_none() && markdown.is_none() {
        eprintln!("error: provide at least one of --name or --markdown.");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({});
    if let Some(n) = name { body["name"] = serde_json::json!(n); }
    if let Some(m) = markdown {
        body["markdown"] = serde_json::json!(unescape_cli(m));
    }

    let path = format!("/sessions/{session_id}");
    let resp: DetailResponse = api.patch(&path, &body);
    let s = &resp.session;

    println!("{}", "Session updated".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(s).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(s).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), s.public_id);
            println!("{}{}", label("name:"), s.name);
            println!("{}{}", label("updated:"), crate::util::format_date(&s.updated_at));
        }
        _ => unreachable!(),
    }
}

pub fn run(session_id: Option<&str>, workspace_id: &str, name: Option<&str>, cmd: &[String]) {
    check_session_lock();
    let sid = match session_id {
        Some(id) => {
            // Verify the session exists
            let api = ApiClient::new(Some(workspace_id));
            let path = format!("/sessions/{id}");
            let _: DetailResponse = api.get(&path);
            id.to_string()
        }
        None => {
            // Create a new session
            let api = ApiClient::new(Some(workspace_id));
            let mut body = serde_json::json!({});
            if let Some(n) = name {
                body["name"] = serde_json::json!(n);
            }
            let resp: DetailResponse = api.post("/sessions", &body);
            resp.session.public_id
        }
    };

    eprintln!("{} {}", "session:".dark_grey(), sid);
    eprintln!("{} {}", "workspace:".dark_grey(), workspace_id);

    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("HOTDATA_SESSION", &sid)
        .env("HOTDATA_WORKSPACE", workspace_id)
        .status();

    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("error: failed to execute '{}': {e}", cmd[0]);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_session_run_ancestor_returns_none_in_test() {
        // No `hotdata sessions run` ancestor exists in the test runner
        assert!(find_session_run_ancestor_inner().is_none());
    }

    #[test]
    fn find_session_run_ancestor_cached_matches_inner() {
        // The cached version should agree with the inner function
        assert_eq!(find_session_run_ancestor(), find_session_run_ancestor_inner());
    }
}

pub fn set(session_id: Option<&str>, workspace_id: &str) {
    check_session_lock();
    match session_id {
        Some(id) => {
            // Verify the session exists by fetching it
            let api = ApiClient::new(Some(workspace_id));
            let path = format!("/sessions/{id}");
            let _: DetailResponse = api.get(&path);

            if let Err(e) = config::save_session("default", id) {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active session updated".green());
            println!("id: {}", id);
        }
        None => {
            // Clear the active session
            if let Err(e) = config::clear_session("default") {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active session cleared".green());
        }
    }
}
