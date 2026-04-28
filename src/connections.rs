use crate::api::ApiClient;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct HealthResponse {
    #[allow(dead_code)]
    connection_id: String,
    healthy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// Result of a best-effort health check. Either the endpoint responded with a
/// parseable body, or it did not — in which case we record why and keep going.
enum HealthStatus {
    Available(HealthResponse),
    Unavailable(String),
}

impl HealthStatus {
    fn is_confirmed_unhealthy(&self) -> bool {
        matches!(self, HealthStatus::Available(h) if !h.healthy)
    }
}

impl Serialize for HealthStatus {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            HealthStatus::Available(h) => h.serialize(ser),
            HealthStatus::Unavailable(_) => ser.serialize_none(),
        }
    }
}

fn fetch_health(api: &ApiClient, connection_id: &str, show_spinner: bool) -> HealthStatus {
    let spinner = show_spinner.then(|| crate::util::spinner("Checking connection health..."));
    let (status, body) = api.get_raw(&format!("/connections/{connection_id}/health"));
    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    if !status.is_success() {
        return HealthStatus::Unavailable(crate::util::api_error(body));
    }
    match serde_json::from_str::<HealthResponse>(&body) {
        Ok(h) => HealthStatus::Available(h),
        Err(e) => HealthStatus::Unavailable(format!("parse error: {e}")),
    }
}

fn format_health(health: &HealthStatus) -> String {
    use crossterm::style::Stylize;
    match health {
        HealthStatus::Available(h) if h.healthy => match h.latency_ms {
            Some(ms) => format!("{} {}", "healthy".green(), format!("({ms}ms)").dark_grey()),
            None => "healthy".green().to_string(),
        },
        HealthStatus::Available(h) => {
            let err = h.error.as_deref().unwrap_or("unknown error");
            format!("{} — {}", "unhealthy".red(), err)
        }
        HealthStatus::Unavailable(err) => {
            format!("{} — {}", "unavailable".yellow(), err)
        }
    }
}

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
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&body.connection_types).unwrap()
        ),
        "yaml" => print!("{}", serde_yaml::to_string(&body.connection_types).unwrap()),
        "table" => {
            if body.connection_types.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No connection types found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body
                    .connection_types
                    .iter()
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

#[derive(Deserialize, Serialize)]
struct ConnectionDetail {
    id: String,
    name: String,
    source_type: String,
    #[serde(default)]
    table_count: u64,
    #[serde(default)]
    synced_table_count: u64,
}

#[derive(Deserialize)]
struct ListResponse {
    connections: Vec<Connection>,
}

pub fn get(workspace_id: &str, connection_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let is_table = format == "table";

    let spinner = is_table.then(|| crate::util::spinner("Fetching connection..."));
    let detail: ConnectionDetail = api.get(&format!("/connections/{connection_id}"));
    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    let health = fetch_health(&api, connection_id, is_table);

    match format {
        "json" => {
            let combined = serde_json::json!({
                "id": detail.id,
                "name": detail.name,
                "source_type": detail.source_type,
                "table_count": detail.table_count,
                "synced_table_count": detail.synced_table_count,
                "health": &health,
            });
            println!("{}", serde_json::to_string_pretty(&combined).unwrap());
        }
        "yaml" => {
            let combined = serde_json::json!({
                "id": detail.id,
                "name": detail.name,
                "source_type": detail.source_type,
                "table_count": detail.table_count,
                "synced_table_count": detail.synced_table_count,
                "health": &health,
            });
            print!("{}", serde_yaml::to_string(&combined).unwrap());
        }
        "table" => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<16}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), detail.id.dark_cyan());
            println!("{}{}", label("name:"), detail.name.white());
            println!("{}{}", label("source_type:"), detail.source_type.green());
            println!(
                "{}{} synced / {} total",
                label("tables:"),
                detail.synced_table_count.to_string().cyan(),
                detail.table_count.to_string().cyan(),
            );
            println!("{}{}", label("health:"), format_health(&health));
        }
        _ => unreachable!(),
    }
}

#[derive(Deserialize, Serialize)]
struct CreateResponse {
    id: String,
    name: String,
    source_type: String,
    tables_discovered: u64,
    discovery_status: String,
    discovery_error: Option<String>,
}

pub fn create(workspace_id: &str, name: &str, source_type: &str, config: &str, format: &str) {
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
    let is_table = format == "table";

    let spinner = is_table.then(|| crate::util::spinner("Creating connection..."));
    let (status, resp_body) = api.post_raw("/connections", &body);
    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let result: CreateResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    let health = fetch_health(&api, &result.id, is_table);

    match format {
        "json" => {
            let combined = serde_json::json!({
                "id": result.id,
                "name": result.name,
                "source_type": result.source_type,
                "tables_discovered": result.tables_discovered,
                "discovery_status": result.discovery_status,
                "discovery_error": result.discovery_error,
                "health": &health,
            });
            println!("{}", serde_json::to_string_pretty(&combined).unwrap());
        }
        "yaml" => {
            let combined = serde_json::json!({
                "id": result.id,
                "name": result.name,
                "source_type": result.source_type,
                "tables_discovered": result.tables_discovered,
                "discovery_status": result.discovery_status,
                "discovery_error": result.discovery_error,
                "health": &health,
            });
            print!("{}", serde_yaml::to_string(&combined).unwrap());
        }
        "table" => {
            use crossterm::style::Stylize;
            println!("{}", "Connection created".green());
            println!("id:                {}", result.id);
            println!("name:              {}", result.name);
            println!("source_type:       {}", result.source_type);
            println!("tables_discovered: {}", result.tables_discovered);
            let status_colored = match result.discovery_status.as_str() {
                "success" => result.discovery_status.green().to_string(),
                "failed" => result
                    .discovery_error
                    .as_deref()
                    .unwrap_or("failed")
                    .red()
                    .to_string(),
                _ => result.discovery_status.yellow().to_string(),
            };
            println!("discovery_status:  {status_colored}");
            println!("health:            {}", format_health(&health));
        }
        _ => unreachable!(),
    }

    if health.is_confirmed_unhealthy() {
        std::process::exit(1);
    }
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/connections");

    match format {
        "json" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&body.connections).unwrap()
            );
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&body.connections).unwrap());
        }
        "table" => {
            if body.connections.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No connections found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body
                    .connections
                    .iter()
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
