use crate::api::ApiClient;
use crate::config;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Sandbox {
    public_id: String,
    name: String,
    markdown: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    #[serde(rename = "sessions")]
    sandboxes: Vec<Sandbox>,
}

#[derive(Deserialize)]
struct DetailResponse {
    #[serde(rename = "session")]
    sandbox: Sandbox,
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/sessions");

    let current_sandbox = std::env::var("HOTDATA_SANDBOX")
        .ok()
        .or_else(|| config::load("default").ok().and_then(|p| p.sandbox));

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.sandboxes).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.sandboxes).unwrap()),
        "table" => {
            if body.sandboxes.is_empty() {
                eprintln!("{}", "No sandboxes found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.sandboxes.iter().map(|s| {
                    let marker = if current_sandbox.as_deref() == Some(&s.public_id) { "*" } else { "" };
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

pub fn get(sandbox_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sessions/{sandbox_id}");
    let body: DetailResponse = api.get(&path);
    let s = &body.sandbox;

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

pub fn read(sandbox_id: &str, workspace_id: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sessions/{sandbox_id}");
    let body: DetailResponse = api.get(&path);
    if body.sandbox.markdown.is_empty() {
        eprintln!("{}", "Sandbox markdown is empty.".dark_grey());
    } else {
        print!("{}", body.sandbox.markdown);
    }
}

fn check_sandbox_lock() {
    if std::env::var("HOTDATA_SANDBOX").is_ok() || find_sandbox_run_ancestor().is_some() {
        eprintln!("error: sandbox is locked");
        std::process::exit(1);
    }
}

pub fn find_sandbox_run_ancestor() -> Option<sysinfo::Pid> {
    static CACHED: std::sync::OnceLock<Option<sysinfo::Pid>> = std::sync::OnceLock::new();
    *CACHED.get_or_init(find_sandbox_run_ancestor_inner)
}

fn find_sandbox_run_ancestor_inner() -> Option<sysinfo::Pid> {
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
            if proc.cmd().iter().any(|a| a == "sandbox")
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
    check_sandbox_lock();
    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({});
    if let Some(n) = name {
        body["name"] = serde_json::json!(n);
    }

    let resp: DetailResponse = api.post("/sessions", &body);
    let s = &resp.sandbox;

    // Set as the active sandbox in config
    if let Err(e) = config::save_sandbox("default", &s.public_id) {
        eprintln!("warning: could not save sandbox to config: {e}");
    }

    println!("{}", "Sandbox created".green());
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

pub fn update(workspace_id: &str, sandbox_id: &str, name: Option<&str>, markdown: Option<&str>, format: &str) {
    if name.is_none() && markdown.is_none() {
        eprintln!("error: provide at least one of --name or --markdown.");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({});
    if let Some(n) = name { body["name"] = serde_json::json!(n); }
    if let Some(m) = markdown { body["markdown"] = serde_json::json!(m); }

    let path = format!("/sessions/{sandbox_id}");
    let resp: DetailResponse = api.patch(&path, &body);
    let s = &resp.sandbox;

    println!("{}", "Sandbox updated".green());
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

pub fn run(sandbox_id: Option<&str>, workspace_id: &str, name: Option<&str>, cmd: &[String]) {
    check_sandbox_lock();
    let sid = match sandbox_id {
        Some(id) => {
            // Verify the sandbox exists
            let api = ApiClient::new(Some(workspace_id));
            let path = format!("/sessions/{id}");
            let _: DetailResponse = api.get(&path);
            id.to_string()
        }
        None => {
            // Create a new sandbox
            let api = ApiClient::new(Some(workspace_id));
            let mut body = serde_json::json!({});
            if let Some(n) = name {
                body["name"] = serde_json::json!(n);
            }
            let resp: DetailResponse = api.post("/sessions", &body);
            resp.sandbox.public_id
        }
    };

    eprintln!("{} {}", "sandbox:".dark_grey(), sid);
    eprintln!("{} {}", "workspace:".dark_grey(), workspace_id);

    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("HOTDATA_SANDBOX", &sid)
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
    fn find_sandbox_run_ancestor_returns_none_in_test() {
        // No `hotdata sandbox run` ancestor exists in the test runner
        assert!(find_sandbox_run_ancestor_inner().is_none());
    }

    #[test]
    fn find_sandbox_run_ancestor_cached_matches_inner() {
        // The cached version should agree with the inner function
        assert_eq!(find_sandbox_run_ancestor(), find_sandbox_run_ancestor_inner());
    }
}

pub fn set(sandbox_id: Option<&str>, workspace_id: &str) {
    check_sandbox_lock();
    match sandbox_id {
        Some(id) => {
            // Verify the sandbox exists by fetching it
            let api = ApiClient::new(Some(workspace_id));
            let path = format!("/sessions/{id}");
            let _: DetailResponse = api.get(&path);

            if let Err(e) = config::save_sandbox("default", id) {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active sandbox updated".green());
            println!("id: {}", id);
        }
        None => {
            // Clear the active sandbox
            if let Err(e) = config::clear_sandbox("default") {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active sandbox cleared".green());
        }
    }
}
