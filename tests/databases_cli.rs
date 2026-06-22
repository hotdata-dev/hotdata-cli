use std::process::Command;

fn hotdata() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hotdata"))
}

#[test]
fn databases_help_lists_subcommands() {
    let output = hotdata().args(["databases", "--help"]).output().unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("list"));
    assert!(help.contains("create"));
    assert!(help.contains("delete"));
    assert!(help.contains("tables"));
    assert!(help.contains("attach"));
    assert!(help.contains("detach"));
}

#[test]
fn databases_create_help_documents_attach_flag() {
    let output = hotdata()
        .args(["databases", "create", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--attach"), "help: {help}");
    // The `connection=alias` form is the documented way to set the SQL alias.
    assert!(help.contains("connection=alias"), "help: {help}");
}

#[test]
fn databases_attach_help_documents_connection_and_alias() {
    let output = hotdata()
        .args(["databases", "attach", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--alias"), "help: {help}");
    assert!(help.contains("--database"), "help: {help}");
}

#[test]
fn databases_attach_requires_a_connection_argument() {
    // `connection` is a required positional — parsing must fail without it.
    let output = hotdata().args(["databases", "attach"]).output().unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("required") || combined.contains("CONNECTION"),
        "output: {combined}"
    );
}

#[test]
fn databases_create_help_documents_table_flag() {
    let output = hotdata()
        .args(["databases", "create", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--table"));
    assert!(help.contains("--name"));
}

#[test]
fn databases_tables_load_help_documents_file_and_upload_id() {
    let output = hotdata()
        .args(["databases", "tables", "load", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("load"));
    assert!(help.contains("--file"));
    assert!(help.contains("--upload-id"));
    assert!(help.contains("parquet"));
}

#[test]
fn databases_tables_load_rejects_both_file_and_upload_id_at_parse_time() {
    let output = hotdata()
        .args([
            "databases",
            "tables",
            "load",
            "t1",
            "--file",
            "a.parquet",
            "--upload-id",
            "upl_1",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("cannot be used with"),
        "output: {combined}"
    );
}
