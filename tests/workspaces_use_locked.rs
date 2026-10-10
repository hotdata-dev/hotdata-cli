//! `hotdata workspaces use` refuses to switch while `HOTDATA_WORKSPACE` locks
//! the workspace. Saving a choice there would have no effect (the env var
//! wins), so the command must fail instead of printing "Switched workspace".
//!
//! Runs offline: the lock check comes before any API call.

use std::process::Command;

#[test]
fn workspaces_use_fails_when_locked_by_env() {
    let dir = tempfile::tempdir().expect("create temp config dir");
    let output = Command::new(env!("CARGO_BIN_EXE_hotdata"))
        .env("HOTDATA_CONFIG_DIR", dir.path())
        .env("HOTDATA_API_URL", "http://127.0.0.1:1")
        .env_remove("HOTDATA_API_KEY")
        .env("HOTDATA_WORKSPACE", "work_locked")
        .args(["--no-input", "workspaces", "use", "work_other"])
        .output()
        .expect("failed to spawn hotdata binary");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "expected failure; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("locked by HOTDATA_WORKSPACE"),
        "expected lock error; stderr:\n{stderr}"
    );
    assert!(
        !dir.path().join("config.yml").exists(),
        "a locked `use` must not write config"
    );
}
