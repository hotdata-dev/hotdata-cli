use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Workspace {
    public_id: String,
    name: String,
    active: bool,
    favorite: bool,
    provision_status: String,
}

#[derive(Deserialize)]
struct ListResponse {
    workspaces: Vec<Workspace>,
}

pub fn list(format: &str) {
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

    let url = format!("{}/workspaces", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!("error: HTTP {}", resp.status());
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
            println!("{}", serde_json::to_string_pretty(&body.workspaces).unwrap());
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&body.workspaces).unwrap());
        }
        "table" => {
            println!(
                "{:<30}  {:<30}  {:<8}  {:<10}  {}",
                "PUBLIC_ID", "NAME", "ACTIVE", "FAVORITE", "PROVISION_STATUS"
            );
            println!("{}", "-".repeat(90));
            for w in &body.workspaces {
                println!(
                    "{:<30}  {:<30}  {:<8}  {:<10}  {}",
                    w.public_id, w.name, w.active, w.favorite, w.provision_status
                );
            }
        }
        _ => unreachable!(),
    }
}
