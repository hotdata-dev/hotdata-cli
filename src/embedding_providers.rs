use crate::api::ApiClient;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Provider {
    id: String,
    name: String,
    provider_type: String,
    config: serde_json::Value,
    has_secret: bool,
    source: String,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    embedding_providers: Vec<Provider>,
}

fn parse_config(raw: Option<&str>) -> Option<serde_json::Value> {
    use crossterm::style::Stylize;
    raw.map(|s| match serde_json::from_str::<serde_json::Value>(s) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", format!("--config is not valid JSON: {e}").red());
            std::process::exit(1);
        }
    })
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/embedding-providers");

    use crossterm::style::Stylize;
    match format {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&body.embedding_providers).unwrap()
        ),
        "yaml" => print!(
            "{}",
            serde_yaml::to_string(&body.embedding_providers).unwrap()
        ),
        "table" => {
            if body.embedding_providers.is_empty() {
                eprintln!("{}", "No embedding providers found.".dark_grey());
                return;
            }
            let rows: Vec<Vec<String>> = body
                .embedding_providers
                .iter()
                .map(|p| {
                    vec![
                        p.id.clone(),
                        p.name.clone(),
                        p.provider_type.clone(),
                        p.source.clone(),
                        if p.has_secret { "yes" } else { "no" }.to_string(),
                    ]
                })
                .collect();
            crate::table::print(&["ID", "NAME", "TYPE", "SOURCE", "SECRET"], &rows);
        }
        _ => unreachable!(),
    }
}

pub fn get(workspace_id: &str, id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let p: Provider = api.get(&format!("/embedding-providers/{id}"));

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&p).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&p).unwrap()),
        "table" => {
            println!("id:           {}", p.id);
            println!("name:         {}", p.name);
            println!("type:         {}", p.provider_type);
            println!("source:       {}", p.source);
            println!("has_secret:   {}", p.has_secret);
            println!("created_at:   {}", crate::util::format_date(&p.created_at));
            println!("updated_at:   {}", crate::util::format_date(&p.updated_at));
            println!(
                "config:       {}",
                serde_json::to_string_pretty(&p.config).unwrap_or_default()
            );
        }
        _ => unreachable!(),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    workspace_id: &str,
    name: &str,
    provider_type: &str,
    config: Option<&str>,
    api_key: Option<&str>,
    secret_name: Option<&str>,
    format: &str,
) {
    use crossterm::style::Stylize;

    let api = ApiClient::new(Some(workspace_id));
    let mut body = serde_json::json!({
        "name": name,
        "provider_type": provider_type,
    });
    if let Some(cfg) = parse_config(config) {
        body["config"] = cfg;
    }
    if let Some(k) = api_key {
        body["api_key"] = serde_json::json!(k);
    }
    if let Some(s) = secret_name {
        body["secret_name"] = serde_json::json!(s);
    }

    let (status, resp_body) = api.post_raw("/embedding-providers", &body);
    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let parsed: serde_json::Value = serde_json::from_str(&resp_body).unwrap_or_default();
    eprintln!("{}", "Embedding provider created.".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&parsed).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&parsed).unwrap()),
        "table" => {
            println!("id:    {}", parsed["id"].as_str().unwrap_or(""));
            println!("name:  {}", parsed["name"].as_str().unwrap_or(""));
            println!(
                "type:  {}",
                parsed["provider_type"].as_str().unwrap_or("")
            );
        }
        _ => unreachable!(),
    }
}

pub fn update(
    workspace_id: &str,
    id: &str,
    name: Option<&str>,
    config: Option<&str>,
    api_key: Option<&str>,
    secret_name: Option<&str>,
    format: &str,
) {
    use crossterm::style::Stylize;

    if name.is_none() && config.is_none() && api_key.is_none() && secret_name.is_none() {
        eprintln!(
            "{}",
            "error: provide at least one of --name, --config, --api-key, --secret-name.".red()
        );
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));
    let mut body = serde_json::json!({});
    if let Some(n) = name {
        body["name"] = serde_json::json!(n);
    }
    if let Some(cfg) = parse_config(config) {
        body["config"] = cfg;
    }
    if let Some(k) = api_key {
        body["api_key"] = serde_json::json!(k);
    }
    if let Some(s) = secret_name {
        body["secret_name"] = serde_json::json!(s);
    }

    let resp: serde_json::Value = api.put(&format!("/embedding-providers/{id}"), &body);
    eprintln!("{}", "Embedding provider updated.".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&resp).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&resp).unwrap()),
        "table" => {
            println!("id:          {}", resp["id"].as_str().unwrap_or(""));
            println!("name:        {}", resp["name"].as_str().unwrap_or(""));
            if let Some(updated_at) = resp["updated_at"].as_str() {
                println!("updated_at:  {}", crate::util::format_date(updated_at));
            }
        }
        _ => unreachable!(),
    }
}

pub fn delete(workspace_id: &str, id: &str) {
    use crossterm::style::Stylize;
    let api = ApiClient::new(Some(workspace_id));
    let (status, resp_body) = api.delete_raw(&format!("/embedding-providers/{id}"));
    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }
    println!("{}", format!("Embedding provider '{id}' deleted.").green());
}
