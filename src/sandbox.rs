use crate::api::ApiClient;
use crate::config;
use crate::sandbox_session::{self, SandboxSession};
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
    sandboxes: Vec<Sandbox>,
}

#[derive(Deserialize)]
struct DetailResponse {
    sandbox: Sandbox,
}

/// Response shape of `/v1/auth/sandbox` and `/v1/auth/sandbox/<id>`.
#[derive(Deserialize)]
struct SandboxTokenResponse {
    token: String,
    refresh_token: String,
    sandbox_id: String,
    expires_in: u64,
    refresh_expires_in: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn persist_sandbox_session(resp: SandboxTokenResponse, workspace_id: &str) {
    let now = now_unix();
    let session = SandboxSession {
        access_token: resp.token,
        refresh_token: resp.refresh_token,
        sandbox_id: resp.sandbox_id,
        workspace_id: workspace_id.to_string(),
        access_expires_at: now + resp.expires_in,
        refresh_expires_at: now + resp.refresh_expires_in,
    };
    if let Err(e) = sandbox_session::save(&session) {
        eprintln!("warning: could not persist sandbox session: {e}");
    }
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/sandboxes");

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
                let rows: Vec<Vec<String>> = body
                    .sandboxes
                    .iter()
                    .map(|s| {
                        let marker = if current_sandbox.as_deref() == Some(&s.public_id) {
                            "*"
                        } else {
                            ""
                        };
                        vec![
                            marker.to_string(),
                            s.public_id.clone(),
                            s.name.clone(),
                            crate::util::format_date(&s.updated_at),
                        ]
                    })
                    .collect();
                crate::table::print(&["ACTIVE", "ID", "NAME", "UPDATED"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(sandbox_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sandboxes/{sandbox_id}");
    let body: DetailResponse = api.get(&path);
    let s = &body.sandbox;

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(s).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(s).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), s.public_id);
            println!("{}{}", label("name:"), s.name);
            println!(
                "{}{}",
                label("created:"),
                crate::util::format_date(&s.created_at)
            );
            println!(
                "{}{}",
                label("updated:"),
                crate::util::format_date(&s.updated_at)
            );
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
    let path = format!("/sandboxes/{sandbox_id}");
    let body: DetailResponse = api.get(&path);
    if body.sandbox.markdown.is_empty() {
        eprintln!("{}", "Sandbox markdown is empty.".dark_grey());
    } else {
        println!("{}", body.sandbox.markdown);
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
        RefreshKind::nothing()
            .with_processes(ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always)),
    );

    let current_pid = sysinfo::get_current_pid().ok()?;
    let mut pid = sys.process(current_pid)?.parent()?;

    for _ in 0..64 {
        let proc = sys.process(pid)?;
        let name = proc.name().to_string_lossy();
        if name == "hotdata"
            && proc.cmd().iter().any(|a| a == "sandbox")
            && proc.cmd().iter().any(|a| a == "run")
        {
            return Some(pid);
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

    // POST /auth/sandbox creates the sandbox AND mints a sandbox-scoped
    // JWT (+ refresh token) in one round-trip.
    let resp: SandboxTokenResponse = api.post("/auth/sandbox", &body);
    let sandbox_id = resp.sandbox_id.clone();
    persist_sandbox_session(resp, workspace_id);

    if let Err(e) = config::save_sandbox("default", &sandbox_id) {
        eprintln!("warning: could not save sandbox to config: {e}");
    }

    eprintln!("{}", "Sandbox created".green());
    match format {
        "json" => println!("{}", serde_json::json!({"public_id": sandbox_id})),
        "yaml" => print!(
            "{}",
            serde_yaml::to_string(&serde_json::json!({"public_id": sandbox_id})).unwrap()
        ),
        "table" => {
            println!("id:   {}", sandbox_id);
            if let Some(n) = name {
                println!("name: {}", n);
            }
        }
        _ => unreachable!(),
    }
}

pub fn update(
    workspace_id: &str,
    sandbox_id: &str,
    name: Option<&str>,
    markdown: Option<&str>,
    format: &str,
) {
    if name.is_none() && markdown.is_none() {
        eprintln!("error: provide at least one of --name or --markdown.");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({});
    if let Some(n) = name {
        body["name"] = serde_json::json!(n);
    }
    if let Some(m) = markdown {
        body["markdown"] = serde_json::json!(m);
    }

    let path = format!("/sandboxes/{sandbox_id}");
    let resp: DetailResponse = api.patch(&path, &body);
    let s = &resp.sandbox;

    eprintln!("{}", "Sandbox updated".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(s).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(s).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), s.public_id);
            println!("{}{}", label("name:"), s.name);
            println!(
                "{}{}",
                label("updated:"),
                crate::util::format_date(&s.updated_at)
            );
        }
        _ => unreachable!(),
    }
}

pub fn run(sandbox_id: Option<&str>, workspace_id: &str, name: Option<&str>, cmd: &[String]) {
    check_sandbox_lock();
    let api = ApiClient::new(Some(workspace_id));

    // Mint (or re-mint, for an existing sandbox) a sandbox-scoped JWT
    // by dispatching on grant_type at /auth/sandbox. Either way we
    // end up with a fresh bundle persisted to sandbox_session.json
    // before we spawn.
    let body = match sandbox_id {
        Some(id) => serde_json::json!({
            "grant_type": "existing_sandbox",
            "sandbox_id": id,
        }),
        None => {
            let mut b = serde_json::json!({});
            if let Some(n) = name {
                b["name"] = serde_json::json!(n);
            }
            b
        }
    };
    let resp: SandboxTokenResponse = api.post("/auth/sandbox", &body);

    let sid = resp.sandbox_id.clone();
    let sandbox_jwt = resp.token.clone();
    let sandbox_refresh = resp.refresh_token.clone();
    persist_sandbox_session(resp, workspace_id);

    eprintln!("{} {}", "sandbox:".dark_grey(), sid);
    eprintln!("{} {}", "workspace:".dark_grey(), workspace_id);

    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("HOTDATA_SANDBOX", &sid)
        .env("HOTDATA_WORKSPACE", workspace_id)
        .env("HOTDATA_API_URL", &api.api_url)
        .env("HOTDATA_SANDBOX_TOKEN", &sandbox_jwt)
        .env("HOTDATA_SANDBOX_REFRESH_TOKEN", &sandbox_refresh)
        .status();

    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("error: failed to execute '{}': {e}", cmd[0]);
            std::process::exit(1);
        }
    }
}

