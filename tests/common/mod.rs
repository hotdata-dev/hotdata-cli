//! Shared support for the CLI's env-gated integration tests.
//!
//! These tests drive the compiled `hotdata` binary (via `CARGO_BIN_EXE_hotdata`)
//! against production, mirroring the scenario contract in
//! www.hotdata.dev/api/test-scenarios.yaml. The harness centralizes the
//! env-driven gating the SDKs express elsewhere (sdk-python's conftest fixtures,
//! sdk-rust's `tests/common/mod.rs`).
//!
//! Every test reads the same `HOTDATA_SDK_TEST_*` env vars and SKIPS cleanly
//! (early-returns with a notice on stderr) when they are unset, so `cargo test`
//! passes offline / in CI without secrets configured.
//!
//! The shared env vars are translated into the CLI's own configuration surface:
//!   HOTDATA_SDK_TEST_API_KEY      -> HOTDATA_API_KEY   (CLI mints a JWT from it)
//!   HOTDATA_SDK_TEST_API_URL      -> HOTDATA_API_URL
//!   HOTDATA_SDK_TEST_WORKSPACE_ID -> HOTDATA_WORKSPACE
//! plus an isolated `HOTDATA_CONFIG_DIR` (a tempdir) so the test never reads or
//! writes the developer's real `~/.hotdata` config.

#![allow(dead_code)]

use std::process::{Command, Output};

use tempfile::TempDir;

/// Default API host (matches the SDK harnesses and the `--api-url` default).
pub const DEFAULT_API_URL: &str = "https://api.hotdata.dev";

/// Name of the shared database that query-scoped scenarios target. Databases
/// persist (no auto-expiry), so — mirroring sdk-python's conftest — we reuse one
/// stable database keyed by name across runs rather than creating one per test.
pub const SHARED_DATABASE_NAME: &str = "sdkci-shared";

/// SQL catalog alias for [`SHARED_DATABASE_NAME`]. Must match `[a-z_][a-z0-9_]*`
/// and be globally unique; find-or-create keys on the name, so re-runs reuse it.
pub const SHARED_DATABASE_CATALOG: &str = "sdkci_shared";

/// Resolved test environment. Mirrors sdk-rust's `TestEnv`.
///
/// GitHub Actions sets `env:` keys even when the underlying secret/var is unset,
/// producing empty strings rather than absent keys. We treat empty strings as
/// absent (see [`load_env`]).
#[derive(Clone, Debug)]
pub struct TestEnv {
    pub api_key: Option<String>,
    pub workspace_id: Option<String>,
    pub api_url: String,
    pub connection_id: Option<String>,
}

impl TestEnv {
    /// True when both required credentials (api key + workspace id) are present.
    pub fn has_creds(&self) -> bool {
        self.api_key.is_some() && self.workspace_id.is_some()
    }
}

fn non_empty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|s| !s.is_empty())
}

/// Read the test environment. Empty strings are treated as absent; `api_url`
/// falls back to [`DEFAULT_API_URL`].
pub fn load_env() -> TestEnv {
    TestEnv {
        api_key: non_empty("HOTDATA_SDK_TEST_API_KEY"),
        workspace_id: non_empty("HOTDATA_SDK_TEST_WORKSPACE_ID"),
        api_url: non_empty("HOTDATA_SDK_TEST_API_URL")
            .unwrap_or_else(|| DEFAULT_API_URL.to_string()),
        connection_id: non_empty("HOTDATA_SDK_TEST_CONNECTION_ID"),
    }
}

/// `sdkci-<scenario>-<8 hex>` so any orphaned resources are identifiable and can
/// be swept. See www.hotdata.dev/api/README.md — every test-created resource
/// must use this prefix.
pub fn sdkci_name(scenario: &str) -> String {
    let id: u32 = rand::random();
    format!("sdkci-{scenario}-{id:08x}")
}

/// A configured CLI under test: resolved credentials plus an isolated config
/// directory. Owns a [`TempDir`] so config stays out of `~/.hotdata` for the
/// lifetime of the test.
pub struct Cli {
    pub env: TestEnv,
    config_dir: TempDir,
}

impl Cli {
    fn new(env: TestEnv) -> Self {
        let config_dir = tempfile::tempdir().expect("create temp config dir");
        Cli { env, config_dir }
    }

    /// The seeded workspace id (creds are guaranteed present by the skip macros).
    pub fn workspace_id(&self) -> &str {
        self.env
            .workspace_id
            .as_deref()
            .expect("creds checked by skip macro")
    }

    /// Base command: the binary, an isolated config dir, the test API URL, and a
    /// cleared environment so an ambient sandbox/database token can't leak in.
    /// Does NOT set credentials or a workspace.
    fn base(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotdata"));
        cmd.env("HOTDATA_CONFIG_DIR", self.config_dir.path())
            .env("HOTDATA_API_URL", &self.env.api_url)
            .env_remove("HOTDATA_API_KEY")
            .env_remove("HOTDATA_WORKSPACE")
            .env_remove("HOTDATA_SANDBOX")
            .env_remove("HOTDATA_SANDBOX_TOKEN")
            .env_remove("HOTDATA_DATABASE")
            .env_remove("HOTDATA_DATABASE_TOKEN")
            .arg("--no-input");
        cmd
    }

    /// Authenticated command locked to the seeded workspace. `HOTDATA_WORKSPACE`
    /// is set, so commands resolve it automatically and any conflicting `-w`
    /// flag is rejected by the CLI.
    pub fn cmd(&self) -> Command {
        let mut cmd = self.base();
        cmd.env(
            "HOTDATA_API_KEY",
            self.env.api_key.as_deref().expect("creds checked"),
        )
        .env("HOTDATA_WORKSPACE", self.workspace_id());
        cmd
    }

