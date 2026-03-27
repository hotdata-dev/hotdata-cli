use crate::api::ApiClient;
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
    let api = ApiClient::new(Some(workspace_id));
    let body: ListConnectionTypesResponse = api.get("/connection-types");

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.connection_types).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.connection_types).unwrap()),
        "table" => {
            if body.connection_types.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No connection types found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.connection_types.iter()
                    .map(|ct| vec![ct.name.clone(), ct.label.clone()])
                    .collect();
                crate::table::print(&["NAME", "LABEL"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn types_get(workspace_id: &str, name: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let detail: ConnectionTypeDetail = api.get(&format!("/connection-types/{name}"));

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

pub fn create(
    workspace_id: &str,
    name: &str,
    source_type: &str,
    config: &str,
    format: &str,
) {
    let config_value: serde_json::Value = match serde_json::from_str(config) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: --config must be a valid JSON object: {e}");
            std::process::exit(1);
        }
    };

    let body = serde_json::json!({
        "name": name,
        "source_type": source_type,
        "config": config_value,
    });

    let api = ApiClient::new(Some(workspace_id));

    #[derive(Deserialize, Serialize)]
    struct CreateResponse {
        id: String,
        name: String,
        source_type: String,
        tables_discovered: u64,
        discovery_status: String,
        discovery_error: Option<String>,
    }

    let result: CreateResponse = api.post("/connections", &body);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&result).unwrap()),
        "table" => {
            use crossterm::style::Stylize;
            println!("{}", "Connection created".green());
            println!("id:                {}", result.id);
            println!("name:              {}", result.name);
            println!("source_type:       {}", result.source_type);
            println!("tables_discovered: {}", result.tables_discovered);
            let status_colored = match result.discovery_status.as_str() {
                "success" => result.discovery_status.green().to_string(),
                "failed"  => result.discovery_error.as_deref().unwrap_or("failed").red().to_string(),
                _         => result.discovery_status.yellow().to_string(),
            };
            println!("discovery_status:  {status_colored}");
        }
        _ => unreachable!(),
    }
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/connections");

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&body.connections).unwrap());
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&body.connections).unwrap());
        }
        "table" => {
            if body.connections.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No connections found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.connections.iter()
                    .map(|c| vec![c.id.clone(), c.name.clone(), c.source_type.clone()])
                    .collect();
                crate::table::print(&["ID", "NAME", "SOURCE TYPE"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn refresh(workspace_id: &str, connection_id: &str) {
    let body = serde_json::json!({
        "connection_id": connection_id,
        "data": false,
    });

    let api = ApiClient::new(Some(workspace_id));
    let (status, resp_body) = api.post_raw("/refresh", &body);

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    use crossterm::style::Stylize;
    println!("{}", "Schema refresh completed.".green());
}
