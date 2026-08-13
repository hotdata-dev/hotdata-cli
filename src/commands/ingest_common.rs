//! Presentation and argument helpers shared by the three ingest-side command
//! groups: `hotdata datasource`, `hotdata ingest`, and `hotdata run`.
//!
//! They were one group before the datasource/ingest/run split, and the output
//! conventions must not drift now that they are three: one `render`, one
//! spinner wrapper, one detail-view label width, one `@file.json` parser, one
//! date cell. Each group keeps its own request-building logic.

use crate::util;

/// Detail-view label column. Wide enough for the longest label in the group
/// (`config version:`), so the values in `datasource show` / `ingest show` /
/// `run show` line up with each other.
const LABEL_WIDTH: usize = 16;

/// Render a value for `-o json|yaml`, or fall through to the human branch.
/// One definition so the json-println / yaml-print convention cannot drift
/// between the (many) commands that support all three formats.
pub fn render<T: serde::Serialize>(output: &str, value: &T, human: impl FnOnce()) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(value).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(value).unwrap()),
        _ => human(),
    }
}

/// Run `f` under a spinner, clearing it before either returning the value or
/// printing the error — the clear-before-exit invariant lives here instead of
/// being copy-discipline at every call site.
pub fn with_spinner<T>(
    msg: &str,
    f: impl FnOnce() -> Result<T, crate::client::ingest::IngestError>,
) -> T {
    let spinner = util::spinner(msg);
    match f() {
        Ok(v) => {
            spinner.finish_and_clear();
            v
        }
        Err(e) => {
            spinner.finish_and_clear();
            e.exit()
        }
    }
}

/// Fatal command-layer error: the message the user can act on, then exit 1.
pub fn fail(msg: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!("{}", format!("error: {msg}").red());
    std::process::exit(1);
}

/// One line of a detail view: a dark-grey aligned label, then the value.
pub fn field(label: &str, value: &str) {
    use crossterm::style::Stylize;
    println!("{}{}", format!("{label:<LABEL_WIDTH$}").dark_grey(), value);
}

/// A closing hint under a detail view or ack — always dark grey, always the
/// next command to run.
pub fn hint(msg: &str) {
    use crossterm::style::Stylize;
    println!("{}", msg.dark_grey());
}

/// An empty-listing notice. Goes to **stderr** so `-o table | …` pipelines see
/// an empty stdout rather than prose.
pub fn empty_notice(msg: &str) {
    use crossterm::style::Stylize;
    eprintln!("{}", msg.dark_grey());
}

/// CREATED/STARTED cell for the listing tables — util::format_date, aligned
/// with every other table in the CLI ("2026-07-08 10:12").
pub fn date_cell(ts: Option<&str>) -> String {
    ts.map(util::format_date).unwrap_or_else(|| "-".into())
}

/// Table cell for an optional string: missing renders as "-", never blank.
pub fn cell(v: Option<&str>) -> String {
    v.filter(|s| !s.trim().is_empty())
        .unwrap_or("-")
        .to_string()
}

/// Only one flag per invocation may read `@-`; the second would block forever
/// on an already-drained stdin.
static STDIN_CONSUMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Parse a JSON payload flag: inline JSON, `@file.json`, or `@-` for stdin.
///
/// `flag` names the option in any error, because `--selector`, `--destination`,
/// `--schedule`, `--config`, and `--credentials` all come through here and
/// "invalid JSON" alone would not say which one.
pub fn parse_json_arg(flag: &str, arg: &str) -> serde_json::Value {
    use std::io::Read;
    use std::sync::atomic::Ordering;

    let raw = if arg == "@-" {
        if STDIN_CONSUMED.swap(true, Ordering::SeqCst) {
            fail(&format!(
                "{flag} cannot also read @- — stdin was already consumed by another flag; \
                 pass a file or inline JSON"
            ));
        }
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .unwrap_or_else(|e| fail(&format!("{flag} reading stdin: {e}")));
        s
    } else if let Some(path) = arg.strip_prefix('@') {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| fail(&format!("{flag} reading {path}: {e}")))
    } else {
        arg.to_string()
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| fail(&format!("{flag} invalid JSON: {e}")))
}

/// Parse a human duration into seconds: `45s`, `5m`, `2h`, `1d`, or a bare
/// number of seconds. Pure and total so `--every` is unit-testable and the
/// error text is the CLI's, not a server 422's.
pub fn parse_duration(s: &str) -> Result<u64, String> {
    let t = s.trim();
    if t.is_empty() {
        return Err("empty duration — use e.g. 30s, 5m, 2h, 1d".into());
    }
    let (digits, unit) = match t.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => (&t[..i], &t[i..]),
        None => (t, "s"),
    };
    let n: u64 = digits
        .parse()
        .map_err(|_| format!("'{s}' is not a duration — use e.g. 30s, 5m, 2h, 1d"))?;
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "s" | "sec" | "secs" => 1,
        "m" | "min" | "mins" => 60,
        "h" | "hr" | "hrs" => 3_600,
        "d" | "day" | "days" => 86_400,
        other => {
            return Err(format!(
                "unknown duration unit '{other}' — use s, m, h, or d"
            ));
        }
    };
    let seconds = n
        .checked_mul(multiplier)
        .ok_or_else(|| format!("'{s}' is too large a duration"))?;
    if seconds == 0 {
        return Err("a schedule interval must be greater than zero".into());
    }
    Ok(seconds)
}

