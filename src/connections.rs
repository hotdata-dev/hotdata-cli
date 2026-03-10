use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Connection {
    id: String,
    name: String,
    source_type: String,
}

#[derive(Deserialize)]
struct ListResponse {
    connections: Vec<Connection>,
}

pub fn list(workspace_id: &str, format: &str) {
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

    let url = format!("{}/connections", profile_config.api_url);
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

    let body: ListResponse = match resp.json() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&body.connections).unwrap());
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&body.connections).unwrap());
        }
        "table" => {
            let mut table = crate::util::make_table();
            table.set_header(["ID", "NAME", "SOURCE_TYPE"]);
            for c in &body.connections {
                table.add_row([&c.id, &c.name, &c.source_type]);
            }
            println!("{table}");
        }
        _ => unreachable!(),
    }
}
