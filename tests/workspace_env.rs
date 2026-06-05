use std::process::Command;

fn hotdata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
}

// --- workspace env lock tests ---
//
// `resolve_workspace` refuses to let a `--workspace-id`/`-w` flag override a
// workspace pinned by the HOTDATA_WORKSPACE env var. That check runs before
// any auth or network I/O, so any workspace-scoped subcommand exercises it;
// we use `datasets list` here.

#[test]
fn workspace_env_blocks_conflicting_flag() {
    let output = hotdata()
        .args(["datasets", "list", "-w", "other-ws"])
        .env("HOTDATA_WORKSPACE", "locked-ws")
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
    // Will fail later on auth, but should NOT fail on the workspace lock.
    let output = hotdata()
        .args(["datasets", "list", "-w", "ws-1"])
        .env("HOTDATA_WORKSPACE", "ws-1")
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("locked by HOTDATA_WORKSPACE"),
        "unexpected workspace lock error: {stderr}"
    );
}
