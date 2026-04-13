use inquire::{Confirm, Password, Select, Text};
use inquire::validator::Validation;
use serde_json::{Map, Number, Value};

use crate::api::ApiClient;

// ── HTTP helpers ──────────────────────────────────────────────────────────────

struct ConnectionTypeSummary {
    name: String,
    label: String,
}

struct ConnectionTypeDetail {
    config_schema: Option<Value>,
    auth: Option<Value>,
}

fn fetch_types(api: &ApiClient) -> Vec<ConnectionTypeSummary> {
    let body: Value = api.get("/connection-types");
    body["connection_types"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| {
            Some(ConnectionTypeSummary {
                name: v["name"].as_str()?.to_string(),
                label: v["label"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn fetch_detail(api: &ApiClient, name: &str) -> ConnectionTypeDetail {
    let body: Value = api.get(&format!("/connection-types/{name}"));
    ConnectionTypeDetail {
        config_schema: if body["config_schema"].is_null() { None } else { Some(body["config_schema"].clone()) },
        auth: if body["auth"].is_null() { None } else { Some(body["auth"].clone()) },
    }
}

// ── Schema walkers ────────────────────────────────────────────────────────────

/// Walk a flat JSON Schema object and return collected field values.
fn walk_properties(schema: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let Some(props) = schema["properties"].as_object() else { return out };

    for (key, field) in props {
        let is_required = required.contains(&key.as_str());
        if let Some(val) = prompt_field(key, field, is_required) {
            out.insert(key.clone(), val);
        }
    }
    out
}

/// Walk a oneOf variant — same as walk_properties but auto-injects `const` fields.
fn walk_variant(schema: &Value) -> Map<String, Value> {
    let mut out = Map::new();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let Some(props) = schema["properties"].as_object() else { return out };

    for (key, field) in props {
        // Auto-inject const fields without prompting
        if let Some(const_val) = field.get("const") {
            out.insert(key.clone(), const_val.clone());
            continue;
        }
        let is_required = required.contains(&key.as_str());
        if let Some(val) = prompt_field(key, field, is_required) {
            out.insert(key.clone(), val);
        }
    }
    out
}

fn prompt_field(key: &str, field: &Value, is_required: bool) -> Option<Value> {
    // Field is itself a oneOf (e.g. iceberg's catalog_type)
    if let Some(one_of) = field["oneOf"].as_array() {
        let titles: Vec<String> = one_of
            .iter()
            .filter_map(|v| v["title"].as_str().map(str::to_string))
            .collect();
        let selected = Select::new(&format!("{key}:"), titles.clone())
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        let idx = titles.iter().position(|t| t == &selected).unwrap();
        let nested = walk_variant(&one_of[idx]);
        return Some(Value::Object(nested));
    }

    let field_type = field["type"].as_str().unwrap_or("string");
    let format = field["format"].as_str().unwrap_or("");
    let opt_hint = "optional — press Enter to skip";

    match (field_type, format) {
        ("string", "password") => {
            let label = format!("{key}:");
            let mut p = Password::new(&label).without_confirmation();
            if !is_required {
                p = p.with_help_message(opt_hint);
            }
            let val = p.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required { None } else { Some(Value::String(val)) }
        }

        ("string", _) => {
            let label = format!("{key}:");
            let mut t = Text::new(&label);
            if let Some(default) = field["default"].as_str() {
                t = t.with_default(default);
            }
            if !is_required {
                t = t.with_help_message(opt_hint);
            }
            let val = t.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required { None } else { Some(Value::String(val)) }
        }

        ("integer", _) => {
            let label = format!("{key}:");
            let t = Text::new(&label)
                .with_validator(move |input: &str| {
                    if input.is_empty() {
                        if is_required {
                            return Ok(Validation::Invalid("This field is required".into()));
                        }
                        return Ok(Validation::Valid);
                    }
                    if input.parse::<i64>().is_ok() {
                        Ok(Validation::Valid)
                    } else {
                        Ok(Validation::Invalid("Must be a whole number".into()))
                    }
                });
            let help_t;
            let t = if !is_required {
                help_t = t.with_help_message(opt_hint);
                help_t
            } else {
                t
            };
            let val = t.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required {
                None
            } else {
                val.parse::<i64>().ok().map(|n| Value::Number(Number::from(n)))
            }
        }

        ("boolean", _) => {
            let label = format!("{key}:");
            let default = field["default"].as_bool().unwrap_or(false);
            let val = Confirm::new(&label)
                .with_default(default)
                .prompt()
                .unwrap_or_else(|_| std::process::exit(0));
            Some(Value::Bool(val))
        }

        ("array", _) => {
            let label = format!("{key}:");
            let help = if is_required {
                "Enter values separated by commas"
            } else {
                "Enter values separated by commas — optional, press Enter to skip"
            };
            let val = Text::new(&label)
                .with_placeholder("value1, value2, ...")
                .with_help_message(help)
                .prompt()
                .unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required {
                None
            } else {
                let items = val
                    .split(',')
                    .map(|s| Value::String(s.trim().to_string()))
                    .collect();
                Some(Value::Array(items))
            }
        }

        _ => None,
    }
}

fn walk_auth(schema: &Value) -> Map<String, Value> {
    // Multiple auth methods
    if let Some(one_of) = schema["oneOf"].as_array() {
        let titles: Vec<String> = one_of
            .iter()
            .filter_map(|v| v["title"].as_str().map(str::to_string))
            .collect();
        let selected = Select::new("Authentication method:", titles.clone())
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        let idx = titles.iter().position(|t| t == &selected).unwrap();
        return walk_variant(&one_of[idx]);
    }
    // Single auth method
    walk_properties(schema)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(workspace_id: &str) {
    let api = ApiClient::new(Some(workspace_id));

    // Phase 1: Select connection type
    let types = fetch_types(&api);
    if types.is_empty() {
        eprintln!("error: no connection types available");
        std::process::exit(1);
    }
    let displays: Vec<String> = types.iter().map(|t| format!("{} ({})", t.label, t.name)).collect();
    let names: Vec<String> = types.iter().map(|t| t.name.clone()).collect();

    let selected_display = Select::new("Connection type:", displays.clone())
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    let idx = displays.iter().position(|d| d == &selected_display).unwrap();
    let source_type = &names[idx];

    // Phase 2: Fetch schema for selected type
    let detail = fetch_detail(&api, source_type);

    // Phase 3: Connection name
    let conn_name = Text::new("Connection name:")
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));

    // Phase 4: Config properties
    let mut config: Map<String, Value> = Map::new();
    if let Some(schema) = &detail.config_schema {
        config.extend(walk_properties(schema));
    }

    // Phase 5: Auth properties
    if let Some(auth_schema) = &detail.auth {
        config.extend(walk_auth(auth_schema));
    }

    // Phase 6: Submit
    let body = serde_json::json!({
        "name": conn_name,
        "source_type": source_type,
        "config": Value::Object(config),
    });

    #[derive(serde::Deserialize)]
    struct CreateResponse {
        id: String,
        name: String,
        source_type: String,
        tables_discovered: u64,
        discovery_status: String,
        discovery_error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct HealthResponse {
        healthy: bool,
        #[serde(default)]
        latency_ms: Option<u64>,
        #[serde(default)]
        error: Option<String>,
    }

    enum HealthStatus {
        Available(HealthResponse),
        Unavailable(String),
    }

    let create_spinner = crate::util::spinner("Creating connection...");
    let (status_code, resp_body) = api.post_raw("/connections", &body);
    create_spinner.finish_and_clear();

    use crossterm::style::Stylize;
    if !status_code.is_success() {
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

    let health_spinner = crate::util::spinner("Checking connection health...");
    let (hstatus, hbody) = api.get_raw(&format!("/connections/{}/health", result.id));
    health_spinner.finish_and_clear();

    let health = if !hstatus.is_success() {
        HealthStatus::Unavailable(crate::util::api_error(hbody))
    } else {
        match serde_json::from_str::<HealthResponse>(&hbody) {
            Ok(h) => HealthStatus::Available(h),
            Err(e) => HealthStatus::Unavailable(format!("parse error: {e}")),
        }
    };

    println!("{}", "Connection created".green());
    println!("id:                {}", result.id);
    println!("name:              {}", result.name);
    println!("source_type:       {}", result.source_type);
    println!("tables_discovered: {}", result.tables_discovered);
    let status = match result.discovery_status.as_str() {
        "success" => result.discovery_status.green().to_string(),
        "failed"  => result.discovery_error.as_deref().unwrap_or("failed").red().to_string(),
        _         => result.discovery_status.yellow().to_string(),
    };
    println!("discovery_status:  {status}");
    let health_str = match &health {
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
    };
    println!("health:            {health_str}");

    if matches!(&health, HealthStatus::Available(h) if !h.healthy) {
        std::process::exit(1);
    }
}