    /// Authenticated, but with no `HOTDATA_WORKSPACE` lock so a `-w <id>` flag
    /// takes effect (used by `auth_unknown_workspace`).
    pub fn cmd_unlocked_workspace(&self) -> Command {
        let mut cmd = self.base();
        cmd.env(
            "HOTDATA_API_KEY",
            self.env.api_key.as_deref().expect("creds checked"),
        );
        cmd
    }

    /// Run `hotdata <args>` (authenticated, workspace-locked) and return raw output.
    pub fn run(&self, args: &[&str]) -> Output {
        self.cmd()
            .args(args)
            .output()
            .expect("failed to spawn hotdata binary")
    }

    /// Run `hotdata <args> -o json`, assert success, and parse stdout as JSON.
    pub fn json(&self, args: &[&str]) -> serde_json::Value {
        let output = self.run(args);
        assert_success(&output, args);
        parse_json(&output.stdout, args)
    }
}

/// Build an [`Output`] runner for a command that should NOT carry credentials —
/// isolated config + API URL only. Used by `auth_missing_token_401`.
pub fn unauthenticated_output(api_url: &str, args: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("create temp config dir");
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
        .env("HOTDATA_CONFIG_DIR", dir.path())
        .env("HOTDATA_API_URL", api_url)
        .env_remove("HOTDATA_API_KEY")
        .env_remove("HOTDATA_WORKSPACE")
        .env_remove("HOTDATA_SANDBOX")
        .env_remove("HOTDATA_SANDBOX_TOKEN")
        .env_remove("HOTDATA_DATABASE")
        .env_remove("HOTDATA_DATABASE_TOKEN")
        .arg("--no-input")
        .args(args)
        .output()
        .expect("failed to spawn hotdata binary")
}

/// Assert a command succeeded, surfacing stdout+stderr on failure.
pub fn assert_success(output: &Output, args: &[&str]) {
    assert!(
        output.status.success(),
        "`hotdata {}` failed ({})\n--- stdout ---\n{}\n--- stderr ---\n{}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Parse bytes as JSON, panicking with context (the args + raw output) on failure.
pub fn parse_json(bytes: &[u8], args: &[&str]) -> serde_json::Value {
    serde_json::from_slice(bytes).unwrap_or_else(|e| {
        panic!(
            "`hotdata {}` stdout was not valid JSON ({e}):\n{}",
            args.join(" "),
            String::from_utf8_lossy(bytes)
        )
    })
}

/// Find-or-create the shared `sdkci-shared` managed database and return its id.
///
/// Queries require a database scope (the `-d` flag / `X-Database-Id` header);
/// a bare query returns 400 "a database is required". Mirroring sdk-python's
/// conftest, we reuse one stable database keyed by name rather than creating and
/// deleting one per test (which would leak on failure).
pub fn shared_database_id(cli: &Cli) -> String {
    let listing = cli.json(&["databases", "list", "-o", "json"]);
    if let Some(arr) = listing.as_array() {
        for db in arr {
            if db.get("name").and_then(|v| v.as_str()) == Some(SHARED_DATABASE_NAME) {
                if let Some(id) = db.get("id").and_then(|v| v.as_str()) {
                    return id.to_string();
                }
            }
        }
    }

    let created = cli.json(&[
        "databases",
        "create",
        "--name",
        SHARED_DATABASE_NAME,
        "--catalog",
        SHARED_DATABASE_CATALOG,
        "-o",
        "json",
    ]);
    created
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("databases create returned no id: {created}"))
        .to_string()
}

/// Build a [`Cli`] from the environment, or `None` if required creds are missing.
pub fn cli_or_skip() -> Option<Cli> {
    let env = load_env();
    if !env.has_creds() {
        return None;
    }
    Some(Cli::new(env))
}

/// Early-return out of a `#[test]` when credentials are unavailable, printing a
/// notice to stderr. Mirrors sdk-rust's macro of the same name. Evaluates to the
/// bound [`Cli`] when creds are present.
#[macro_export]
macro_rules! skip_if_no_creds {
    () => {{
        match $crate::common::cli_or_skip() {
            Some(cli) => cli,
            None => {
                eprintln!(
                    "SKIP {}: set HOTDATA_SDK_TEST_API_KEY and \
                     HOTDATA_SDK_TEST_WORKSPACE_ID to run this scenario",
                    module_path!()
                );
                return;
            }
        }
    }};
}

/// Like [`skip_if_no_creds!`] but also requires `HOTDATA_SDK_TEST_CONNECTION_ID`.
/// Returns `(Cli, connection_id)`.
#[macro_export]
macro_rules! skip_if_no_connection {
    () => {{
        match $crate::common::cli_or_skip() {
            Some(cli) => {
                let connection_id = cli.env.connection_id.clone();
                match connection_id {
                    Some(connection_id) => (cli, connection_id),
                    None => {
                        eprintln!(
                            "SKIP {}: set HOTDATA_SDK_TEST_CONNECTION_ID to run this scenario",
                            module_path!()
                        );
                        return;
                    }
                }
            }
            None => {
                eprintln!(
                    "SKIP {}: set HOTDATA_SDK_TEST_API_KEY and \
                     HOTDATA_SDK_TEST_WORKSPACE_ID to run this scenario",
                    module_path!()
                );
                return;
            }
        }
    }};
}
