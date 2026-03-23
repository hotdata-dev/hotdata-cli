use crate::config;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[allow(dead_code)]
struct ResultResponse {
    result_id: String,
    status: Option<String>,
    columns: Vec<String>,
    #[serde(default)]
    nullable: Vec<bool>,
    rows: Vec<Vec<Value>>,
    row_count: u64,
    #[serde(default)]
    execution_time_ms: u64,
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

#[derive(Deserialize, Serialize)]
struct ResultEntry {
    id: String,
    status: String,
    created_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    results: Vec<ResultEntry>,
    count: u64,
    has_more: bool,
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
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

    let mut url = format!("{}/results", profile_config.api_url);
    let mut params = vec![];
    if let Some(l) = limit { params.push(format!("limit={l}")); }
    if let Some(o) = offset { params.push(format!("offset={o}")); }
    if !params.is_empty() { url = format!("{url}?{}", params.join("&")); }

    let client = reqwest::blocking::Client::new();
    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let body: ListResponse = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.results).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.results).unwrap()),
        "table" => {
            if body.results.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No results found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.results.iter().map(|r| vec![
                    r.id.clone(),
                    r.status.clone(),
                    crate::util::format_date(&r.created_at),
                ]).collect();
                crate::table::print(&["ID", "STATUS", "CREATED AT"], &rows);
            }
            if body.has_more {
                let next = offset.unwrap_or(0) + body.count as u32;
                use crossterm::style::Stylize;
                eprintln!("{}", format!("showing {} results — use --offset {next} for more", body.count).dark_grey());
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(result_id: &str, workspace_id: &str, format: &str) {
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

    let url = format!("{}/results/{result_id}", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        let body = resp.text().unwrap_or_default();
        let message = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or(body);
        use crossterm::style::Stylize;
        eprintln!("{}", message.red());
        std::process::exit(1);
    }

    let result: ResultResponse = match resp.json() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

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
            eprintln!("{}", format!("\n{} row{} ({} ms) [result-id: {}]", result.row_count, if result.row_count == 1 { "" } else { "s" }, result.execution_time_ms, result.result_id).dark_grey());
        }
        _ => unreachable!(),
    }
}
