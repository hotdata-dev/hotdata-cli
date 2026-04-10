use std::process::Command;

fn hotdata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
}

// --- session lock tests ---

#[test]
fn sessions_run_blocked_when_hotdata_session_set() {
    let output = hotdata()
        .args(["sessions", "run", "echo", "hi"])
        .env("HOTDATA_SESSION", "existing-session")
        .env("HOTDATA_WORKSPACE", "ws-1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session is locked"), "stderr: {stderr}");
}

#[test]
fn sessions_new_blocked_when_hotdata_session_set() {
    let output = hotdata()
        .args(["sessions", "new"])
        .env("HOTDATA_SESSION", "existing-session")
        .env("HOTDATA_WORKSPACE", "ws-1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session is locked"), "stderr: {stderr}");
}

#[test]
fn sessions_set_blocked_when_hotdata_session_set() {
    let output = hotdata()
        .args(["sessions", "set", "some-id"])
        .env("HOTDATA_SESSION", "existing-session")
        .env("HOTDATA_WORKSPACE", "ws-1")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("session is locked"), "stderr: {stderr}");
}

// --- workspace env lock tests ---

#[test]
fn workspace_env_blocks_conflicting_flag() {
    let output = hotdata()
        .args(["sessions", "-w", "other-ws", "list"])
        .env("HOTDATA_WORKSPACE", "locked-ws")
        .env_remove("HOTDATA_SESSION")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("locked by HOTDATA_WORKSPACE"),
        "stderr: {stderr}"
    );
}

#[test]
fn workspace_env_allows_matching_flag() {
    // When the flag matches the env var, no workspace conflict error.
    // Will fail later on auth, but should NOT fail on workspace lock.
    let output = hotdata()
        .args(["sessions", "-w", "ws-1", "list"])
        .env("HOTDATA_WORKSPACE", "ws-1")
        .env_remove("HOTDATA_SESSION")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("locked by HOTDATA_WORKSPACE"),
        "unexpected workspace lock error: {stderr}"
    );
}