/// The `next_run_at` value for a `--next` argument. `now` is passed through
/// literally — the API accepts it as a keyword, and it is the documented way
/// to bring the next scheduled run forward without creating an extra one.
pub fn parse_next_run_at(s: &str) -> String {
    let t = s.trim();
    if t.eq_ignore_ascii_case("now") {
        "now".to_string()
    } else {
        t.to_string()
    }
}

/// Compact one-line summary of an ingest's destination for a listing cell:
/// `db_456.public.orders_raw`. Falls back to whatever parts exist.
///
/// Takes the nested `destination` object, which is the only place the wire
/// carries it: the service materialises database/schema/table into their own
/// columns for the ownership index, but the ingest views return the document
/// the create request sent. Reading top-level `destination_*` fields instead
/// renders every real response as `-`.
pub fn destination_cell(destination: Option<&serde_json::Value>) -> String {
    let parts: Vec<String> = ["database_id", "schema", "table"]
        .into_iter()
        .filter_map(|key| {
            destination
                .and_then(|d| d.get(key))
                .and_then(serde_json::Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
        })
        .collect();
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join(".")
    }
}

/// Compact one-line summary of a schedule for a listing cell: `every 5m` (plus
/// the next dispatch when the server reported one).
pub fn schedule_cell(
    schedule: Option<&serde_json::Value>,
    next_attempt_at: Option<&str>,
) -> String {
    let interval = schedule
        .and_then(|s| s.get("interval_seconds"))
        .and_then(serde_json::Value::as_u64)
        .map(format_duration);
    match (interval, next_attempt_at) {
        (Some(i), Some(next)) => format!("every {i} (next {})", util::format_date(next)),
        (Some(i), None) => format!("every {i}"),
        (None, Some(next)) => format!("next {}", util::format_date(next)),
        (None, None) => "-".into(),
    }
}

