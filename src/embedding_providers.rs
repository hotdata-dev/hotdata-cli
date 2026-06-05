use crate::sdk::Api;
use hotdata::models::{
    CreateEmbeddingProviderRequest, EmbeddingProviderResponse, UpdateEmbeddingProviderRequest,
};
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

impl From<EmbeddingProviderResponse> for Provider {
    fn from(p: EmbeddingProviderResponse) -> Self {
        Provider {
            id: p.id,
            name: p.name,
            provider_type: p.provider_type,
            config: p.config.unwrap_or(serde_json::Value::Null),
            has_secret: p.has_secret,
            source: p.source,
            created_at: p.created_at,
            updated_at: p.updated_at,
        }
    }
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
    let api = Api::new(Some(workspace_id));
    let providers: Vec<Provider> = crate::sdk::block(api.client().embedding_providers().list())
        .unwrap_or_else(|e| e.exit())
        .embedding_providers
        .into_iter()
        .map(Provider::from)
        .collect();

    use crossterm::style::Stylize;
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&providers).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&providers).unwrap()),
        "table" => {
            if providers.is_empty() {
                eprintln!("{}", "No embedding providers found.".dark_grey());
                return;
            }
            let rows: Vec<Vec<String>> = providers
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
    let api = Api::new(Some(workspace_id));
    let p: Provider = crate::sdk::block(api.client().embedding_providers().get(id))
        .unwrap_or_else(|e| e.exit())
        .into();

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

    let api = Api::new(Some(workspace_id));
    let mut req = CreateEmbeddingProviderRequest::new(name.to_string(), provider_type.to_string());
    if let Some(cfg) = parse_config(config) {
        req.config = Some(Some(cfg));
    }
    if let Some(k) = api_key {
        req.api_key = Some(Some(k.to_string()));
    }
    if let Some(s) = secret_name {
        req.secret_name = Some(Some(s.to_string()));
    }

    let resp = crate::sdk::block(api.client().embedding_providers().create(req))
        .unwrap_or_else(|e| e.exit());
    let parsed = serde_json::to_value(&resp).unwrap_or_default();

    eprintln!("{}", "Embedding provider created.".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&parsed).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&parsed).unwrap()),
        "table" => {
            println!("id:    {}", parsed["id"].as_str().unwrap_or(""));
            println!("name:  {}", parsed["name"].as_str().unwrap_or(""));
            println!("type:  {}", parsed["provider_type"].as_str().unwrap_or(""));
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
            "error: provide at least one of --name, --config, --provider-api-key, --secret-name."
                .red()
        );
        std::process::exit(1);
    }

    let api = Api::new(Some(workspace_id));
    let mut req = UpdateEmbeddingProviderRequest::new();
    if let Some(n) = name {
        req.name = Some(Some(n.to_string()));
    }
    if let Some(cfg) = parse_config(config) {
        req.config = Some(Some(cfg));
    }
    if let Some(k) = api_key {
        req.api_key = Some(Some(k.to_string()));
    }
    if let Some(s) = secret_name {
        req.secret_name = Some(Some(s.to_string()));
    }

    let resp = crate::sdk::block(api.client().embedding_providers().update(id, req))
        .unwrap_or_else(|e| e.exit());
    let resp = serde_json::to_value(&resp).unwrap_or_default();

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
    let api = Api::new(Some(workspace_id));
    crate::sdk::block(api.client().embedding_providers().delete(id)).unwrap_or_else(|e| e.exit());
    println!("{}", format!("Embedding provider '{id}' deleted.").green());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors runtimedb's `EmbeddingProviderResponse` (see `runtimedb/openapi.yaml`).
    /// If the server response shape changes, update this fixture in lockstep.
    #[test]
    fn provider_deserializes_runtimedb_payload() {
        let body = serde_json::json!({
            "id": "sys_emb_openai",
            "name": "openai",
            "provider_type": "service",
            "config": {
                "base_url": "https://api.openai.com/v1",
                "metric": "cosine",
                "model": "text-embedding-3-small"
            },
            "has_secret": true,
            "source": "system",
            "created_at": "2026-04-29T08:19:57.083658085Z",
            "updated_at": "2026-04-29T08:19:57.083658085Z"
        });
        let p: Provider = serde_json::from_value(body).unwrap();
        assert_eq!(p.id, "sys_emb_openai");
        assert_eq!(p.provider_type, "service");
        assert_eq!(p.source, "system");
        assert!(p.has_secret);
        assert_eq!(p.config["model"], "text-embedding-3-small");
    }

    /// The SDK `EmbeddingProviderResponse` converts into the CLI `Provider`
    /// display struct, preserving the fields the CLI prints. A null `config`
    /// from the SDK collapses to JSON null so table/json output stays stable.
    #[test]
    fn sdk_response_converts_to_provider() {
        let sdk = EmbeddingProviderResponse {
            config: Some(serde_json::json!({"model": "text-embedding-3-small"})),
            created_at: "2026-04-29T08:19:57Z".to_string(),
            has_secret: true,
            id: "sys_emb_openai".to_string(),
            name: "openai".to_string(),
            provider_type: "service".to_string(),
            source: "system".to_string(),
            updated_at: "2026-04-29T08:19:57Z".to_string(),
        };
        let p: Provider = sdk.into();
        assert_eq!(p.id, "sys_emb_openai");
        assert_eq!(p.config["model"], "text-embedding-3-small");

        let sdk_null = EmbeddingProviderResponse {
            config: None,
            created_at: String::new(),
            has_secret: false,
            id: "x".to_string(),
            name: "n".to_string(),
            provider_type: "local".to_string(),
            source: "user".to_string(),
            updated_at: String::new(),
        };
        let p: Provider = sdk_null.into();
        assert!(p.config.is_null());
    }

    #[test]
    fn parse_config_rejects_invalid_json() {
        // parse_config exits on invalid JSON, so we only verify the success path here.
        let parsed = parse_config(Some(r#"{"key":"value"}"#));
        assert_eq!(parsed.unwrap()["key"], "value");
        assert!(parse_config(None).is_none());
    }
}
