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

    println!("{}", "Sandbox created".green());
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

    println!("{}", "Sandbox updated".green());
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
    // by hitting the auth endpoint. The same call creates a sandbox
    // when no id is provided. Either way we end up with a fresh
    // bundle persisted to sandbox_session.json before we spawn.
    let resp: SandboxTokenResponse = match sandbox_id {
        Some(id) => {
            let path = format!("/auth/sandbox/{id}");
            api.post(&path, &serde_json::json!({}))
        }
        None => {
            let mut body = serde_json::json!({});
            if let Some(n) = name {
                body["name"] = serde_json::json!(n);
            }
            api.post("/auth/sandbox", &body)
        }
    };

    let sid = resp.sandbox_id.clone();
    let sandbox_jwt = resp.token.clone();
    let sandbox_refresh = resp.refresh_token.clone();
    persist_sandbox_session(resp, workspace_id);

    eprintln!("{} {}", "sandbox:".dark_grey(), sid);
    eprintln!("{} {}", "workspace:".dark_grey(), workspace_id);

    spawn_child_with_sandbox_env(&sid, workspace_id, &sandbox_jwt, &sandbox_refresh, &api.api_url, cmd);
}

/// Allow-list of parent environment variables to forward to a
/// `sandbox run` child. Anything outside this set is dropped, so the
/// child can't accidentally read the user's API key, AWS creds, or any
/// other secret the parent shell happens to expose.
///
/// Public so the unit test can assert against the exact set.
pub(crate) const SANDBOX_ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "TMPDIR",
];

/// Names of the auth env vars injected into the child. The CLI reads
/// `HOTDATA_SANDBOX_TOKEN` and treats it as the only valid bearer when
/// set (see `api::ApiClient::new`). Kept as a constant alongside the
/// allow-list so a future audit can compare both at a glance.
#[allow(dead_code)] // Referenced from the audit test below.
pub(crate) const SANDBOX_AUTH_ENV: &[&str] = &[
    "HOTDATA_SANDBOX",
    "HOTDATA_WORKSPACE",
    "HOTDATA_API_URL",
    "HOTDATA_SANDBOX_TOKEN",
    "HOTDATA_SANDBOX_REFRESH_TOKEN",
];

fn spawn_child_with_sandbox_env(
    sandbox_id: &str,
    workspace_id: &str,
    sandbox_jwt: &str,
    sandbox_refresh: &str,
    api_url: &str,
    cmd: &[String],
) {
    let mut command = std::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]);

    // Scrub: start from a clean environment and explicitly re-add
    // only what the child legitimately needs.
    command.env_clear();
    for key in SANDBOX_ENV_ALLOWLIST {
        if let Ok(val) = std::env::var(key) {
            command.env(key, val);
        }
    }
    command.env("HOTDATA_SANDBOX", sandbox_id);
    command.env("HOTDATA_WORKSPACE", workspace_id);
    command.env("HOTDATA_API_URL", api_url);
    command.env("HOTDATA_SANDBOX_TOKEN", sandbox_jwt);
    command.env("HOTDATA_SANDBOX_REFRESH_TOKEN", sandbox_refresh);

    match command.status() {
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
            // Mint a sandbox-scoped JWT against this existing id. The
            // call doubles as an existence + access check (404/403 if
            // the user can't reach it).
            let api = ApiClient::new(Some(workspace_id));
            let path = format!("/auth/sandbox/{id}");
            let resp: SandboxTokenResponse = api.post(&path, &serde_json::json!({}));
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

    #[test]
    fn sandbox_env_allowlist_excludes_sensitive_parent_state() {
        // The allowlist must not leak the parent's auth credentials or
        // the parent's active-sandbox state into a child that's
        // supposed to be running under the freshly-minted sandbox JWT.
        let forbidden = [
            "HOTDATA_API_KEY",
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "GITHUB_TOKEN",
        ];
        for k in forbidden {
            assert!(
                !SANDBOX_ENV_ALLOWLIST.contains(&k),
                "{k} must not be forwarded to sandbox child"
            );
        }
    }

    #[test]
    fn sandbox_auth_env_set_is_self_consistent() {
        // Anything the parent injects must also be the set the child's
        // ApiClient knows to read (HOTDATA_SANDBOX_TOKEN in particular).
        assert!(SANDBOX_AUTH_ENV.contains(&"HOTDATA_SANDBOX_TOKEN"));
        assert!(SANDBOX_AUTH_ENV.contains(&"HOTDATA_SANDBOX_REFRESH_TOKEN"));
        assert!(SANDBOX_AUTH_ENV.contains(&"HOTDATA_SANDBOX"));
        assert!(SANDBOX_AUTH_ENV.contains(&"HOTDATA_WORKSPACE"));
    }
}