pub fn set(sandbox_id: Option<&str>, workspace_id: &str) {
    check_sandbox_lock();
    match sandbox_id {
        Some(id) => {
            // Mint a sandbox-scoped JWT against this existing id via
            // the grant_type=existing_sandbox dispatch. The call
            // doubles as an existence + access check (404/403 if the
            // user can't reach it).
            let api = ApiClient::new(Some(workspace_id));
            let body = serde_json::json!({
                "grant_type": "existing_sandbox",
                "sandbox_id": id,
            });
            let resp: SandboxTokenResponse = api.post("/auth/sandbox", &body);
            persist_sandbox_session(resp, workspace_id);

            if let Err(e) = config::save_sandbox("default", id) {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active sandbox updated".green());
            println!("id: {}", id);
        }
        None => {
            // Clear the active sandbox + its cached session.
            sandbox_session::clear();
            if let Err(e) = config::clear_sandbox("default") {
                eprintln!("error saving config: {e}");
                std::process::exit(1);
            }
            println!("{}", "Active sandbox cleared".green());
        }
    }
}

pub fn delete(sandbox_id: &str, workspace_id: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/sandboxes/{sandbox_id}");
    let (status, resp_body) = api.delete_raw(&path);

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    // If the deleted sandbox was the active one, clear the cached session
    // and config pointer so subsequent commands don't keep routing through
    // a stale sandbox JWT — mirroring what `sandbox set` (no args) does.
    let active = std::env::var("HOTDATA_SANDBOX")
        .ok()
        .or_else(|| config::load("default").ok().and_then(|p| p.sandbox));
    if active.as_deref() == Some(sandbox_id) {
        sandbox_session::clear();
        if let Err(e) = config::clear_sandbox("default") {
            eprintln!("warning: could not clear sandbox from config: {e}");
        }
    }

    eprintln!("{}", "Sandbox deleted".green());
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
        assert_eq!(
            find_sandbox_run_ancestor(),
            find_sandbox_run_ancestor_inner()
        );
    }

}
