//! Scenario: auth_unknown_workspace.
//!
//! A valid api key combined with a fabricated workspace id must be rejected and
//! must never leak data from another workspace. The CLI mints a JWT from the
//! real api key, then sends the fabricated id as the gateway-enforced
//! `X-Workspace-Id`; the server responds 4xx (403/404). We assert the command
//! exits non-zero and never prints a successful listing.
//!
//! Uses `databases list` — a workspace-scoped listing that returns a JSON array
//! — so a missing rejection would surface as leaked cross-workspace data. (The
//! scenario's `ops: ["*"]` allows any op; this is a real command that reaches
//! the gateway with the fabricated workspace id, unlike the removed
//! `connections list` this test previously called, which failed at arg-parse.)

mod common;

#[test]
fn auth_unknown_workspace() {
    let cli = skip_if_no_creds!();

    let fake_workspace = format!(
        "ws_{:08x}{:08x}",
        rand::random::<u32>(),
        rand::random::<u32>()
    );

    // Real api key (no HOTDATA_WORKSPACE lock) + fabricated workspace via -w.
    let output = cli
        .cmd_unlocked_workspace()
        .args(["databases", "list", "-w", &fake_workspace, "-o", "json"])
        .output()
        .expect("failed to spawn hotdata binary");

    assert!(
        !output.status.success(),
        "databases list with fabricated workspace {fake_workspace} must fail \
         (potential cross-workspace leak); stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    // Defensive: must not have leaked a successful JSON listing on stdout.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim())
            .ok()
            .and_then(|v| v.as_array().map(|a| !a.is_empty()))
            != Some(true),
        "fabricated workspace {fake_workspace} leaked a database listing:\n{stdout}"
    );
}
