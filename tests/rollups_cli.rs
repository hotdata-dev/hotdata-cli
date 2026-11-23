use std::process::Command;

fn hotdata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
}

fn help(args: &[&str]) -> String {
    let output = hotdata().args(args).arg("--help").output().unwrap();
    assert!(
        output.status.success(),
        "{args:?} --help failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn rollups_help_lists_every_verb() {
    let help = help(&["rollups"]);
    for verb in ["create", "list", "show", "rebuild", "window", "remove"] {
        assert!(help.contains(verb), "missing {verb} in help: {help}");
    }
}

#[test]
fn rollups_create_help_documents_its_flags_and_the_write_restriction() {
    let help = help(&["rollups", "create"]);
    for flag in [
        "--time-column",
        "--bucket",
        "--measure",
        "--dimensions",
        "--filter",
        "--window",
        "--database",
        "--async",
        "--output",
    ] {
        assert!(help.contains(flag), "missing {flag} in help: {help}");
    }
    assert!(help.contains("[alias: --by]"), "help: {help}");
    // Creating a rollup changes how its table can be written; the one place a
    // user reads before running the command has to say so.
    assert!(help.contains("restricts writes"), "help: {help}");
    assert!(help.contains("approx_percentile"), "help: {help}");
}

#[test]
fn rollups_verbs_document_their_arguments() {
    let list = help(&["rollups", "list"]);
    for flag in ["--schema", "--table", "--database", "--output"] {
        assert!(list.contains(flag), "missing {flag} in help: {list}");
    }
    assert!(help(&["rollups", "rebuild"]).contains("--async"));
    let window = help(&["rollups", "window"]);
    assert!(window.contains("<TABLE>"), "help: {window}");
    assert!(window.contains("<DURATION>"), "help: {window}");
}

/// `avg` is the kind people reach for first and the API has no such kind. The
/// refusal names the one to use, and comes from argument parsing, so it needs
/// no login and sends nothing.
#[test]
fn an_avg_measure_is_refused_with_the_kind_to_use() {
    let output = hotdata()
        .args([
            "rollups",
            "create",
            "public.requests",
            "--time-column",
            "ts",
            "--bucket",
            "1h",
            "--measure",
            "avg:bytes",
        ])
        .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
        .env("HOTDATA_API_URL", "http://127.0.0.1:1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("numeric:bytes"), "stderr: {stderr}");
}

#[test]
fn a_bad_duration_is_refused_with_the_units_to_use() {
    let output = hotdata()
        .args(["rollups", "window", "public.requests", "1w"])
        .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("s, m, h or d"), "stderr: {stderr}");
}
