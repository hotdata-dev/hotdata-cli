use crate::sdk::{Api, ApiError};
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct QueryResponse {
    pub result_id: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// Rows actually carried here (`rows.len()`). For a complete result this is
    /// the whole result; for an incomplete preview (`truncated`) it's a bounded
    /// subset.
    pub row_count: u64,
    /// Grand total rows in the full result when the server reported it. `None`
    /// when unknown — e.g. a truncated result whose persistence never started.
    pub total_row_count: Option<u64>,
    /// True when `rows` is an *incomplete* subset the CLI could not complete:
    /// the server truncated the result and it was either never persisted (no
    /// `result_id`) or the follow-up fetch failed. A truncated result the CLI
    /// successfully follows to the full set is `false` — the rows held are the
    /// whole result. Drives the fail-closed exit in [`print_result`].
    pub truncated: bool,
    pub execution_time_ms: Option<u64>,
    pub warning: Option<String>,
}

/// Exit code emitted when the CLI prints an incomplete preview (a truncated
/// result it could not complete). Distinct from the generic `1`/`2` so pipelines
/// can tell "partial data" apart from a hard failure and break rather than
/// silently ingest a subset.
pub const EXIT_INCOMPLETE_RESULT: i32 = 3;

/// Convert the SDK's inline `QueryResponse` (200 path) into the CLI's display
/// model. The async path decodes Arrow instead (see `fetch_arrow_result`).
///
/// `row_count` is derived from `rows.len()` — the rows actually carried in this
/// body — rather than the deprecated SDK `row_count` field. This path only ever
/// renders the rows it holds: a non-truncated response carries the whole result,
/// and the truncated-without-`result_id` fallback keeps just the preview (with a
/// warning). A truncated result that *can* be fetched never reaches here — it's
/// followed to the full set via Arrow in `resolve_inline`. So counting the held
/// rows can never overstate or understate what the user sees.
///
/// `truncated` and `total_row_count` are carried through verbatim so structured
/// output (`--output json`) exposes whether the rows are a partial preview and,
/// when known, the grand total. For a complete (non-truncated) result the server
/// reports `total_row_count == preview`; if it's absent we backfill it from the
/// held rows, which for a complete result is the whole truth.
fn query_response_from_sdk(resp: hotdata::models::QueryResponse) -> QueryResponse {
    let row_count = resp.rows.len() as u64;
    // A negative total is impossible per the API contract; treat one as absent
    // rather than clamping to 0, which would emit a contradictory
    // `preview_row_count > total_row_count`.
    let total_row_count = resp
        .total_row_count
        .flatten()
        .and_then(|t| u64::try_from(t).ok())
        .or(if resp.truncated {
            None
        } else {
            Some(row_count)
        });
    QueryResponse {
        result_id: resp.result_id.flatten(),
        columns: resp.columns,
        row_count,
        total_row_count,
        truncated: resp.truncated,
        rows: resp.rows,
        execution_time_ms: Some(resp.execution_time_ms.max(0) as u64),
        warning: resp.warning.flatten(),
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let (formatted, count) = crate::table::truncate_array(arr);
            match count {
                Some(n) => format!("{formatted} ({n} items)"),
                None => formatted,
            }
        }
        Value::Object(_) => v.to_string(),
    }
}

