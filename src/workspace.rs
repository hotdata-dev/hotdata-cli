use crate::api::ApiClient;
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

pub fn set(workspace_id: Option<&str>) {
    if std::env::var("HOTDATA_WORKSPACE").is_ok() || crate::sessions::find_session_run_ancestor().is_some() {
        eprintln!("error: workspace is locked");
        std::process::exit(1);
    }
    let api = ApiClient::new(None);
    let body: ListResponse = api.get("/workspaces");
    let workspaces = body.workspaces;

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
    let default_id = std::env::var("HOTDATA_WORKSPACE")
        .unwrap_or_else(|_| profile_config.workspaces.first().map(|w| w.public_id.clone()).unwrap_or_default());

    let api = ApiClient::new(None);
    let body: ListResponse = api.get("/workspaces");

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
