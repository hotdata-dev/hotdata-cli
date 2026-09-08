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
    assert!(help.contains("count"));
    assert!(help.contains("create"));
    assert!(help.contains("remove"));
    assert!(help.contains("tables"));
    assert!(help.contains("attach"));
    assert!(help.contains("detach"));
}

#[test]
fn databases_count_help_documents_output_flag() {
    let output = hotdata()
        .args(["databases", "count", "--help"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--output"), "help: {help}");
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
    // The `catalog=alias` form is the documented way to set the SQL alias.
    assert!(help.contains("catalog=alias"), "help: {help}");
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
    // `catalog` is a required positional — parsing must fail without it.
    let output = hotdata().args(["databases", "attach"]).output().unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("required") || combined.contains("CATALOG"),
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

#[test]
fn databases_tables_help_lists_add() {
    let output = hotdata()
        .args(["databases", "tables", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("add"), "help: {help}");
    assert!(help.contains("load"), "help: {help}");
    assert!(help.contains("show"), "help: {help}");
}

#[test]
fn databases_tables_add_help_documents_key_and_layout_flags() {
    let output = hotdata()
        .args(["databases", "tables", "add", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--key"), "help: {help}");
    assert!(help.contains("--key-determines"), "help: {help}");
    assert!(help.contains("--sorted-by"), "help: {help}");
    assert!(help.contains("--partition-by"), "help: {help}");
}

#[test]
fn databases_tables_add_requires_a_table_argument() {
    let output = hotdata()
        .args(["databases", "tables", "add"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("required") || combined.contains("TABLE"),
        "output: {combined}"
    );
}

#[test]
fn databases_load_help_documents_mode_format_and_key() {
    let output = hotdata()
        .args(["databases", "load", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--mode"), "help: {help}");
    assert!(help.contains("--format"), "help: {help}");
    assert!(help.contains("--key"), "help: {help}");
    // The keyed modes are the point of --mode; they must be discoverable.
    for mode in ["replace", "append", "delete", "update", "upsert"] {
        assert!(help.contains(mode), "help missing '{mode}': {help}");
    }
}

#[test]
fn databases_load_rejects_an_unknown_mode_at_parse_time() {
    let output = hotdata()
        .args([
            "databases",
            "load",
            "--catalog",
            "c",
            "--table",
            "t",
            "--file",
            "a.csv",
            "--mode",
            "merge",
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
        combined.contains("invalid value") || combined.contains("possible values"),
        "output: {combined}"
    );
}

#[test]
fn databases_load_rejects_append_together_with_mode() {
    // `--append` is the old shorthand for `--mode append`; accepting both would
    // leave the effective mode ambiguous, so clap refuses the pair.
    let output = hotdata()
        .args([
            "databases",
            "load",
            "--catalog",
            "c",
            "--table",
            "t",
            "--file",
            "a.csv",
            "--append",
            "--mode",
            "upsert",
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

#[test]
fn databases_load_accepts_a_non_parquet_file_at_parse_time() {
    // A csv must get past argument parsing and the client entirely — the load
    // reads csv, json, and parquet, and an unrecognised extension is the
    // server's call, not a client-side rejection. Without credentials the run
    // fails later, on auth or the network, never on the file's extension.
    let output = hotdata()
        .args([
            "databases",
            "load",
            "--catalog",
            "c",
            "--table",
            "t",
            "--file",
            "/nonexistent/data.csv",
        ])
        .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
        .env("HOTDATA_WORKSPACE", "workffffffffffffffffffffffffffff")
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !combined.contains("require a parquet"),
        "csv was rejected client-side: {combined}"
    );
}

#[test]
fn databases_load_rejects_format_together_with_result_id() {
    // A stored result is always parquet and the load endpoint rejects `format`
    // beside `result_id`, so the request builder has no field to put it in.
    // Without this conflict the flag would be accepted and silently dropped.
    let output = hotdata()
        .args([
            "databases",
            "load",
            "--catalog",
            "c",
            "--table",
            "t",
            "--result-id",
            "rslt_1",
            "--format",
            "csv",
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

#[test]
fn databases_load_rejects_a_keyed_mode_with_result_id() {
    // Not a clap conflict — `--mode` conflicts with `--result-id` only for the
    // three keyed values, so the check is hand-rolled. It runs before any
    // config or network access, which is what makes it reachable here.
    for mode in ["delete", "update", "upsert"] {
        let output = hotdata()
            .args([
                "databases",
                "load",
                "--catalog",
                "c",
                "--table",
                "t",
                "--result-id",
                "rslt_1",
                "--mode",
                mode,
            ])
            .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
            .env("HOTDATA_WORKSPACE", "workffffffffffffffffffffffffffff")
            .output()
            .unwrap();
        assert!(!output.status.success(), "mode {mode} was accepted");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            combined.contains("matches rows by key"),
            "mode {mode} output: {combined}"
        );
    }

    // The non-keyed modes must still reach the network on the same input.
    for mode in ["replace", "append"] {
        let output = hotdata()
            .args([
                "databases",
                "load",
                "--catalog",
                "c",
                "--table",
                "t",
                "--result-id",
                "rslt_1",
                "--mode",
                mode,
            ])
            .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
            .env("HOTDATA_WORKSPACE", "workffffffffffffffffffffffffffff")
            .output()
            .unwrap();
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !combined.contains("matches rows by key"),
            "mode {mode} was rejected as keyed: {combined}"
        );
    }
}

#[test]
fn databases_tables_add_rejects_key_determines_without_a_key() {
    // `--key-determines` names columns the key fixes, so it is meaningless
    // without `--key` — the server would accept and ignore it.
    let output = hotdata()
        .args([
            "databases",
            "tables",
            "add",
            "t",
            "--key-determines",
            "tenant",
        ])
        .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
        .env("HOTDATA_WORKSPACE", "workffffffffffffffffffffffffffff")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("needs --key"), "output: {combined}");
}

#[test]
fn databases_tables_add_rejects_a_bad_sort_direction_and_transform() {
    for (flag, value, needle) in [
        ("--sorted-by", "ts=sideways", "asc or desc"),
        ("--partition-by", "created_at=week", "identity, year, month"),
    ] {
        let output = hotdata()
            .args(["databases", "tables", "add", "t", flag, value])
            .env("HOTDATA_CONFIG_DIR", "/nonexistent-config-dir")
            .env("HOTDATA_WORKSPACE", "workffffffffffffffffffffffffffff")
            .output()
            .unwrap();
        assert!(!output.status.success(), "{flag} {value} was accepted");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(combined.contains(needle), "{flag} output: {combined}");
    }
}
