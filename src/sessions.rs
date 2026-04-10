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

pub fn read(session_id: &str, workspace_id: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sessions/{session_id}");
    let body: DetailResponse = api.get(&path);
    if body.session.markdown.is_empty() {
        eprintln!("{}", "Session markdown is empty.".dark_grey());
    } else {
        print!("{}", body.session.markdown);
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
    if let Some(m) = markdown { body["markdown"] = serde_json::json!(m); }

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
