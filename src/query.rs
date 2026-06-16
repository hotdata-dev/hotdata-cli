use crate::sdk::Api;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct QueryResponse {
    pub result_id: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: u64,
    pub execution_time_ms: Option<u64>,
    pub warning: Option<String>,
}

/// Convert the SDK's inline `QueryResponse` (200 path) into the CLI's display
/// model. The async path decodes Arrow instead (see `fetch_arrow_result`).
fn query_response_from_sdk(resp: hotdata::models::QueryResponse) -> QueryResponse {
    QueryResponse {
        result_id: resp.result_id.flatten(),
        columns: resp.columns,
        rows: resp.rows,
        row_count: resp.row_count.max(0) as u64,
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
    QueryResponse {
        result_id: Some(result_id),
        columns,
        rows,
        row_count,
        execution_time_ms: None,
        warning: None,
    }
}

/// Fetch `/results/{result_id}` as Arrow and return a `QueryResponse`.
///
/// Both transport and decode are owned by the SDK's `get_result_arrow` (via the
/// [`Api::get_result_arrow`] seam), so the CLI shares one `arrow` major version
/// with the SDK.
pub(crate) fn fetch_arrow_result(api: &Api, result_id: &str) -> QueryResponse {
    let result = api.get_result_arrow(result_id).unwrap_or_else(|e| e.exit());
    arrow_result_to_query_response(result, result_id.to_owned())
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
/// initiated — see the SDK's `warning` field), the full result is unfetchable,
/// so fall back to the preview and surface a warning rather than failing.
fn resolve_inline(api: &Api, resp: hotdata::models::QueryResponse) -> QueryResponse {
    if !resp.truncated {
        return query_response_from_sdk(resp);
    }
    match resp.result_id.clone().flatten() {
        Some(result_id) => {
            // The Arrow fetch returns only schema + rows; carry the query-level
            // warning and execution time the inline response reported, which
            // `arrow_result_to_query_response` otherwise hardcodes to None.
            let mut full = fetch_arrow_result(api, &result_id);
            full.warning = resp.warning.flatten();
            full.execution_time_ms = Some(resp.execution_time_ms.max(0) as u64);
            full
        }
        None => {
            let mut preview = query_response_from_sdk(resp);
            let note = "result truncated to a preview; full result unavailable (persistence not initiated)";
            preview.warning = Some(match preview.warning {
                Some(w) => format!("{w}; {note}"),
                None => note.to_string(),
            });
            preview
        }
    }
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

    let spinner = crate::util::spinner("running query...");
    let outcome = crate::sdk::block(api.client().submit_query(request, database))
        .unwrap_or_else(|e| e.exit());
    spinner.finish_and_clear();

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

pub fn print_result(result: &QueryResponse, format: &str) {
    if let Some(ref warning) = result.warning {
        eprintln!("warning: {warning}");
    }

    match format {
        "json" => {
            let out = serde_json::json!({
                "result_id": result.result_id,
                "columns": result.columns,
                "rows": result.rows,
                "row_count": result.row_count,
                "execution_time_ms": result.execution_time_ms,
            });
            println!("{}", serde_json::to_string_pretty(&out).unwrap());
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
            let id_part = result
                .result_id
                .as_deref()
                .map(|id| format!(" [result-id: {id}]"))
                .unwrap_or_default();
            let time_part = match result.execution_time_ms {
                Some(ms) => format!("{ms} ms"),
                None => "\u{2014}".to_string(), // em dash
            };
            eprintln!(
                "{}",
                format!(
                    "\n{} row{} ({}){}",
                    result.row_count,
                    if result.row_count == 1 { "" } else { "s" },
                    time_part,
                    id_part
                )
                .dark_grey()
            );
        }
        _ => unreachable!(),
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
        // Inline warning + timing carried through, not dropped by the fetch.
        assert_eq!(resolved.warning.as_deref(), Some("approximate aggregate"));
        assert_eq!(resolved.execution_time_ms, Some(5));
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
    }

    #[test]
    fn resolve_inline_truncated_without_result_id_warns_and_keeps_preview() {
        // Truncated but persistence never started (result_id is null): the full
        // result is unfetchable, so keep the preview and surface a warning.
        let server = mockito::Server::new();
        let api = Api::test_new(&server.url(), "test-jwt", Some("ws-1"));

        let resolved = resolve_inline(&api, truncated_preview(None));
        assert_eq!(resolved.row_count, 1);
        assert_eq!(resolved.rows.len(), 1);
        assert!(
            resolved
                .warning
                .as_deref()
                .unwrap_or("")
                .contains("truncated")
        );
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
