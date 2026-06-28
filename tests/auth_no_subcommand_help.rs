//! `hotdata auth` with no subcommand prints the `auth` help (listing its
//! subcommands) instead of triggering a browser login — the behavior change
//! from PR #182.
//!
//! Runs fully offline and without credentials: bare `auth` is exempt from the
//! update gate and hits no API. Before the change this arm called
//! `auth::login()`, which prints "Opening browser to log in..." and starts a
//! local callback server; the test asserts that flow is *not* started.

mod common;

use common::{unauthenticated_output, DEFAULT_API_URL};

#[test]
fn bare_auth_prints_subcommand_help() {
    let output = unauthenticated_output(DEFAULT_API_URL, &["auth"]);

    assert!(
        output.status.success(),
        "`hotdata auth` should exit 0 printing help\n--- stderr ---\n{}",
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Clap help block, listing every auth subcommand.
    assert!(
        stdout.contains("Usage"),
        "expected help output with a usage line; got:\n{stdout}"
    );
    for sub in ["login", "register", "status", "logout"] {
        assert!(
            stdout.contains(sub),
            "auth help missing `{sub}` subcommand; got:\n{stdout}"
        );
    }

    // The login flow must NOT have started. `auth::login()` prints this banner
    // before opening a browser / spinning up the callback server.
    let combined = format!(
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("Opening browser to log in"),
        "bare `auth` should print help, not start a login flow; got:\n{combined}"
    );
}
