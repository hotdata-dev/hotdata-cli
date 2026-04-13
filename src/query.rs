use crate::api::ApiClient;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct QueryResponse {
    pub result_id: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub row_count: u64,
    #[serde(default)]
    pub execution_time_ms: u64,
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
    error: Option<String>,
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

pub fn execute(sql: &str, workspace_id: &str, connection: Option<&str>, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

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
        let async_resp: AsyncResponse = match serde_json::from_str(&resp_body) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("error parsing async response: {e}");
                std::process::exit(1);
            }
        };
        use crossterm::style::Stylize;
        eprintln!("{}", format!("query still running (status: {})", async_resp.status).yellow());
        eprintln!("query_run_id: {}", async_resp.query_run_id);
        eprintln!("{}", format!("Poll with: hotdata query status {}", async_resp.query_run_id).dark_grey());
        std::process::exit(2);
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
        "succeeded" => {
            match run.result_id {
                Some(ref result_id) => {
                    let result: QueryResponse = api.get(&format!("/results/{result_id}"));
                    print_result(&result, format);
                }
                None => {
                    use crossterm::style::Stylize;
                    println!("{}", "Query succeeded but no result available.".yellow());
                }
            }
        }
        "failed" => {
            use crossterm::style::Stylize;
            let err = run.error.as_deref().unwrap_or("unknown error");
            eprintln!("{}", format!("query failed: {err}").red());
            std::process::exit(1);
        }
        status => {
            use crossterm::style::Stylize;
            eprintln!("{}", format!("query status: {status}").yellow());
            eprintln!("query_run_id: {}", run.id);
            eprintln!("{}", format!("Poll again with: hotdata query status {}", run.id).dark_grey());
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
                let cells: Vec<String> = row.iter().map(|v| {
                    let s = value_to_string(v);
                    if s.contains(',') || s.contains('"') || s.contains('\n') {
                        format!("\"{}\"", s.replace('"', "\"\""))
                    } else {
                        s
                    }
                }).collect();
                println!("{}", cells.join(","));
            }
        }
        "table" => {
            crate::table::print_json(&result.columns, &result.rows);
            use crossterm::style::Stylize;
            let id_part = result.result_id.as_deref().map(|id| format!(" [result-id: {id}]")).unwrap_or_default();
            eprintln!("{}", format!("\n{} row{} ({} ms){}", result.row_count, if result.row_count == 1 { "" } else { "s" }, result.execution_time_ms, id_part).dark_grey());
        }
        _ => unreachable!(),
    }
}
