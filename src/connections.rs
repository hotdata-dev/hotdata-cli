use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct ConnectionType {
    name: String,
    label: String,
}

#[derive(Deserialize)]
struct ListConnectionTypesResponse {
    connection_types: Vec<ConnectionType>,
}

#[derive(Deserialize, Serialize)]
struct ConnectionTypeDetail {
    name: String,
    label: String,
    config_schema: Option<serde_json::Value>,
    auth: Option<serde_json::Value>,
}

pub fn types_list(workspace_id: &str, format: &str) {
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

    let url = format!("{}/connection-types", profile_config.api_url);
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

    let body: ListConnectionTypesResponse = match resp.json() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.connection_types).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.connection_types).unwrap()),
        "table" => {
            let mut table = crate::util::make_table();
            table.set_header(["NAME", "LABEL"]);
            for ct in &body.connection_types {
                table.add_row([&ct.name, &ct.label]);
            }
            println!("{table}");
        }
        _ => unreachable!(),
    }
}

pub fn types_get(workspace_id: &str, name: &str, format: &str) {
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

    let url = format!("{}/connection-types/{name}", profile_config.api_url);
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

    let detail: ConnectionTypeDetail = match resp.json() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&detail).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&detail).unwrap()),
        "table" => {
            println!("name:   {}", detail.name);
            println!("label:  {}", detail.label);
            if let Some(schema) = &detail.config_schema {
                println!("config: {}", serde_json::to_string_pretty(schema).unwrap());
            }
            if let Some(auth) = &detail.auth {
                println!("auth:   {}", serde_json::to_string_pretty(auth).unwrap());
            }
        }
        _ => unreachable!(),
    }
}

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
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
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
            table.set_header(["ID", "NAME", "SOURCE TYPE"]);
            for c in &body.connections {
                table.add_row([&c.id, &c.name, &c.source_type]);
            }
            println!("{table}");
        }
        _ => unreachable!(),
    }
}
