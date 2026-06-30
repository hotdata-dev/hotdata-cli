use crate::client::sdk::Api;
use crate::config;
use serde::Serialize;

#[derive(Serialize)]
struct Workspace {
    public_id: String,
    name: String,
    active: bool,
    favorite: bool,
    provision_status: String,
}

impl From<&hotdata::models::WorkspaceListItem> for Workspace {
    fn from(w: &hotdata::models::WorkspaceListItem) -> Self {
        Workspace {
            public_id: w.public_id.clone(),
            name: w.name.clone(),
            active: w.active,
            favorite: w.favorite,
            provision_status: w.provision_status.clone(),
        }
    }
}

fn fetch_workspaces() -> Vec<Workspace> {
    let api = Api::new(None);
    let body = api.list_workspaces(None).unwrap_or_else(|e| e.exit());
    body.workspaces.iter().map(Workspace::from).collect()
}

pub fn set(workspace_id: Option<&str>) {
    let workspaces = fetch_workspaces();

    let chosen = match workspace_id {
        Some(id) => match workspaces.iter().find(|w| w.public_id == id) {
            Some(w) => config::WorkspaceEntry {
                public_id: w.public_id.clone(),
                name: w.name.clone(),
            },
            None => {
                eprintln!("error: workspace '{id}' not found or you don't have access to it.");
                std::process::exit(1);
            }
        },
        None => {
            if workspaces.is_empty() {
                eprintln!("error: no workspaces available.");
                std::process::exit(1);
            }
            if !crate::util::is_interactive() {
                eprintln!(
                    "error: stdin is not a TTY; cannot prompt for selection. \
                     Run 'hotdata workspaces list' to see available IDs, \
                     then 'hotdata workspaces set <workspace_id>'."
                );
                std::process::exit(1);
            }
            let options: Vec<String> = workspaces
                .iter()
                .map(|w| format!("{} ({})", w.name, w.public_id))
                .collect();
            let selection =
                match inquire::Select::new("Select default workspace:", options.clone()).prompt() {
                    Ok(s) => s,
                    Err(_) => std::process::exit(1),
                };
            let idx = options.iter().position(|o| o == &selection).unwrap();
            let w = &workspaces[idx];
            config::WorkspaceEntry {
                public_id: w.public_id.clone(),
                name: w.name.clone(),
            }
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
    let default_id = std::env::var("HOTDATA_WORKSPACE").unwrap_or_else(|_| {
        profile_config
            .workspaces
            .first()
            .map(|w| w.public_id.clone())
            .unwrap_or_default()
    });

    let workspaces = fetch_workspaces();

    match format {
        "json" => {
            println!("{}", serde_json::to_string_pretty(&workspaces).unwrap());
        }
        "yaml" => {
            print!("{}", serde_yaml::to_string(&workspaces).unwrap());
        }
        "table" => {
            if workspaces.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No workspaces found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = workspaces
                    .iter()
                    .map(|w| {
                        let marker = if w.public_id == default_id { "*" } else { "" };
                        vec![
                            marker.to_string(),
                            w.public_id.clone(),
                            w.name.clone(),
                            w.provision_status.clone(),
                        ]
                    })
                    .collect();
                crate::output::table::print(
                    &["DEFAULT", "PUBLIC_ID", "NAME", "PROVISION_STATUS"],
                    &rows,
                );
            }
        }
        _ => unreachable!(),
    }
}