/// Seconds back to the compact form `--every` accepts, so a listing reads the
/// way the flag was written.
pub fn format_duration(seconds: u64) -> String {
    for (unit, size) in [("d", 86_400u64), ("h", 3_600), ("m", 60)] {
        if seconds.is_multiple_of(size) && seconds >= size {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

// --- run status vocabulary -------------------------------------------------

/// A run's `status` is a CLOSED set: queued | running | succeeded | failed |
/// cancelled. Anything else the service reports is a finer in-flight stage —
/// presented as `running` with the raw value demoted to the stage slot, so a
/// new server-side stage name can never look like a new terminal state.
pub fn normalize_run_status(raw: &str) -> (&'static str, Option<&str>) {
    match raw {
        "queued" => ("queued", None),
        "running" => ("running", None),
        "succeeded" => ("succeeded", None),
        "failed" => ("failed", None),
        "cancelled" => ("cancelled", None),
        stage => ("running", Some(stage)),
    }
}

/// The (status, stage) pair for a run row: the server's `stage` field wins, a
/// stage-shaped `status` is the fallback.
pub fn presented_run_status(status: &str, stage: Option<&str>) -> (String, Option<String>) {
    let (normalized, fallback) = normalize_run_status(status);
    (
        normalized.to_string(),
        stage.map(str::to_string).or(fallback.map(str::to_string)),
    )
}

/// STATUS cell for the run tables/details: the normalized status, with the
/// in-flight stage in parentheses when there is one.
pub fn run_status_cell(status: &str, stage: Option<&str>) -> String {
    let (normalized, stage) = presented_run_status(status, stage);
    let colored = util::color_status(&normalized);
    match stage {
        Some(s) => format!("{colored} ({s})"),
        None => colored,
    }
}

/// Exit code for `hotdata run show`, mirroring `query status`: 0 succeeded,
/// 1 failed/cancelled, 2 still in flight (queued/running).
pub fn run_exit_code(status: &str) -> i32 {
    match normalize_run_status(status).0 {
        "succeeded" => 0,
        "failed" | "cancelled" => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_every_supported_unit() {
        assert_eq!(parse_duration("30s").unwrap(), 30);
        assert_eq!(parse_duration("5m").unwrap(), 300);
        assert_eq!(parse_duration("2h").unwrap(), 7_200);
        assert_eq!(parse_duration("1d").unwrap(), 86_400);
        // A bare number is seconds, and surrounding space is tolerated.
        assert_eq!(parse_duration("90").unwrap(), 90);
        assert_eq!(parse_duration(" 5m ").unwrap(), 300);
        // Long forms and case.
        assert_eq!(parse_duration("15MIN").unwrap(), 900);
        assert_eq!(parse_duration("3Hrs").unwrap(), 10_800);
    }

    #[test]
    fn durations_reject_what_the_scheduler_cannot_use() {
        for bad in ["", "soon", "5w", "-5m", "m5"] {
            assert!(parse_duration(bad).is_err(), "{bad} should not parse");
        }
        // Zero would mean "dispatch continuously" — reject at the CLI rather
        // than let the server decide.
        assert!(parse_duration("0s").unwrap_err().contains("greater than"));
        assert!(parse_duration("0").unwrap_err().contains("greater than"));
    }

    #[test]
    fn durations_round_trip_through_the_display_form() {
        for s in ["30s", "5m", "2h", "1d"] {
            let secs = parse_duration(s).unwrap();
            assert_eq!(format_duration(secs), s, "{s} did not round-trip");
        }
        // Non-round values fall back to seconds rather than lying.
        assert_eq!(format_duration(90), "90s");
        assert_eq!(format_duration(3_601), "3601s");
    }

    #[test]
    fn next_run_at_passes_now_and_timestamps_through() {
        assert_eq!(parse_next_run_at("now"), "now");
        assert_eq!(parse_next_run_at("NOW"), "now");
        assert_eq!(
            parse_next_run_at("2026-08-13T12:00:00Z"),
            "2026-08-13T12:00:00Z"
        );
    }

    #[test]
    fn parse_json_arg_accepts_inline_json() {
        let v = parse_json_arg("--config", r#"{"dialect": "postgres"}"#);
        assert_eq!(v["dialect"], "postgres");
    }

    #[test]
    fn parse_json_arg_reads_an_at_file() {
        let dir = std::env::temp_dir().join("hotdata-cli-test-parse-json-arg");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("selector.json");
        std::fs::write(&path, r#"{"mode": "tables"}"#).unwrap();
        let v = parse_json_arg("--selector", &format!("@{}", path.display()));
        assert_eq!(v["mode"], "tables");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn cells_render_missing_values_as_a_dash() {
        assert_eq!(cell(None), "-");
        assert_eq!(cell(Some("  ")), "-");
        assert_eq!(cell(Some("ds_1")), "ds_1");
        assert_eq!(
            date_cell(Some("2026-07-08T10:12:00+00:00")),
            "2026-07-08 10:12"
        );
        assert_eq!(date_cell(None), "-");
    }

    #[test]
    fn destination_cell_joins_the_parts_it_has() {
        let full = serde_json::json!({
            "database_id": "db_456", "schema": "public",
            "table": "orders_raw", "write_mode": "replace"
        });
        assert_eq!(destination_cell(Some(&full)), "db_456.public.orders_raw");
        // A destination the server defaulted the schema on still reads.
        let partial = serde_json::json!({"database_id": "db_456", "table": "t"});
        assert_eq!(destination_cell(Some(&partial)), "db_456.t");
        assert_eq!(destination_cell(None), "-");
        assert_eq!(destination_cell(Some(&serde_json::json!({}))), "-");
    }

    #[test]
    fn run_statuses_normalize_to_the_closed_set() {
        assert_eq!(normalize_run_status("succeeded"), ("succeeded", None));
        assert_eq!(normalize_run_status("queued"), ("queued", None));
        assert_eq!(normalize_run_status("cancelled"), ("cancelled", None));
        // A stage reported through `status` is presented as running.
        assert_eq!(
            normalize_run_status("extracting"),
            ("running", Some("extracting"))
        );
        // The old vocabulary is NOT terminal any more: `done`/`pending` are
        // not run statuses, so they must not read as success.
        assert_eq!(normalize_run_status("done"), ("running", Some("done")));
        assert_eq!(
            normalize_run_status("pending"),
            ("running", Some("pending"))
        );
        // A server-provided stage field wins over the fallback.
        assert_eq!(
            presented_run_status("running", Some("loading")),
            ("running".into(), Some("loading".into()))
        );
    }

    #[test]
    fn run_exit_codes_close_over_stage_states() {
        assert_eq!(run_exit_code("succeeded"), 0);
        assert_eq!(run_exit_code("failed"), 1);
        assert_eq!(run_exit_code("cancelled"), 1);
        assert_eq!(run_exit_code("queued"), 2);
        assert_eq!(run_exit_code("running"), 2);
        assert_eq!(run_exit_code("loading"), 2);
    }

    #[test]
    fn schedule_cell_reads_back_as_the_every_flag() {
        let s = serde_json::json!({"interval_seconds": 300});
        assert_eq!(schedule_cell(Some(&s), None), "every 5m");
        assert_eq!(
            schedule_cell(Some(&s), Some("2026-08-13T12:00:00+00:00")),
            "every 5m (next 2026-08-13 12:00)"
        );
        // A one-time ingest has neither.
        assert_eq!(schedule_cell(None, None), "-");
    }
}
