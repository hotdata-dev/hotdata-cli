use inquire::validator::Validation;
use inquire::{Confirm, Password, Select, Text};
use serde_json::{Map, Number, Value};

use crate::client::sdk::{Api, ApiError, block, block_with_wakeup};

// ── SDK helpers ─────────────────────────────────────────────────────────────

struct ConnectionTypeSummary {
    name: String,
    label: String,
}

struct ConnectionTypeDetail {
    config_schema: Option<Value>,
    auth: Option<Value>,
}

fn fetch_types(api: &Api) -> Vec<ConnectionTypeSummary> {
    let resp = block(api.client().connection_types().list()).unwrap_or_else(|e| e.exit());
    resp.connection_types
        .into_iter()
        .map(|t| ConnectionTypeSummary {
            name: t.name,
            label: t.label,
        })
        .collect()
}

fn fetch_detail(api: &Api, name: &str) -> ConnectionTypeDetail {
    let detail = block(api.client().connection_types().get(name)).unwrap_or_else(|e| e.exit());
    // The SDK models nullable fields as `Option<Option<Value>>`; flatten and
    // treat an explicit JSON `null` as absent to match the old `is_null()` check.
    let flatten = |field: Option<Option<Value>>| -> Option<Value> {
        field.flatten().filter(|v| !v.is_null())
    };
    ConnectionTypeDetail {
        config_schema: flatten(detail.config_schema),
        auth: flatten(detail.auth),
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

    let Some(props) = schema["properties"].as_object() else {
        return out;
    };

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

    let Some(props) = schema["properties"].as_object() else {
        return out;
    };

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
    let description = field["description"].as_str();
    // Accept both string and integer examples — `as_str` alone would silently
    // miss schemas like `"examples": [8080]` on integer fields.
    let example: Option<String> = field["examples"]
        .as_array()
        .and_then(|a| a.first())
        .and_then(|v| {
            v.as_str()
                .map(str::to_owned)
                .or_else(|| v.as_i64().map(|n| n.to_string()))
        });
    let opt_hint = "optional — press Enter to skip";
    let help_message: Option<String> = match (description, is_required) {
        (Some(d), true) => Some(d.to_string()),
        (Some(d), false) => Some(format!("{d} ({opt_hint})")),
        (None, true) => None,
        (None, false) => Some(opt_hint.to_string()),
    };

    match (field_type, format) {
        ("string", "password") => {
            let label = format!("{key}:");
            let mut p = Password::new(&label).without_confirmation();
            if let Some(h) = &help_message {
                p = p.with_help_message(h);
            }
            let val = p.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required {
                None
            } else {
                Some(Value::String(val))
            }
        }

        ("string", _) => {
            let label = format!("{key}:");
            let mut t = Text::new(&label);
            let default = field["default"].as_str();
            if let Some(d) = default {
                t = t.with_default(d);
            } else if let Some(e) = example.as_deref() {
                t = t.with_placeholder(e);
            }
            if let Some(h) = &help_message {
                t = t.with_help_message(h);
            }
            let val = t.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required {
                None
            } else {
                Some(Value::String(val))
            }
        }

        ("integer", _) => {
            let label = format!("{key}:");
            let mut t = Text::new(&label).with_validator(move |input: &str| {
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
            if let Some(e) = example.as_deref() {
                t = t.with_placeholder(e);
            }
            if let Some(h) = &help_message {
                t = t.with_help_message(h);
            }
            let val = t.prompt().unwrap_or_else(|_| std::process::exit(0));
            if val.is_empty() && !is_required {
                None
            } else {
                val.parse::<i64>()
                    .ok()
                    .map(|n| Value::Number(Number::from(n)))
            }
        }

        ("boolean", _) => {
            let label = format!("{key}:");
            let default = field["default"].as_bool().unwrap_or(false);
            let mut c = Confirm::new(&label).with_default(default);
            if let Some(h) = &help_message {
                c = c.with_help_message(h);
            }
            let val = c.prompt().unwrap_or_else(|_| std::process::exit(0));
            Some(Value::Bool(val))
        }

        ("array", _) => {
            let label = format!("{key}:");
            let array_hint = if is_required {
                "Enter values separated by commas"
            } else {
                "Enter values separated by commas — optional, press Enter to skip"
            };
            let help = match description {
                Some(d) => format!("{d} — {array_hint}"),
                None => array_hint.to_string(),
            };
            let val = Text::new(&label)
                .with_placeholder(example.as_deref().unwrap_or("value1, value2, ..."))
                .with_help_message(&help)
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
    if !crate::util::is_interactive() {
        eprintln!(
            "error: 'connections new' is interactive and stdin is not a TTY. \
             Use 'hotdata connections create list' to discover types and their config schemas, \
             then 'hotdata connections create --name <n> --type <t> --config '{{…}}''."
        );
        std::process::exit(1);
    }

    let api = Api::new(Some(workspace_id));

    // Phase 1: Select connection type
    let types = fetch_types(&api);
    if types.is_empty() {
        eprintln!("error: no connection types available");
        std::process::exit(1);
    }
    let displays: Vec<String> = types
        .iter()
        .map(|t| format!("{} ({})", t.label, t.name))
        .collect();
    let names: Vec<String> = types.iter().map(|t| t.name.clone()).collect();

    let selected_display = Select::new("Connection type:", displays.clone())
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    let idx = displays
        .iter()
        .position(|d| d == &selected_display)
        .unwrap();
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
    let request = hotdata::models::CreateConnectionRequest::new(
        config.into_iter().collect(),
        conn_name,
        source_type.clone(),
    );

    /// Health outcome: a fetched response, or an unavailable reason string.
    enum HealthStatus {
        Available(hotdata::models::ConnectionHealthResponse),
        Unavailable(String),
    }

    /// Render an [`ApiError`] the way the old raw paths did: the server body
    /// through `api_error`, or the transport message verbatim.
    fn error_text(e: ApiError) -> String {
        match e {
            ApiError::Status { body, .. } => crate::util::api_error(body),
            ApiError::Transport(msg) => msg,
        }
    }

    let result = block_with_wakeup(
        &api,
        "Creating connection...",
        api.client().connections().create(request),
    );

    use crossterm::style::Stylize;
    let result = match result {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", error_text(e).red());
            std::process::exit(1);
        }
    };

    let health_spinner = crate::util::spinner("Checking connection health...");
    let health_result = block(api.client().connections().check_health(&result.id));
    health_spinner.finish_and_clear();

    let health = match health_result {
        Ok(h) => HealthStatus::Available(h),
        Err(e) => HealthStatus::Unavailable(error_text(e)),
    };

    println!("{}", "Connection created".green());
    println!("id:                {}", result.id);
    println!("name:              {}", result.name);
    println!("source_type:       {}", result.source_type);
    println!("tables_discovered: {}", result.tables_discovered);
    let discovery_status = result.discovery_status.to_string();
    let status = match discovery_status.as_str() {
        "success" => discovery_status.green().to_string(),
        "failed" => result
            .discovery_error
            .flatten()
            .as_deref()
            .unwrap_or("failed")
            .red()
            .to_string(),
        _ => discovery_status.yellow().to_string(),
    };
    println!("discovery_status:  {status}");
    let health_str = match &health {
        HealthStatus::Available(h) if h.healthy => {
            let ms = h.latency_ms;
            format!("{} {}", "healthy".green(), format!("({ms}ms)").dark_grey())
        }
        HealthStatus::Available(h) => {
            let err = h
                .error
                .as_ref()
                .and_then(|e| e.as_deref())
                .unwrap_or("unknown error");
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
