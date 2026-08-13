//! Parse-time and help-surface tests for the datasource / ingest / run command
//! groups. No network: these assert the CLI *contract* — which verbs exist,
//! which arguments are required, which flags are mutually exclusive, and that a
//! removed verb explains itself.

use std::process::Command;

fn hotdata() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotdata"));
    // ISOLATED FROM THE MACHINE RUNNING THE TEST. A spawned process inherits
    // this one's environment, so without these two the assertions below run
    // against whatever profile the developer happens to be logged into — and
    // the failure that produces is invisible locally and only shows up in CI,
    // which is the one place nobody is watching a test they just wrote.
    //
    // That is not hypothetical: three tests here asserted a retired verb's
    // explanation and passed on a laptop with a default workspace, while CI
    // had none and got an auth error instead.
    cmd.env("HOTDATA_CONFIG_DIR", config_dir());
    cmd.env_remove("HOTDATA_API_KEY");
    cmd.env_remove("HOTDATA_WORKSPACE_ID");
    cmd
}

/// One empty config directory for the whole file, kept alive for the run.
fn config_dir() -> &'static std::path::Path {
    static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    DIR.get_or_init(|| tempfile::tempdir().expect("temp config dir"))
        .path()
}

/// stdout + stderr together: clap writes usage errors to stderr, help to
/// stdout, and the removed-verb notices to stderr.
fn combined(args: &[&str]) -> (bool, String) {
    let out = hotdata().args(args).output().unwrap();
    (
        out.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

// --- the command tree --------------------------------------------------------

#[test]
fn datasource_help_lists_the_lifecycle_verbs() {
    let (ok, help) = combined(&["datasource", "--help"]);
    assert!(ok, "{help}");
    for verb in [
        "validate",
        "create",
        "list",
        "show",
        "update-config",
        "delete",
    ] {
        assert!(help.contains(verb), "missing {verb}: {help}");
    }
}

#[test]
fn ingest_help_lists_the_lifecycle_verbs_and_no_run_now() {
    let (ok, help) = combined(&["ingest", "--help"]);
    assert!(ok, "{help}");
    for verb in [
        "create", "list", "show", "cancel", "resume", "schedule", "runs",
    ] {
        assert!(help.contains(verb), "missing {verb}: {help}");
    }
    // The removal is documented where someone looking for it would look.
    assert!(help.contains("trigger-import"), "{help}");
    assert!(help.contains("--next now"), "{help}");
}

#[test]
fn run_help_lists_show_and_disambiguates_the_noun() {
    let (ok, help) = combined(&["run", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("show"), "{help}");
    // `hotdata run` sits next to `hotdata databases run` and `hotdata jobs`.
    assert!(help.contains("databases run"), "{help}");
}

// --- ids are the canonical arguments -----------------------------------------

#[test]
fn ingest_runs_requires_an_ingest_id() {
    let (ok, out) = combined(&["ingest", "runs"]);
    assert!(!ok, "should not parse: {out}");
    assert!(
        out.contains("required") || out.contains("INGEST_ID"),
        "{out}"
    );
}

#[test]
fn ingest_runs_accepts_the_id_as_a_flag_instead_of_a_positional() {
    // Parse-only proof: with neither form clap errors on the missing argument,
    // so reaching a *different* failure means --ingest-id satisfied it.
    let (ok, out) = combined(&["ingest", "runs", "--ingest-id", "ing_1", "--help"]);
    assert!(ok, "{out}");
    assert!(out.contains("--ingest-id"), "{out}");
}

#[test]
fn ingest_runs_rejects_the_id_given_twice() {
    let (ok, out) = combined(&["ingest", "runs", "ing_1", "--ingest-id", "ing_2"]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

#[test]
fn ingest_create_requires_a_datasource_id() {
    let (ok, out) = combined(&["ingest", "create", "--sql", "SELECT * FROM t"]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("--datasource-id"), "{out}");
}

#[test]
fn datasource_show_requires_a_datasource_id() {
    let (ok, out) = combined(&["datasource", "show"]);
    assert!(!ok, "should not parse: {out}");
    assert!(
        out.contains("required") || out.contains("DATASOURCE_ID"),
        "{out}"
    );
}

#[test]
fn run_show_requires_a_run_id() {
    let (ok, out) = combined(&["run", "show"]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("required") || out.contains("RUN_ID"), "{out}");
}

// --- mutually exclusive flag groups ------------------------------------------

#[test]
fn ingest_create_rejects_selector_and_sql_together() {
    let (ok, out) = combined(&[
        "ingest",
        "create",
        "--datasource-id",
        "ds_1",
        "--selector",
        "{}",
        "--sql",
        "SELECT * FROM t",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

#[test]
fn ingest_create_rejects_destination_json_alongside_its_shorthand_flags() {
    let (ok, out) = combined(&[
        "ingest",
        "create",
        "--datasource-id",
        "ds_1",
        "--selector",
        "{}",
        "--destination",
        "{}",
        "--database-id",
        "db_1",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

#[test]
fn ingest_schedule_rejects_schedule_json_alongside_every() {
    let (ok, out) = combined(&[
        "ingest",
        "schedule",
        "ing_1",
        "--schedule",
        "{}",
        "--every",
        "5m",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

#[test]
fn datasource_update_config_rejects_both_credential_flags() {
    let (ok, out) = combined(&[
        "datasource",
        "update-config",
        "ds_1",
        "--config",
        "{}",
        "--credentials",
        "{}",
        "--no-credentials",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

// --- filters the design doc calls out ----------------------------------------

#[test]
fn ingest_list_accepts_the_datasource_id_filter() {
    let (ok, help) = combined(&["ingest", "list", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("--datasource-id"), "{help}");
    assert!(help.contains("--type"), "{help}");
    assert!(help.contains("--state"), "{help}");
}

#[test]
fn datasource_list_accepts_family_and_state_filters() {
    let (ok, help) = combined(&["datasource", "list", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("--family"), "{help}");
    assert!(help.contains("--state"), "{help}");
}

#[test]
fn ingest_create_documents_the_type_values_and_payload_flags() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("one-time"), "{help}");
    assert!(help.contains("scheduled"), "{help}");
    assert!(help.contains("continuous"), "{help}");
    // Both payload styles from the design doc's CLI section.
    assert!(help.contains("@file.json"), "{help}");
    assert!(help.contains("--selector"), "{help}");
    assert!(help.contains("--destination"), "{help}");
    assert!(help.contains("--schedule"), "{help}");
}

#[test]
fn ingest_schedule_documents_every_and_next() {
    let (ok, help) = combined(&["ingest", "schedule", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("--every"), "{help}");
    assert!(help.contains("--next"), "{help}");
    assert!(help.contains("5m") || help.contains("30s"), "{help}");
}

// --- removed verbs -----------------------------------------------------------

#[test]
fn trigger_import_explains_its_removal_instead_of_vanishing() {
    let (ok, out) = combined(&["ingest", "trigger-import", "ing_1"]);
    assert!(!ok, "should fail: {out}");
    assert!(out.contains("no replacement"), "{out}");
    // Says *why*, not just "gone", and names the supported alternative.
    assert!(out.contains("last committed state"), "{out}");
    assert!(out.contains("--next now"), "{out}");
}

#[test]
fn renamed_verbs_point_at_their_replacements() {
    for (old, expected) in [
        ("new-datasource", "hotdata datasource create"),
        ("list-datasources", "hotdata datasource list"),
        ("show-datasource", "hotdata datasource show"),
        ("delete-datasource", "hotdata datasource delete"),
        ("new-import", "hotdata ingest create"),
        ("list-imports", "hotdata ingest list"),
        ("status", "hotdata run show"),
    ] {
        let (ok, out) = combined(&["ingest", old]);
        assert!(!ok, "{old} should fail: {out}");
        assert!(out.contains(expected), "{old}: {out}");
    }
}

#[test]
fn an_unknown_verb_is_not_claimed_to_be_a_removed_one() {
    let (ok, out) = combined(&["ingest", "crate"]);
    assert!(!ok, "should fail: {out}");
    assert!(out.contains("unrecognized subcommand"), "{out}");
    assert!(out.contains("create, list, show"), "{out}");
}
