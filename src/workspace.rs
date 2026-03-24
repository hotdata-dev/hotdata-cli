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

fn load_client() -> (reqwest::blocking::Client, String, String) {
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
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };
    let api_url = profile_config.api_url.to_string();
    (reqwest::blocking::Client::new(), api_key, api_url)
}

fn fetch_all_workspaces(client: &reqwest::blocking::Client, api_key: &str, api_url: &str) -> Vec<Workspace> {
    let url = format!("{api_url}/workspaces");
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
        eprintln!("error: {}", crate::util::api_error(resp.text().unwrap_or_default()));
        std::process::exit(1);
    }
    match resp.json::<ListResponse>() {
        Ok(b) => b.workspaces,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    }
}

pub fn set(workspace_id: Option<&str>) {
    let (client, api_key, api_url) = load_client();
    let workspaces = fetch_all_workspaces(&client, &api_key, &api_url);

    let chosen = match workspace_id {
        Some(id) => {
            match workspaces.iter().find(|w| w.public_id == id) {
                Some(w) => config::WorkspaceEntry { public_id: w.public_id.clone(), name: w.name.clone() },
                None => {
                    eprintln!("error: workspace '{id}' not found or you don't have access to it.");
                    std::process::exit(1);
                }
            }
        }
        None => {
            if workspaces.is_empty() {
                eprintln!("error: no workspaces available.");
                std::process::exit(1);
            }
            let options: Vec<String> = workspaces.iter()
                .map(|w| format!("{} ({})", w.name, w.public_id))
                .collect();
            let selection = match inquire::Select::new("Select default workspace:", options.clone()).prompt() {
                Ok(s) => s,
                Err(_) => std::process::exit(1),
            };
            let idx = options.iter().position(|o| o == &selection).unwrap();
            let w = &workspaces[idx];
            config::WorkspaceEntry { public_id: w.public_id.clone(), name: w.name.clone() }
        }
    };

    if let Err(e) = config::save_default_workspace("default", chosen.clone()) {
        eprintln!("error saving config: {e}");
        std::process::exit(1);
    }

    use crossterm::style::Stylize;
    println!("{}", "Default workspace updated".green());
    println!("id:   {}", chosen.public_id);
    println!("name: {}", chosen.name);
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
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };

    let default_id = profile_config.workspaces.first().map(|w| w.public_id.as_str()).unwrap_or("").to_string();

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
        eprintln!("error: {}", crate::util::api_error(resp.text().unwrap_or_default()));
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
            if body.workspaces.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No workspaces found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.workspaces.iter().map(|w| {
                    let marker = if w.public_id == default_id { "*" } else { "" };
                    vec![marker.to_string(), w.public_id.clone(), w.name.clone(), w.provision_status.clone()]
                }).collect();
                crate::table::print(&["DEFAULT", "PUBLIC_ID", "NAME", "PROVISION_STATUS"], &rows);
            }
        }
        _ => unreachable!(),
    }
}
