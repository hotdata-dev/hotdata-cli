//! Scenario: results_arrow.
//!
//! The CLI always fetches stored results as Arrow IPC over the wire
//! (`Accept: application/vnd.apache.arrow.stream`, see `src/query.rs`
//! `fetch_arrow_result`) and decodes them locally before rendering. So any
//! `query`/`results` invocation that prints rows exercises the Arrow round-trip
//! end to end. We verify schema + values survive both the JSON and CSV
//! renderers.
//!
//! `ORDER BY x` makes the row order deterministic — a bare UNION ALL has no
//! guaranteed order, so the assertions below would otherwise be flaky.
//!
//! Note: the CLI's `results <id>` has no offset/limit flags, so the Arrow
//! offset/limit pagination the SDK test covers is out of scope here.

mod common;

const SQL: &str = "SELECT 1 AS x, 'hello' AS msg UNION ALL SELECT 2, 'world' ORDER BY x";

#[test]
fn results_arrow() {
    let cli = skip_if_no_creds!();
    let database_id = common::shared_database_id(&cli);

    // JSON renderer: schema + values round-trip through the Arrow decode.
    let result = cli.json(&["query", SQL, "-d", &database_id, "-o", "json"]);
    assert_eq!(
        result.get("columns"),
        Some(&serde_json::json!(["x", "msg"])),
        "expected columns [x, msg]: {result}"
    );
    assert_eq!(
        result.get("rows"),
        Some(&serde_json::json!([[1, "hello"], [2, "world"]])),
        "expected rows [[1, hello], [2, world]]: {result}"
    );

    // CSV renderer: the same Arrow data, formatted as CSV.
    let output = cli.run(&["query", SQL, "-d", &database_id, "-o", "csv"]);
    common::assert_success(&output, &["query", "<sql>", "-d", "<db>", "-o", "csv"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["x,msg", "1,hello", "2,world"],
        "unexpected CSV output:\n{stdout}"
    );
}
