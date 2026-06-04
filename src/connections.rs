use crate::api::ApiClient;
use crate::sdk::{block, Api, ApiError};
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

/// Render an [`ApiError`] the way the old raw `fetch_health` path did: the
/// server body through `api_error`, or the transport message verbatim.
fn error_text(e: ApiError) -> String {
    match e {
        ApiError::Status { body, .. } => crate::util::api_error(body),
        ApiError::Transport(msg) => msg,
    }
}

fn fetch_health(api: &Api, connection_id: &str, show_spinner: bool) -> HealthStatus {
    let spinner = show_spinner.then(|| crate::util::spinner("Checking connection health..."));
    let result = block(api.client().connections().check_health(connection_id));
    if let Some(s) = spinner {
        s.finish_and_clear();
    }

    match result {
        Ok(h) => HealthStatus::Available(HealthResponse {
            connection_id: h.connection_id,
            healthy: h.healthy,
            latency_ms: Some(h.latency_ms as u64),
            error: h.error.flatten(),
        }),
        Err(e) => HealthStatus::Unavailable(error_text(e)),
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
    let api = Api::new(Some(workspace_id));
    let resp =
        block(api.client().connection_types().list()).unwrap_or_else(|e| e.exit());
    let body = ListConnectionTypesResponse {
        connection_types: resp
            .connection_types
            .into_iter()
            .map(|t| ConnectionType {
                name: t.name,
                label: t.label,
            })
            .collect(),
    };

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
    let api = Api::new(Some(workspace_id));
    let resp =
        block(api.client().connection_types().get(name)).unwrap_or_else(|e| e.exit());
    // The SDK models nullable fields as `Option<Option<Value>>`; flatten and
    // drop an explicit JSON `null` to match the old behavior (the old struct
    // deserialized a missing/`null` field to `None`).
    let detail = ConnectionTypeDetail {
        name: resp.name,
        label: resp.label,
        config_schema: resp.config_schema.flatten().filter(|v| !v.is_null()),
        auth: resp.auth.flatten().filter(|v| !v.is_null()),
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

/// Resolve a connection name or ID to a connection ID, exiting on failure.
///
/// If `name_or_id` looks like a raw connection ID (starts with "conn"), tries
/// `GET /connections/{id}` directly first to avoid listing the full workspace.
/// Falls back to listing and matching by name on a 404 or when given a plain name.
pub fn resolve_connection_id(api: &ApiClient, name_or_id: &str) -> String {
    use crossterm::style::Stylize;

    if name_or_id.starts_with("conn") {
        let (status, _) = api.get_raw(&format!("/connections/{name_or_id}"));
        if status.is_success() {
            return name_or_id.to_string();
        }
    }

    // Before listing connections, check if the active database's catalog or name
    // matches — prefer it over any stale connection entry with the same name.
    if let Some(ws) = api.workspace_id() {
        if let Some(active_id) = crate::config::load_current_database("default", ws) {
            if let Some(active_db) = api.get_none_if_not_found::<crate::databases::Database>(&format!("/databases/{active_id}")) {
                if active_db.default_catalog.as_deref() == Some(name_or_id)
                    || active_db.name.as_deref() == Some(name_or_id)
                {
                    return active_db.default_connection_id;
                }
            }
        }
    }

    let body: ListResponse = api.get("/connections");
    if let Some(conn) = body
        .connections
        .iter()
        .find(|c| c.id == name_or_id || c.name == name_or_id)
    {
        return conn.id.clone();
    }

    // Fall back to managed databases: treat name_or_id as a catalog alias.
    if let Ok(db) = crate::databases::try_resolve_database(api, name_or_id) {
        return db.default_connection_id;
    }

    eprintln!(
        "{}",
        format!("error: no connection named or with id '{name_or_id}'").red()
    );
    std::process::exit(1);
}

pub fn get(workspace_id: &str, connection_id: &str, format: &str) {
    let api = Api::new(Some(workspace_id));
    let is_table = format == "table";

    let spinner = is_table.then(|| crate::util::spinner("Fetching connection..."));
    let resp =
        block(api.client().connections().get(connection_id)).unwrap_or_else(|e| e.exit());
    if let Some(s) = spinner {
        s.finish_and_clear();
    }
    let detail = ConnectionDetail {
        id: resp.id,
        name: resp.name,
        source_type: resp.source_type,
        table_count: resp.table_count as u64,
        synced_table_count: resp.synced_table_count as u64,
    };

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

    let api = Api::new(Some(workspace_id));
    let is_table = format == "table";

    let spinner = is_table.then(|| crate::util::spinner("Creating connection..."));
    let (status, resp_body) = api
        .post_raw("/connections", &body)
        .unwrap_or_else(|e| {
            if let Some(s) = &spinner {
                s.finish_and_clear();
            }
            eprintln!("{}", error_text(e));
            std::process::exit(1);
        });
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
    let api = Api::new(Some(workspace_id));
    let resp = block(api.client().connections().list()).unwrap_or_else(|e| e.exit());
    let body = ListResponse {
        connections: resp
            .connections
            .into_iter()
            .map(|c| Connection {
                id: c.id,
                name: c.name,
                source_type: c.source_type,
            })
            .collect(),
    };

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

    let api = Api::new(Some(workspace_id));
    let (status, resp_body) = api.post_raw("/refresh", &body).unwrap_or_else(|e| {
        eprintln!("{}", error_text(e).red());
        std::process::exit(1);
    });

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
