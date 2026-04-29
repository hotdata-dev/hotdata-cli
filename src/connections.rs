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

pub fn refresh(
    workspace_id: &str,
    connection_id: &str,
    data: bool,
    schema: Option<&str>,
    table: Option<&str>,
    async_mode: bool,
    include_uncached: bool,
) {
    use crossterm::style::Stylize;

    if async_mode && !data {
        eprintln!(
            "{}",
            "--async only valid with --data (schema refresh is always synchronous)".red()
        );
        std::process::exit(1);
    }
    if include_uncached && !data {
        eprintln!("{}", "--include-uncached only valid with --data".red());
        std::process::exit(1);
    }
    if include_uncached && table.is_some() {
        eprintln!(
            "{}",
            "--include-uncached cannot be combined with --table (it only applies to connection-wide refresh)".red()
        );
        std::process::exit(1);
    }
    if table.is_some() && schema.is_none() {
        eprintln!("{}", "--table requires --schema".red());
        std::process::exit(1);
    }
    if data && schema.is_some() && table.is_none() {
        eprintln!(
            "{}",
            "--schema requires --table for data refresh (no schema-scoped data refresh)".red()
        );
        std::process::exit(1);
    }

    let mut body = serde_json::json!({
        "connection_id": connection_id,
        "data": data,
    });
    if let Some(s) = schema {
        body["schema_name"] = serde_json::Value::String(s.to_string());
    }
    if let Some(t) = table {
        body["table_name"] = serde_json::Value::String(t.to_string());
    }
    if async_mode {
        body["async"] = serde_json::Value::Bool(true);
    }
    if include_uncached {
        body["include_uncached"] = serde_json::Value::Bool(true);
    }

    let api = ApiClient::new(Some(workspace_id));
    let (status, resp_body) = api.post_raw("/refresh", &body);

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let parsed: serde_json::Value = serde_json::from_str(&resp_body).unwrap_or_default();

    if async_mode {
        let job_id = parsed["id"].as_str().unwrap_or("unknown");
        println!("{}", "Data refresh submitted.".green());
        println!("job_id: {}", job_id);
        println!(
            "{}",
            format!("Use 'hotdata jobs {}' to check status.", job_id).dark_grey()
        );
        return;
    }

    if !data {
        let discovered = parsed["tables_discovered"].as_u64().unwrap_or(0);
        let added = parsed["tables_added"].as_u64().unwrap_or(0);
        let modified = parsed["tables_modified"].as_u64().unwrap_or(0);
        println!("{}", "Schema refresh completed.".green());
        println!(
            "{}",
            format!("  tables: {discovered} discovered, {added} added, {modified} modified")
                .dark_grey()
        );
        return;
    }

    if let Some(rows) = parsed["rows_synced"].as_u64() {
        let dur = parsed["duration_ms"].as_u64().unwrap_or(0);
        println!("{}", "Data refresh completed.".green());
        println!("{}", format!("  {rows} rows synced ({dur} ms)").dark_grey());
    } else {
        let refreshed = parsed["tables_refreshed"].as_u64().unwrap_or(0);
        let failed = parsed["tables_failed"].as_u64().unwrap_or(0);
        let total = parsed["total_rows"].as_u64().unwrap_or(0);
        let dur = parsed["duration_ms"].as_u64().unwrap_or(0);
        println!("{}", "Data refresh completed.".green());
        println!(
            "{}",
            format!(
                "  {refreshed} tables refreshed, {failed} failed, {total} total rows ({dur} ms)"
            )
            .dark_grey()
        );
        if let Some(errors) = parsed["errors"].as_array() {
            if !errors.is_empty() {
                eprintln!("{}", format!("  {} error(s):", errors.len()).yellow());
                for err in errors {
                    eprintln!("    {}", err);
                }
            }
        }
    }
}