/// Convert one cell of an Arrow array to a `serde_json::Value`.
fn arrow_cell(col: &dyn arrow::array::Array, row: usize) -> Value {
    use arrow::array::*;
    use arrow::datatypes::DataType::*;
    use serde_json::Number;

    if col.is_null(row) {
        return Value::Null;
    }

    match col.data_type() {
        Boolean => Value::Bool(
            col.as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .value(row),
        ),
        Int8 => Value::Number(
            col.as_any()
                .downcast_ref::<Int8Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        Int16 => Value::Number(
            col.as_any()
                .downcast_ref::<Int16Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        Int32 => Value::Number(
            col.as_any()
                .downcast_ref::<Int32Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        Int64 => Value::Number(
            col.as_any()
                .downcast_ref::<Int64Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        UInt8 => Value::Number(
            col.as_any()
                .downcast_ref::<UInt8Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        UInt16 => Value::Number(
            col.as_any()
                .downcast_ref::<UInt16Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        UInt32 => Value::Number(
            col.as_any()
                .downcast_ref::<UInt32Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        UInt64 => Value::Number(
            col.as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .value(row)
                .into(),
        ),
        Float32 => {
            let v = col
                .as_any()
                .downcast_ref::<Float32Array>()
                .unwrap()
                .value(row) as f64;
            Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        Float64 => {
            let v = col
                .as_any()
                .downcast_ref::<Float64Array>()
                .unwrap()
                .value(row);
            Number::from_f64(v)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        Utf8 => Value::String(
            col.as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(row)
                .to_owned(),
        ),
        LargeUtf8 => Value::String(
            col.as_any()
                .downcast_ref::<LargeStringArray>()
                .unwrap()
                .value(row)
                .to_owned(),
        ),
        // Dates, timestamps, decimals, etc. — format via Arrow's display helper.
        _ => {
            use arrow::util::display::{ArrayFormatter, FormatOptions};
            let opts = FormatOptions::default();
            ArrayFormatter::try_new(col, &opts)
                .map(|f| Value::String(f.value(row).to_string()))
                .unwrap_or(Value::Null)
        }
    }
}

/// Convert an SDK-decoded [`hotdata::ArrowResult`] into a `QueryResponse`
/// suitable for display.
fn arrow_result_to_query_response(
    result: hotdata::ArrowResult,
    result_id: String,
) -> QueryResponse {
    let columns: Vec<String> = result
        .schema
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut rows: Vec<Vec<Value>> = Vec::new();

    for batch in &result.batches {
        for row in 0..batch.num_rows() {
            rows.push(
                (0..batch.num_columns())
                    .map(|c| arrow_cell(batch.column(c).as_ref(), row))
                    .collect(),
            );
        }
    }

    let row_count = rows.len() as u64;
    // The fetched Arrow result is the full persisted result, so it's complete:
    // `truncated` is false and the total is the authoritative `X-Total-Row-Count`
    // the SDK parsed (falling back to the rows we hold).
    let total_row_count = result
        .total_row_count
        .and_then(|t| u64::try_from(t).ok())
        .or(Some(row_count));
    QueryResponse {
        result_id: Some(result_id),
        columns,
        rows,
        row_count,
        total_row_count,
        truncated: false,
        execution_time_ms: None,
        warning: None,
    }
}

/// Fetch `/results/{result_id}` as Arrow and return a `QueryResponse`, returning
/// the error instead of exiting.
///
/// Both transport and decode are owned by the SDK's `get_result_arrow` (via the
/// [`Api::get_result_arrow`] seam), so the CLI shares one `arrow` major version
/// with the SDK.
///
/// The fallible form lets callers that hold a fallback (e.g. an inline preview)
/// degrade gracefully rather than terminate the process; [`fetch_arrow_result`]
/// is the exiting wrapper for callers with nothing else to show.
pub(crate) fn try_fetch_arrow_result(
    api: &Api,
    result_id: &str,
) -> Result<QueryResponse, ApiError> {
    let result = api.get_result_arrow(result_id)?;
    Ok(arrow_result_to_query_response(result, result_id.to_owned()))
}

/// Fetch `/results/{result_id}` as Arrow, exiting the process on failure.
pub(crate) fn fetch_arrow_result(api: &Api, result_id: &str) -> QueryResponse {
    try_fetch_arrow_result(api, result_id).unwrap_or_else(|e| e.exit())
}

/// Resolve an inline (HTTP 200) query response for display.
///
/// A non-truncated response carries the whole result in `rows`, so it's shown
/// as-is. A truncated one (#640) carries only a bounded preview — the full set
/// is persisted under `result_id` — so follow it to the full result via Arrow,
/// the same path the async (202) branch uses. Truncation rides on result *size*
/// while `async_after_ms` gates on *time*, so a fast-completing but large query
/// returns a truncated inline 200; without this follow the CLI would silently
/// print only the preview rows.
///
/// If a truncated response has no `result_id` (persistence could not be
/// initiated — see the SDK's `warning` field), or the follow-up Arrow fetch
/// fails, the full result is unreachable. Rather than exiting and discarding the
/// preview the inline body already carries, fall back to that preview, mark it
/// incomplete (`truncated`), and surface a warning. `print_result` then fails
/// closed (non-zero exit) so the partial data can't be silently consumed.
fn resolve_inline(api: &Api, resp: hotdata::models::QueryResponse) -> QueryResponse {
    if !resp.truncated {
        return query_response_from_sdk(resp);
    }
    match resp.result_id.clone().flatten() {
        Some(result_id) => match try_fetch_arrow_result(api, &result_id) {
            // The Arrow fetch returns only schema + rows; carry the query-level
            // warning and execution time the inline response reported, which
            // `arrow_result_to_query_response` otherwise hardcodes to None.
            Ok(mut full) => {
                full.warning = resp.warning.flatten();
                full.execution_time_ms = Some(resp.execution_time_ms.max(0) as u64);
                full
            }
            // The full result is persisted but the follow-up fetch failed (e.g.
            // transport error, persistence still in progress). Degrade to the
            // preview instead of hard-exiting and losing the rows in hand.
            Err(e) => incomplete_preview(
                resp,
                &format!("could not fetch full result ({})", e.message()),
            ),
        },
        None => incomplete_preview(resp, "full result unavailable (persistence not initiated)"),
    }
}

/// Keep a truncated inline response's preview rows, mark it incomplete, and fold
/// `note` into the warning (preserving any SDK-provided warning that explains
/// *why* the full result is unreachable).
fn incomplete_preview(resp: hotdata::models::QueryResponse, note: &str) -> QueryResponse {
    let mut preview = query_response_from_sdk(resp);
    let note = format!("result truncated to a preview; {note}");
    preview.warning = Some(match preview.warning {
        Some(w) => format!("{w}; {note}"),
        None => note,
    });
    preview
}

pub fn execute(sql: &str, workspace_id: &str, database: Option<&str>, format: &str) {
    let api = Api::new(Some(workspace_id));

    // Scope to the explicit --database flag, else the active database resolved
    // at construction (HOTDATA_DATABASE / current database). submit_query sends
    // it as the X-Database-Id header.
    let database = database.or(api.database_id());

    let mut request = hotdata::models::QueryRequest::new(sql.to_string());
    request.r#async = Some(true);
    request.async_after_ms = Some(Some(1000));

    let outcome = crate::sdk::block_with_wakeup(
        &api,
        "running query...",
        api.client().submit_query(request, database),
    )
    .unwrap_or_else(|e| e.exit());

    let async_resp = match outcome {
        // Completed within async_after_ms — inline results. A large result can
        // come back truncated to a preview even on this fast path, so follow it
        // to the full set (resolve_inline) rather than printing the preview.
        hotdata::QueryOutcome::Inline(resp) => {
            print_result(&resolve_inline(&api, resp), format);
            return;
        }
        // Still running — poll the query run, then fetch the result as Arrow.
        hotdata::QueryOutcome::Submitted(async_resp) => async_resp,
        // QueryOutcome is #[non_exhaustive]; guard against future variants.
        _ => {
            eprintln!("unexpected query response from server");
            std::process::exit(1);
        }
    };

    let run_id = &async_resp.query_run_id;
    let spinner = crate::util::spinner("waiting for query...");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);

    loop {
        // Drive the poll loop ourselves to preserve the 5-minute deadline and
        // 500ms cadence (NOT the SDK's PollConfig defaults).
        let run =
            crate::sdk::block(api.client().query_runs().get(run_id)).unwrap_or_else(|e| e.exit());
        match run.status.as_str() {
            "succeeded" => {
                spinner.finish_and_clear();
                match run.result_id.flatten() {
                    Some(ref result_id) => {
                        let result = fetch_arrow_result(&api, result_id);
                        print_result(&result, format);
                    }
                    None => {
                        use crossterm::style::Stylize;
                        println!("{}", "Query succeeded but no result available.".yellow());
                    }
                }
                return;
            }
            "failed" => {
                spinner.finish_and_clear();
                use crossterm::style::Stylize;
                let err = run
                    .error_message
                    .flatten()
                    .unwrap_or_else(|| "unknown error".to_string());
                eprintln!("{}", format!("query failed: {err}").red());
                std::process::exit(1);
            }
            "running" | "queued" | "pending" => {}
            status => {
                spinner.finish_and_clear();
                use crossterm::style::Stylize;
                eprintln!("{}", format!("query status: {status}").yellow());
                eprintln!(
                    "{}",
                    format!("Check status with: hotdata query status {run_id}").dark_grey()
                );
                std::process::exit(2);
            }
        }
        if std::time::Instant::now() > deadline {
            spinner.finish_and_clear();
            use crossterm::style::Stylize;
            eprintln!("{}", "query timed out after 5 minutes".red());
            eprintln!(
                "{}",
                format!("Check status with: hotdata query status {run_id}").dark_grey()
            );
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Poll a query run by ID. If succeeded and has a result_id, fetch and display the result.
pub fn poll(query_run_id: &str, workspace_id: &str, format: &str) {
    let api = Api::new(Some(workspace_id));

    let run =
        crate::sdk::block(api.client().query_runs().get(query_run_id)).unwrap_or_else(|e| e.exit());

    match run.status.as_str() {
        "succeeded" => match run.result_id.flatten() {
            Some(ref result_id) => {
                let result = fetch_arrow_result(&api, result_id);
                print_result(&result, format);
            }
            None => {
                use crossterm::style::Stylize;
                println!("{}", "Query succeeded but no result available.".yellow());
            }
        },
        "failed" => {
            use crossterm::style::Stylize;
            let err = run
                .error_message
                .flatten()
                .unwrap_or_else(|| "unknown error".to_string());
            eprintln!("{}", format!("query failed: {err}").red());
            std::process::exit(1);
        }
        status => {
            use crossterm::style::Stylize;
            eprintln!("{}", format!("query status: {status}").yellow());
            eprintln!("query_run_id: {}", run.id);
            eprintln!(
                "{}",
                format!("Poll again with: hotdata query status {}", run.id).dark_grey()
            );
            std::process::exit(2);
        }
    }
}

/// Build the `--output json` body for a result.
///
/// `row_count` (rows in this body) is kept for backward compatibility, but the
/// truncation truth is carried explicitly: `truncated`, `preview_row_count`
/// (rows held), and `total_row_count` (grand total, `null` when unknown). A
/// JSON consumer can detect a partial result from these rather than having to
/// notice a stderr-only warning.
fn result_json(result: &QueryResponse) -> Value {
    serde_json::json!({
        "result_id": result.result_id,
        "columns": result.columns,
        "rows": result.rows,
        "row_count": result.row_count,
        "preview_row_count": result.row_count,
        "total_row_count": result.total_row_count,
        "truncated": result.truncated,
        "execution_time_ms": result.execution_time_ms,
    })
}

/// Process exit code after rendering a result: [`EXIT_INCOMPLETE_RESULT`] when
/// the rows are an incomplete preview (fail closed so pipelines break), else `0`.
fn result_exit_code(result: &QueryResponse) -> i32 {
    if result.truncated {
        EXIT_INCOMPLETE_RESULT
    } else {
        0
    }
}

/// The unstyled summary line printed under a `table` result.
///
/// A complete result reads `N rows (time) [result-id]`. An incomplete preview is
/// loud — `N of TOTAL rows — INCOMPLETE PREVIEW (...)` — with `?` standing in for
/// a total the server didn't report. The caller colours it (red vs grey).
fn table_footer(result: &QueryResponse) -> String {
    let id_part = result
        .result_id
        .as_deref()
        .map(|id| format!(" [result-id: {id}]"))
        .unwrap_or_default();
    let time_part = match result.execution_time_ms {
        Some(ms) => format!("{ms} ms"),
        None => "\u{2014}".to_string(), // em dash
    };
    if result.truncated {
        let total = result
            .total_row_count
            .map(|t| t.to_string())
            .unwrap_or_else(|| "?".to_string());
        format!(
            "{} of {} rows — INCOMPLETE PREVIEW ({}){}",
            result.row_count, total, time_part, id_part
        )
    } else {
        format!(
            "{} row{} ({}){}",
            result.row_count,
            if result.row_count == 1 { "" } else { "s" },
            time_part,
            id_part
        )
    }
}

pub fn print_result(result: &QueryResponse, format: &str) {
    if let Some(ref warning) = result.warning {
        eprintln!("warning: {warning}");
    }

    match format {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result_json(result)).unwrap()
            );
        }
        "csv" => {
            println!("{}", result.columns.join(","));
            for row in &result.rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|v| {
                        let s = value_to_string(v);
                        if s.contains(',') || s.contains('"') || s.contains('\n') {
                            format!("\"{}\"", s.replace('"', "\"\""))
                        } else {
                            s
                        }
                    })
                    .collect();
                println!("{}", cells.join(","));
            }
        }
        "table" => {
            crate::table::print_json(&result.columns, &result.rows);
            use crossterm::style::Stylize;
            let footer = table_footer(result);
            // Loud (red) when the preview is incomplete so it can't be mistaken
            // for a complete result; quiet (grey) otherwise.
            if result.truncated {
                eprintln!("\n{}", footer.red());
            } else {
                eprintln!("\n{}", footer.dark_grey());
            }
        }
        _ => unreachable!(),
    }

    // Fail closed: an incomplete preview was just printed (with a stderr
    // warning). Exit non-zero so a pipeline consuming the output breaks rather
    // than silently ingesting a subset of the result as if it were complete.
    let code = result_exit_code(result);
    if code != 0 {
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::Api;
    use std::sync::Arc;

    /// A truncated inline 200: one preview row standing in for a larger result.
    /// `result_id` uses the wire double-option (`Some(None)` = field present but
    /// null, i.e. persistence not initiated).
    fn truncated_preview(result_id: Option<&str>) -> hotdata::models::QueryResponse {
        let mut resp = hotdata::models::QueryResponse::new(
            vec!["id".to_string()],           // columns
            5,                                // execution_time_ms
            vec![false],                      // nullable
            1,                                // preview_row_count
            "qrun_1".to_string(),             // query_run_id
            1,                                // row_count (deprecated, == preview)
            vec![vec![serde_json::json!(1)]], // rows (preview only)
            true,                             // truncated
        );
        resp.result_id = Some(result_id.map(|s| s.to_string()));
        resp
    }

    #[test]
    fn resolve_inline_follows_truncated_result_to_full_arrow() {
        use arrow::array::{Int64Array, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::ipc::writer::StreamWriter;

        // Full result has 3 rows — more than the 1-row inline preview.
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let mut ipc: Vec<u8> = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut ipc, &schema).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }

        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/vnd.apache.arrow.stream")
            .with_body(ipc)
            .create();

        // The inline response carries a query-level warning and execution time
        // (execution_time_ms=5 from `truncated_preview`) that must survive the
        // Arrow follow, which otherwise hardcodes them to None.
        let mut resp = truncated_preview(Some("res_1"));
        resp.warning = Some(Some("approximate aggregate".to_string()));

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let resolved = resolve_inline(&api, resp);

        // Followed the truncated preview to the full 3-row result.
        assert_eq!(resolved.row_count, 3);
        assert_eq!(resolved.rows.len(), 3);
        assert_eq!(resolved.result_id.as_deref(), Some("res_1"));
        // The held rows are now the whole result: complete, not a preview.
        assert!(!resolved.truncated);
        assert_eq!(resolved.total_row_count, Some(3));
        // Inline warning + timing carried through, not dropped by the fetch.
        assert_eq!(resolved.warning.as_deref(), Some("approximate aggregate"));
        assert_eq!(resolved.execution_time_ms, Some(5));
        m.assert();
    }

    #[test]
    fn resolve_inline_falls_back_to_preview_when_follow_fetch_fails() {
        // Truncated with a result_id, but the follow-up Arrow fetch fails (500).
        // The CLI must NOT hard-exit (which would also discard the preview it
        // already holds) — it degrades to the preview, marks it incomplete, and
        // explains. A returning call here is itself the assertion that no exit
        // happened.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .with_status(500)
            .with_body("boom")
            .create();

        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));
        let resolved = resolve_inline(&api, truncated_preview(Some("res_1")));

        // Preview kept, flagged incomplete so print_result fails closed.
        assert!(resolved.truncated);
        assert_eq!(resolved.row_count, 1);
        assert_eq!(resolved.rows.len(), 1);
        // The fixture reports no total, so the table renders "1 of ? rows".
        assert_eq!(resolved.total_row_count, None);
        let warning = resolved.warning.as_deref().unwrap_or("");
        assert!(warning.contains("truncated"), "warning: {warning:?}");
        assert!(
            warning.contains("could not fetch full result"),
            "warning: {warning:?}"
        );
        m.assert();
    }

    #[test]
    fn resolve_inline_returns_untruncated_preview_without_fetching() {
        // truncated=false short-circuits before any network call; point the Api
        // at a server with no mocks so an erroneous fetch would fail loudly.
        let server = mockito::Server::new();
        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));

        let mut resp = hotdata::models::QueryResponse::new(
            vec!["x".to_string()],
            5,
            vec![false],
            2,
            "qrun_2".to_string(),
            2,
            vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
            false, // not truncated
        );
        resp.result_id = Some(Some("res_2".to_string()));

        let resolved = resolve_inline(&api, resp);
        assert_eq!(resolved.row_count, 2);
        assert_eq!(
            resolved.rows,
            vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]]
        );
        assert_eq!(resolved.result_id.as_deref(), Some("res_2"));
        // Complete result: not a preview, total backfilled from held rows.
        assert!(!resolved.truncated);
        assert_eq!(resolved.total_row_count, Some(2));
    }

    #[test]
    fn resolve_inline_truncated_without_result_id_warns_and_keeps_preview() {
        // Truncated but persistence never started (result_id is null): the full
        // result is unfetchable, so keep the preview and surface a warning.
        let server = mockito::Server::new();
        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));

        // The server reports the grand total even though it couldn't persist;
        // it must survive onto the preview so structured output exposes it.
        let mut resp = truncated_preview(None);
        resp.total_row_count = Some(Some(100));

        let resolved = resolve_inline(&api, resp);
        assert!(resolved.truncated);
        assert_eq!(resolved.row_count, 1);
        assert_eq!(resolved.rows.len(), 1);
        assert_eq!(resolved.total_row_count, Some(100));
        assert!(
            resolved
                .warning
                .as_deref()
                .unwrap_or("")
                .contains("truncated")
        );
    }

    #[test]
    fn result_json_exposes_truncation_for_incomplete_preview() {
        // A JSON consumer must be able to detect a partial result from the body
        // alone — not only from a stderr warning. truncated=true, preview <
        // total, and both counts present.
        let result = QueryResponse {
            result_id: None,
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            total_row_count: Some(100),
            truncated: true,
            execution_time_ms: Some(5),
            warning: Some("result truncated to a preview".to_string()),
        };
        let json = result_json(&result);
        assert_eq!(json["truncated"], serde_json::json!(true));
        assert_eq!(json["preview_row_count"], serde_json::json!(1));
        assert_eq!(json["row_count"], serde_json::json!(1));
        assert_eq!(json["total_row_count"], serde_json::json!(100));
    }

    #[test]
    fn result_json_marks_complete_result_not_truncated() {
        let result = QueryResponse {
            result_id: Some("res_9".to_string()),
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
            row_count: 2,
            total_row_count: Some(2),
            truncated: false,
            execution_time_ms: Some(5),
            warning: None,
        };
        let json = result_json(&result);
        assert_eq!(json["truncated"], serde_json::json!(false));
        assert_eq!(json["preview_row_count"], serde_json::json!(2));
        assert_eq!(json["total_row_count"], serde_json::json!(2));
        // A complete result exits 0 — pipelines proceed.
        assert_eq!(result_exit_code(&result), 0);
    }

    #[test]
    fn incomplete_preview_fails_closed_with_distinct_exit_code() {
        // The fail-closed contract: an incomplete preview maps to a non-zero,
        // non-generic exit code so a pipeline breaks instead of ingesting a
        // partial result that exited 0.
        let result = QueryResponse {
            result_id: None,
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            total_row_count: Some(100),
            truncated: true,
            execution_time_ms: Some(5),
            warning: Some("result truncated to a preview".to_string()),
        };
        assert_eq!(result_exit_code(&result), EXIT_INCOMPLETE_RESULT);
        assert_ne!(EXIT_INCOMPLETE_RESULT, 0);
        // Distinct from the generic failure codes used elsewhere in this module.
        assert_ne!(EXIT_INCOMPLETE_RESULT, 1);
        assert_ne!(EXIT_INCOMPLETE_RESULT, 2);
    }

    /// Build a minimal display `QueryResponse` for footer rendering tests.
    fn display_result(
        row_count: u64,
        total_row_count: Option<u64>,
        truncated: bool,
    ) -> QueryResponse {
        QueryResponse {
            result_id: None,
            columns: vec!["id".to_string()],
            rows: Vec::new(),
            row_count,
            total_row_count,
            truncated,
            execution_time_ms: Some(5),
            warning: None,
        }
    }

    #[test]
    fn table_footer_marks_incomplete_preview_loudly() {
        let footer = table_footer(&display_result(1, Some(100), true));
        assert!(footer.contains("INCOMPLETE PREVIEW"), "footer: {footer}");
        assert!(footer.contains("1 of 100 rows"), "footer: {footer}");
    }

    #[test]
    fn table_footer_renders_question_mark_for_unknown_total() {
        // Truncated with no server-reported total → "1 of ? rows".
        let footer = table_footer(&display_result(1, None, true));
        assert!(footer.contains("1 of ? rows"), "footer: {footer}");
        assert!(footer.contains("INCOMPLETE PREVIEW"), "footer: {footer}");
    }

    #[test]
    fn table_footer_for_complete_result_is_plain() {
        let footer = table_footer(&display_result(2, Some(2), false));
        assert!(!footer.contains("INCOMPLETE"), "footer: {footer}");
        assert!(footer.starts_with("2 rows"), "footer: {footer}");
    }

    #[test]
    fn resolve_inline_preserves_existing_warning_when_following_fails() {
        // A truncated response with no result_id often arrives with an SDK
        // warning explaining why persistence didn't start. The truncation note
        // is appended to it, not allowed to clobber it.
        let server = mockito::Server::new();
        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));

        let mut resp = truncated_preview(None);
        resp.warning = Some(Some(
            "result persistence could not be initiated".to_string(),
        ));

        let resolved = resolve_inline(&api, resp);
        let warning = resolved.warning.as_deref().unwrap_or("");
        assert!(
            warning.contains("result persistence could not be initiated"),
            "original warning dropped: {warning:?}"
        );
        assert!(
            warning.contains("truncated"),
            "truncation note missing: {warning:?}"
        );
    }
}
