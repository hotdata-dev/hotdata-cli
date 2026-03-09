use crate::config;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct QueryResponse {
    result_id: Option<String>,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    row_count: u64,
    execution_time_ms: u64,
    warning: Option<String>,
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
    let profile_config = match config::load("default") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => {
            eprintln!("error: not authenticated. Run 'hotdata auth login' to log in.");
            std::process::exit(1);
        }
    };

    let url = format!("{}/query", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let mut body = serde_json::json!({ "sql": sql });
    if let Some(conn) = connection {
        body["connection_id"] = Value::String(conn.to_string());
    }

    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .json(&body)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        let _status = resp.status();
        let body = resp.text().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(body);
        use crossterm::style::Stylize;
        eprintln!("{}", message.red());
        std::process::exit(1);
    }

    let result: QueryResponse = match resp.json() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

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
            // Compute column widths
            let mut widths: Vec<usize> = result.columns.iter().map(|c| c.len()).collect();
            for row in &result.rows {
                for (i, cell) in row.iter().enumerate() {
                    if let Some(w) = widths.get_mut(i) {
                        *w = (*w).max(value_to_string(cell).len());
                    }
                }
            }

            // Header
            let header: Vec<String> = result.columns.iter().enumerate()
                .map(|(i, c)| format!("{:<width$}", c, width = widths[i]))
                .collect();
            println!("{}", header.join("  "));

            // Separator
            let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
            println!("{}", sep.join("  "));

            // Rows
            for row in &result.rows {
                let cells: Vec<String> = row.iter().enumerate()
                    .map(|(i, v)| format!("{:<width$}", value_to_string(v), width = widths.get(i).copied().unwrap_or(0)))
                    .collect();
                println!("{}", cells.join("  "));
            }

            // Footer
            eprintln!("\n{} row{} ({} ms)", result.row_count, if result.row_count == 1 { "" } else { "s" }, result.execution_time_ms);
        }
        _ => unreachable!(),
    }
}
