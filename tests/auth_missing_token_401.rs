//! Scenario: auth_missing_token_401.
//!
//! A request with no credentials must be denied. The SDKs assert a literal 401
//! from the server because they can construct an unauthenticated client; the
//! CLI instead refuses *client-side* (it has no session, no api key, and
//! nothing to mint a JWT from), so the meaningful CLI equivalent is: an
//! authenticated command run with no credentials exits non-zero, reports an
//! auth/not-configured error, and never prints a workspace listing.
//!
//! Although this scenario sends no credentials, it still gates on the standard
//! test env (like sdk-python's `env` fixture) so `cargo test` with no secrets
//! configured does not run a live, misleading path.

mod common;

#[test]
fn auth_missing_token_401() {
    // Gate on creds so offline CI skips cleanly (mirrors the SDK env fixture).
    let _cli = skip_if_no_creds!();
    let env = common::load_env();

    // No api key, no session (isolated empty config), no workspace lock.
    let output =
        common::unauthenticated_output(&env.api_url, &["workspaces", "list", "-o", "json"]);

    assert!(
        !output.status.success(),
        "workspaces list without credentials must fail; stdout:\n{}",
        String::from_utf8_lossy(&output.stdout)
    );

    let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
    assert!(
        stderr.contains("auth") || stderr.contains("log in") || stderr.contains("not configured"),
        "expected an auth/not-configured error on stderr, got:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Defensive: must not have leaked a successful JSON listing on stdout.
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        serde_json::from_str::<serde_json::Value>(stdout.trim())
            .ok()
            .and_then(|v| v.as_array().map(|a| !a.is_empty()))
            != Some(true),
        "unauthenticated call leaked a workspace listing:\n{stdout}"
    );
}
