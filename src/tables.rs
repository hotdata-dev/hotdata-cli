use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Deserialize, Serialize)]
struct Table {
    connection: String,
    schema: String,
    table: String,
    synced: bool,
    last_sync: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    columns: Vec<Column>,
}

#[derive(Deserialize)]
struct ListResponse {
    tables: Vec<Table>,
}

pub fn list(workspace_id: &str, connection_id: Option<&str>, format: &str) {
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

    let mut url = format!("{}/information_schema", profile_config.api_url);
    if let Some(conn_id) = connection_id {
        url.push_str(&format!("?connection_id={conn_id}"));
    }

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
        eprintln!("error: HTTP {}", resp.status());
        std::process::exit(1);
    }

    let mut body: ListResponse = match resp.json() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    if connection_id.is_none() {
        for t in &mut body.tables {
            t.columns.clear();
        }
    }

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&body.tables).unwrap());
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&body.tables).unwrap());
        }
        "table" => {
            let mut table = crate::util::make_table();
            if connection_id.is_some() {
                table.set_header(["TABLE", "COLUMN", "DATA_TYPE", "NULLABLE"]);
                for t in &body.tables {
                    for col in &t.columns {
                        table.add_row([
                            &t.table,
                            &col.name,
                            &col.data_type,
                            &col.nullable.to_string(),
                        ]);
                    }
                }
            } else {
                table.set_header(["TABLE", "SYNCED", "LAST_SYNC"]);
                for t in &body.tables {
                    table.add_row([
                        &t.table,
                        &t.synced.to_string(),
                        t.last_sync.as_deref().unwrap_or("-"),
                    ]);
                }
            }
            println!("{table}");
        }
        _ => unreachable!(),
    }
}
