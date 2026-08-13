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
///
/// `output()` gives the child an EMPTY stdin, which is also what makes the
/// guided-flow tests below safe to run from a developer's terminal: the CLI
/// treats a non-TTY stdin as "nobody is there to ask", so a test that reached
/// a prompt would hang on the terminal the test runner is attached to.
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
fn datasource_help_offers_the_field_reference_verb() {
    let (ok, help) = combined(&["datasource", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("fields"), "{help}");
}

#[test]
fn datasource_fields_takes_an_optional_family_and_documents_json_output() {
    let (ok, help) = combined(&["datasource", "fields", "--help"]);
    assert!(ok, "{help}");
    // Optional: with no family it lists them, so clap must not require one.
    assert!(help.contains("[FAMILY]"), "{help}");
    // The three payloads it is the reference for.
    for word in ["config", "credentials", "selector"] {
        assert!(help.contains(word), "missing {word}: {help}");
    }
    assert!(
        help.contains("-o json") || help.contains("JSON Schema"),
        "{help}"
    );
}

#[test]
fn the_payload_flags_point_at_the_field_reference_by_name() {
    // The one thing this command exists for: a caller who needs field names
    // must be sent to the generated reference, from the flags that take them.
    for (args, flag) in [
        (["datasource", "create", "--help"], "--config"),
        (["ingest", "create", "--help"], "--selector"),
    ] {
        let (ok, help) = combined(&args);
        assert!(ok, "{help}");
        assert!(help.contains(flag), "{flag} missing: {help}");
        assert!(
            help.contains("datasource fields"),
            "{args:?} must name the field reference: {help}"
        );
    }
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

// --- the documented list filters ---------------------------------------------

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
    // Both documented payload styles.
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

// --- the guided create flow, and the gate on it ------------------------------

#[test]
fn create_without_a_terminal_asks_for_flags_instead_of_prompting() {
    // The whole contract for scripts: no TTY means no questions, and the
    // arguments are demanded in the order a caller can act on them — the family
    // first, since it decides what the config even contains.
    let (ok, out) = combined(&["datasource", "create", "-w", "ws_test"]);
    assert!(!ok, "should fail: {out}");
    assert!(out.contains("--family is required"), "{out}");
    // And it says where the questions ARE, so the gate is discoverable.
    assert!(out.contains("terminal"), "{out}");

    let (ok, out) = combined(&["datasource", "create", "-w", "ws_test", "--family", "sql"]);
    assert!(!ok, "should fail: {out}");
    assert!(out.contains("--config is required"), "{out}");
}

#[test]
fn no_input_takes_the_flag_path_even_on_a_terminal() {
    let (ok, out) = combined(&[
        "datasource",
        "create",
        "-w",
        "ws_test",
        "--no-input",
        "--family",
        "sql",
    ]);
    assert!(!ok, "should fail: {out}");
    assert!(out.contains("--config is required"), "{out}");
}

#[test]
fn create_help_describes_the_guided_flow_and_what_turns_it_off() {
    let (ok, help) = combined(&["datasource", "create", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("--no-input"), "{help}");
    // The two things someone automating needs to know: it asks, and it does
    // not ask when there is nobody there.
    assert!(help.contains("terminal"), "{help}");
    assert!(help.contains("service"), "{help}");
}

// --- the shorthand flags ------------------------------------------------------

#[test]
fn the_datasource_shorthands_live_on_datasource_create() {
    let (ok, help) = combined(&["datasource", "create", "--help"]);
    assert!(ok, "{help}");
    for flag in ["--bucket-url", "--catalog-type"] {
        assert!(help.contains(flag), "{flag} missing: {help}");
    }
    // Each says which family it is for, so the flag list reads as a map of the
    // families rather than a pile of options.
    assert!(
        help.contains("filesystem") || help.contains("bucket"),
        "{help}"
    );
    assert!(help.contains("iceberg"), "{help}");
}

#[test]
fn the_selector_shorthands_live_on_ingest_create() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    for flag in [
        "--table",
        "--table-path",
        "--topic",
        "--schema",
        "--format",
        "--glob",
        "--record-shape",
        "--raw-sql",
        "--all",
        "--limit",
        "--source",
        "--dest-table",
    ] {
        assert!(help.contains(flag), "{flag} missing: {help}");
    }
    // Each names the sources it applies to — the fields moved to the selector,
    // so the help has to say which family's selector.
    assert!(help.contains("SQL, Iceberg, DuckLake"), "{help}");
    assert!(help.contains("bucket sources"), "{help}");
    assert!(help.contains("Delta sources"), "{help}");
    assert!(help.contains("Kafka sources"), "{help}");
}

/// Every family whose selection is ONE field has a flag for it. Without these
/// two, a Delta or Kafka ingest — a single path, a single topic — could only
/// be created by hand-writing the `--selector` document, which is the escape
/// hatch and not the path.
#[test]
fn the_single_field_selectors_each_have_a_flag() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    let help = flat(&help);
    // Delta's datasource is the storage ROOT, so the flag has to say the path
    // is relative to it — otherwise it reads as a whole URI.
    assert!(help.contains("--table-path"), "{help}");
    assert!(help.contains("under the datasource root"), "{help}");
    // Kafka's is the CLUSTER, which is why the topics are an ingest's choice
    // and not part of the credential's boundary.
    assert!(help.contains("--topic"), "{help}");
    assert!(help.contains("repeatable"), "{help}");
}

/// A Delta path and a Kafka topic name different families' selectors, so
/// asking for both is not a request any datasource has. clap says so without
/// a round trip.
#[test]
fn the_single_field_selector_flags_exclude_each_other() {
    for pair in [
        vec!["--table-path", "warehouse/orders", "--topic", "events"],
        vec!["--table-path", "warehouse/orders", "--table", "orders"],
        vec!["--topic", "events", "--table", "orders"],
    ] {
        let mut args = vec!["ingest", "create", "--datasource-id", "ds_1"];
        args.extend(pair.iter().copied());
        let (ok, out) = combined(&args);
        assert!(!ok, "{pair:?} should not parse: {out}");
        assert!(out.contains("cannot be used with"), "{pair:?}: {out}");
    }
}

/// Help text is wrapped to the terminal, so a phrase spanning a line break is
/// two lines with an indent between them. Asserting on the flattened form
/// pins what the sentence SAYS rather than where clap happened to break it.
fn flat(help: &str) -> String {
    help.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The two destination flags are two different fields, and which one applies
/// is a property of the SOURCE: one that lands a single table can be told that
/// table's name, one that lands a table per source table can be given at most
/// a common prefix. A flag that did not say which sources it was for would be
/// a flag whose 422 is the first thing that explains it.
#[test]
fn each_destination_flag_says_which_sources_it_is_for() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    let help = flat(&help);
    assert!(help.contains("--dest-table-prefix"), "{help}");
    // The single-table flag names the sources that land one …
    assert!(help.contains("land ONE table"), "{help}");
    assert!(help.contains("Delta"), "{help}");
    // … and the prefix flag names the ones that land several, plus what
    // omitting it does — the common case, and it is not "error".
    assert!(help.contains("lands SEVERAL"), "{help}");
    assert!(help.contains("Kafka"), "{help}");
    assert!(help.contains("used unchanged"), "{help}");
}

#[test]
fn a_table_and_a_table_prefix_are_not_both_askable() {
    // A destination names one table or names a rule for naming several. Both
    // at once is not a request the service has, and clap says so without a
    // round trip.
    let (ok, out) = combined(&[
        "ingest",
        "create",
        "--datasource-id",
        "ds_1",
        "--table",
        "orders",
        "--dest-table",
        "t",
        "--dest-table-prefix",
        "fam",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

/// `create` starts nothing. The scheduler dispatches every run, including the
/// single run of a one-time ingest — so help that promised a run id back
/// promised a field that is null by design.
#[test]
fn create_help_does_not_promise_a_run_id() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    let help = flat(&help);
    assert!(!help.contains("initial_run_id"), "{help}");
    assert!(!help.contains("runs immediately"), "{help}");
    assert!(help.contains("scheduler dispatches every run"), "{help}");
    assert!(help.contains("hotdata ingest runs"), "{help}");
}

#[test]
fn record_shape_help_lists_the_shapes() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("otel_traces"), "{help}");
    assert!(help.contains("mqtt_observations"), "{help}");
}

#[test]
fn source_accepts_a_name_or_an_id_and_is_not_doubled_up() {
    let (ok, help) = combined(&["ingest", "create", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("display name"), "{help}");
    // Two matches must be an error, and the help says so before it happens.
    assert!(help.contains("Two datasources"), "{help}");

    let (ok, out) = combined(&[
        "ingest",
        "create",
        "--datasource-id",
        "ds_1",
        "--source",
        "prod",
        "--table",
        "orders",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

#[test]
fn ingest_create_requires_a_datasource_by_either_flag() {
    let (ok, out) = combined(&["ingest", "create", "--table", "orders"]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("--datasource-id"), "{out}");
}

#[test]
fn the_selector_escape_hatch_excludes_the_shorthands_it_replaces() {
    for shorthand in [
        vec!["--table", "orders"],
        vec!["--table-path", "warehouse/orders"],
        vec!["--topic", "events"],
        vec!["--schema", "public"],
        vec!["--format", "parquet"],
        vec!["--glob", "**"],
        vec!["--raw-sql", "SELECT 1"],
        vec!["--all"],
    ] {
        let mut args = vec![
            "ingest",
            "create",
            "--datasource-id",
            "ds_1",
            "--selector",
            "{}",
        ];
        args.extend(shorthand.iter().copied());
        let (ok, out) = combined(&args);
        assert!(!ok, "{shorthand:?} should not parse: {out}");
        assert!(out.contains("cannot be used with"), "{shorthand:?}: {out}");
    }
}

#[test]
fn all_and_an_explicit_table_are_different_requests() {
    let (ok, out) = combined(&[
        "ingest",
        "create",
        "--datasource-id",
        "ds_1",
        "--all",
        "--table",
        "orders",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
}

// --- waiting is watching ------------------------------------------------------

#[test]
fn every_wait_flag_says_it_cannot_make_a_run_start_sooner() {
    // The one thing a user must not conclude from a --wait on a scheduler-driven
    // model. Each of the three surfaces has to say it where it is read.
    for (args, needle) in [
        (vec!["run", "show", "--help"], "does not make it start"),
        (vec!["ingest", "runs", "--help"], "cannot bring one forward"),
        (
            vec!["datasource", "create", "--help"],
            "cannot make anything happen sooner",
        ),
    ] {
        let (ok, help) = combined(&args);
        assert!(ok, "{help}");
        assert!(help.contains("--wait"), "{args:?}: {help}");
        assert!(
            help.contains(needle),
            "{args:?} must say '{needle}': {help}"
        );
    }
}

#[test]
fn datasource_create_offers_both_halves_of_the_wait() {
    let (ok, help) = combined(&["datasource", "create", "--help"]);
    assert!(ok, "{help}");
    assert!(help.contains("--wait"), "{help}");
    assert!(help.contains("--no-wait"), "{help}");
    // They are opposite answers to one question, not two switches.
    let (ok, out) = combined(&[
        "datasource",
        "create",
        "--family",
        "sql",
        "--config",
        "{}",
        "--wait",
        "--no-wait",
    ]);
    assert!(!ok, "should not parse: {out}");
    assert!(out.contains("cannot be used with"), "{out}");
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
