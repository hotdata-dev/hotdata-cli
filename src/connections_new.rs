use inquire::{Confirm, Password, Select, Text};
use inquire::validator::Validation;
use serde_json::{Map, Number, Value};

// ── HTTP helpers ──────────────────────────────────────────────────────────────

struct ConnectionTypeSummary {
    name: String,
    label: String,
}

struct ConnectionTypeDetail {
    name: String,
    config_schema: Option<Value>,
    auth: Option<Value>,
}

fn load_client() -> (reqwest::blocking::Client, String, String) {
    let profile = match crate::config::load("default") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let api_key = match &profile.api_key {
        Some(k) if k != "PLACEHOLDER" => k.clone(),
        _ => {
            eprintln!("error: not authenticated. Run 'hotdata auth login' to log in.");
            std::process::exit(1);
        }
    };
    (reqwest::blocking::Client::new(), api_key, profile.api_url.to_string())
}

fn fetch_types(workspace_id: &str) -> Vec<ConnectionTypeSummary> {
    let (client, api_key, api_url) = load_client();
    let url = format!("{api_url}/connection-types");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    if !resp.status().is_success() {
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()));
        std::process::exit(1);
    }
    let body: Value = resp.json().unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
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

fn fetch_detail(workspace_id: &str, name: &str) -> ConnectionTypeDetail {
    let (client, api_key, api_url) = load_client();
    let url = format!("{api_url}/connection-types/{name}");
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
        .unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    if !resp.status().is_success() {
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()));
        std::process::exit(1);
    }
    let body: Value = resp.json().unwrap_or_else(|e| { eprintln!("error: {e}"); std::process::exit(1) });
    ConnectionTypeDetail {
        name: body["name"].as_str().unwrap_or(name).to_string(),
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
                .with_validator(|input: &str| {
                    if input.is_empty() || input.parse::<i64>().is_ok() {
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
        return walk_properties(&one_of[idx]);
    }
    // Single auth method
    walk_properties(schema)
}

// ── Entry point ───────────────────────────────────────────────────────────────

pub fn run(workspace_id: &str) {
    // Phase 1: Select connection type
    let types = fetch_types(workspace_id);
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
    let detail = fetch_detail(workspace_id, source_type);

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
    let (client, api_key, api_url) = load_client();
    let body = serde_json::json!({
        "name": conn_name,
        "source_type": source_type,
        "config": Value::Object(config),
    });

    let url = format!("{api_url}/connections");
    let resp = client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .json(&body)
        .send()
        .unwrap_or_else(|e| { eprintln!("error connecting to API: {e}"); std::process::exit(1) });

    if !resp.status().is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    #[derive(serde::Deserialize)]
    struct CreateResponse {
        id: String,
        name: String,
        source_type: String,
        tables_discovered: u64,
        discovery_status: String,
        discovery_error: Option<String>,
    }

    let result: CreateResponse = resp.json()
        .unwrap_or_else(|e| { eprintln!("error parsing response: {e}"); std::process::exit(1) });

    use crossterm::style::Stylize;
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
}
