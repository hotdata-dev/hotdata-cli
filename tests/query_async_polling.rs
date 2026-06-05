//! Scenario: query_async_polling.
//!
//! The CLI's `query` command submits asynchronously (async_after_ms) and polls
//! the query run to a terminal state internally before printing — so a single
//! `hotdata query` invocation exercises the full async submit + poll + fetch
//! path. We then confirm the run and its result surface via `results list`,
//! `queries list`, and `results <id>`.
//!
//! Queries require a database scope, so we target the shared `sdkci-shared`
//! managed database (otherwise the server returns 400 "a database is required").

mod common;

#[test]
fn query_async_polling() {
    let cli = skip_if_no_creds!();
    let database_id = common::shared_database_id(&cli);

    // Submit + poll + fetch, all inside `query`.
    let result = cli.json(&["query", "SELECT 1 AS x", "-d", &database_id, "-o", "json"]);
    assert_eq!(
        result.get("row_count").and_then(|v| v.as_u64()),
        Some(1),
        "expected row_count 1: {result}"
    );
    assert_eq!(
        result.get("rows"),
        Some(&serde_json::json!([[1]])),
        "expected rows [[1]]: {result}"
    );
    let result_id = result
        .get("result_id")
        .and_then(|v| v.as_str())
        .expect("query result should expose a result_id")
        .to_string();

    // list_results — the new result is surfaced.
    let results = cli.json(&["results", "list", "-o", "json"]);
    let result_ids: Vec<&str> = results
        .as_array()
        .expect("results list -o json should be a JSON array")
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_str()))
        .collect();
    assert!(
        result_ids.contains(&result_id.as_str()),
        "result {result_id} not surfaced by results list, got {result_ids:?}"
    );

    // list_query_runs — at least one run exists after submitting a query.
    let runs = cli.json(&["queries", "list", "-o", "json"]);
    assert!(
        !runs
            .as_array()
            .expect("queries list -o json should be a JSON array")
            .is_empty(),
        "expected at least one query run after submitting a query"
    );

    // get_result — fetching by id round-trips the single row.
    let fetched = cli.json(&["results", &result_id, "-o", "json"]);
    assert_eq!(
        fetched.get("row_count").and_then(|v| v.as_u64()),
        Some(1),
        "expected row_count 1 from results <id>: {fetched}"
    );
}
