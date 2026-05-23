use crate::api::ApiClient;
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

#[derive(Deserialize)]
struct AsyncResponse {
    query_run_id: String,
    status: String,
}

#[derive(Deserialize)]
struct QueryRunResponse {
    id: String,
    status: String,
    result_id: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
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
        Boolean => Value::Bool(col.as_any().downcast_ref::<BooleanArray>().unwrap().value(row)),
        Int8  => Value::Number(col.as_any().downcast_ref::<Int8Array>().unwrap().value(row).into()),
        Int16 => Value::Number(col.as_any().downcast_ref::<Int16Array>().unwrap().value(row).into()),
        Int32 => Value::Number(col.as_any().downcast_ref::<Int32Array>().unwrap().value(row).into()),
        Int64 => Value::Number(col.as_any().downcast_ref::<Int64Array>().unwrap().value(row).into()),
        UInt8  => Value::Number(col.as_any().downcast_ref::<UInt8Array>().unwrap().value(row).into()),
        UInt16 => Value::Number(col.as_any().downcast_ref::<UInt16Array>().unwrap().value(row).into()),
        UInt32 => Value::Number(col.as_any().downcast_ref::<UInt32Array>().unwrap().value(row).into()),
        UInt64 => Value::Number(col.as_any().downcast_ref::<UInt64Array>().unwrap().value(row).into()),
        Float32 => {
            let v = col.as_any().downcast_ref::<Float32Array>().unwrap().value(row) as f64;
            Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
        }
        Float64 => {
            let v = col.as_any().downcast_ref::<Float64Array>().unwrap().value(row);
            Number::from_f64(v).map(Value::Number).unwrap_or(Value::Null)
        }
        Utf8 => Value::String(
            col.as_any().downcast_ref::<StringArray>().unwrap().value(row).to_owned(),
        ),
        LargeUtf8 => Value::String(
            col.as_any().downcast_ref::<LargeStringArray>().unwrap().value(row).to_owned(),
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

    let columns: Vec<String> = reader.schema().fields().iter().map(|f| f.name().clone()).collect();
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
            rows.push((0..batch.num_columns()).map(|c| arrow_cell(batch.column(c).as_ref(), row)).collect());
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
pub(crate) fn fetch_arrow_result(api: &ApiClient, result_id: &str) -> QueryResponse {
    let (status, bytes) = api.get_bytes(&format!("/results/{result_id}"), ACCEPT_ARROW);
    if !status.is_success() {
        use crossterm::style::Stylize;
        let msg = String::from_utf8_lossy(&bytes);
        eprintln!("{}", format!("error fetching result: {status} {msg}").red());
        std::process::exit(1);
    }
    arrow_ipc_to_query_response(bytes, result_id.to_owned())
}

pub fn execute(
    sql: &str,
    workspace_id: &str,
    connection: Option<&str>,
    database: Option<&str>,
    format: &str,
) {
    let mut api = ApiClient::new(Some(workspace_id));
    if let Some(db_id) = database {
        api = api.with_database(db_id);
    }

    let mut body = serde_json::json!({
        "sql": sql,
        "async": true,
        "async_after_ms": 1000,
    });
    if let Some(conn) = connection {
        body["connection_id"] = Value::String(conn.to_string());
    }

    let spinner = crate::util::spinner("running query...");
    let (status, resp_body) = api.post_raw("/query", &body);
    spinner.finish_and_clear();

    if status.as_u16() == 202 {
        // Query didn't complete within async_after_ms — poll until done, then
        // fetch the result as Arrow IPC.
        let async_resp: AsyncResponse = match serde_json::from_str(&resp_body) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error parsing async response: {e}");
                std::process::exit(1);
            }
        };

        let run_id = &async_resp.query_run_id;
        let spinner = crate::util::spinner("waiting for query...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);

        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
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
            let run: QueryRunResponse = api.get(&format!("/query-runs/{run_id}"));
            match run.status.as_str() {
                "succeeded" => {
                    spinner.finish_and_clear();
                    match run.result_id {
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
                    let err = run.error_message.as_deref().unwrap_or("unknown error");
                    eprintln!("{}", format!("query failed: {err}").red());
                    std::process::exit(1);
                }
                "running" | "queued" | "pending" => continue,
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
        }
    }

    if !status.is_success() {
        let message = serde_json::from_str::<Value>(&resp_body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(resp_body);
        use crossterm::style::Stylize;
        eprintln!("{}", message.red());
        std::process::exit(1);
    }

    // Fast path: query completed synchronously — response is JSON from /query.
    let result: QueryResponse = match serde_json::from_str(&resp_body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    print_result(&result, format);
}

/// Poll a query run by ID. If succeeded and has a result_id, fetch and display the result.
pub fn poll(query_run_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let run: QueryRunResponse = api.get(&format!("/query-runs/{query_run_id}"));

    match run.status.as_str() {
        "succeeded" => match run.result_id {
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
            let err = run.error_message.as_deref().unwrap_or("unknown error");
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

