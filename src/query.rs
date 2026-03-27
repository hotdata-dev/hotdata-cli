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

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(_) | Value::Object(_) => v.to_string(),
    }
}

pub fn execute(sql: &str, workspace_id: &str, connection: Option<&str>, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let mut body = serde_json::json!({ "sql": sql });
    if let Some(conn) = connection {
        body["connection_id"] = Value::String(conn.to_string());
    }

    let (status, resp_body) = api.post_raw("/query", &body);

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
