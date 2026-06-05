use crate::sdk::Api;
use serde::Deserialize;
use serde_json::Value;

const ACCEPT_ARROW: &str = "application/vnd.apache.arrow.stream";

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

/// Decode an Arrow IPC stream into a `QueryResponse` suitable for display.
fn arrow_ipc_to_query_response(bytes: Vec<u8>, result_id: String) -> QueryResponse {
    use arrow::ipc::reader::StreamReader;
    use std::io::Cursor;

    let reader = match StreamReader::try_new(Cursor::new(&bytes), None) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error reading Arrow IPC stream: {e}");
            std::process::exit(1);
        }
    };

    let columns: Vec<String> = reader
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut rows: Vec<Vec<Value>> = Vec::new();

    for batch_result in reader {
        let batch = match batch_result {
            Ok(b) => b,
            Err(e) => {
                eprintln!("error reading Arrow batch: {e}");
                std::process::exit(1);
            }
        };
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

/// Fetch `/results/{result_id}` as Arrow IPC and return a `QueryResponse`.
///
/// The Arrow stream is fetched through the SDK seam ([`Api::get_bytes`]) — same
/// auth/transport as every other call — but decoded here with the CLI's own
/// pinned `arrow` crate rather than the SDK's `get_result_arrow` (whose
/// `RecordBatch` comes from a different `arrow` major version).
pub(crate) fn fetch_arrow_result(api: &Api, result_id: &str) -> QueryResponse {
    let (status, bytes) = api
        .get_bytes(&format!("/results/{result_id}"), ACCEPT_ARROW)
        .unwrap_or_else(|e| e.exit());
    if !status.is_success() {
        use crossterm::style::Stylize;
        let msg = String::from_utf8_lossy(&bytes);
        eprintln!("{}", format!("error fetching result: {status} {msg}").red());
        std::process::exit(1);
    }
    arrow_ipc_to_query_response(bytes, result_id.to_owned())
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
        // Completed within async_after_ms — inline results.
        hotdata::QueryOutcome::Inline(resp) => {
            print_result(&query_response_from_sdk(resp), format);
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
