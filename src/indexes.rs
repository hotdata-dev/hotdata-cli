use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Index {
    index_name: String,
    index_type: String,
    columns: Vec<String>,
    metric: Option<String>,
    status: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    indexes: Vec<Index>,
}

pub fn list(
    workspace_id: &str,
    connection_id: &str,
    schema: &str,
    table: &str,
    format: &str,
) {
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
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };

    let url = format!(
        "{}/connections/{}/tables/{}/{}/indexes",
        profile_config.api_url, connection_id, schema, table
    );
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
        "json" => println!("{}", serde_json::to_string_pretty(&body.indexes).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.indexes).unwrap()),
        "table" => {
            if body.indexes.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No indexes found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.indexes.iter().map(|i| vec![
                    i.index_name.clone(),
                    i.index_type.clone(),
                    i.columns.join(", "),
                    i.metric.clone().unwrap_or_default(),
                    i.status.clone(),
                    crate::util::format_date(&i.created_at),
                ]).collect();
                crate::table::print(&["NAME", "TYPE", "COLUMNS", "METRIC", "STATUS", "CREATED"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn create(
    workspace_id: &str,
    connection_id: &str,
    schema: &str,
    table: &str,
    name: &str,
    columns: &str,
    index_type: &str,
    metric: Option<&str>,
    async_mode: bool,
) {
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
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };

    let cols: Vec<&str> = columns.split(',').map(str::trim).collect();
    let mut body = serde_json::json!({
        "index_name": name,
        "columns": cols,
        "index_type": index_type,
        "async": async_mode,
    });
    if let Some(m) = metric {
        body["metric"] = serde_json::json!(m);
    }

    let url = format!(
        "{}/connections/{}/tables/{}/{}/indexes",
        profile_config.api_url, connection_id, schema, table
    );
    let client = reqwest::blocking::Client::new();

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
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    use crossterm::style::Stylize;
    if async_mode {
        let body: serde_json::Value = resp.json().unwrap_or_default();
        let job_id = body["job_id"].as_str().unwrap_or("unknown");
        println!("{}", "Index creation submitted.".green());
        println!("job_id: {}", job_id);
        println!("{}", "Use 'hotdata jobs <job_id>' to check status.".dark_grey());
    } else {
        println!("{}", "Index created.".green());
    }
}
