use crate::client::sdk::Api;
use crate::config;
use serde::Serialize;

/// Subcommands for `hotdata workspaces`.
#[derive(clap::Subcommand)]
pub enum WorkspaceCommands {
    /// List all workspaces
    List {
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Set the default workspace
    #[command(name = "use")]
    Set {
        /// Workspace ID to set as default (omit for interactive selection)
        workspace_id: Option<String>,
    },
}

#[derive(Serialize)]
struct Workspace {
    public_id: String,
    name: String,
    /// True for the workspace commands act on by default (the one `workspaces
    /// use` selected, or `HOTDATA_WORKSPACE`). Not to be confused with
    /// `active`, a server-side workspace-state flag that is true for every
    /// usable workspace — scripts looking for "the current workspace" need
    /// this field, and before it existed the JSON offered nothing.
    default: bool,
    active: bool,
    favorite: bool,
    provision_status: String,
}

impl From<&hotdata::models::WorkspaceListItem> for Workspace {
    fn from(w: &hotdata::models::WorkspaceListItem) -> Self {
        Workspace {
            public_id: w.public_id.clone(),
            name: w.name.clone(),
            // Stamped by fetch_workspaces, which knows the configured default.
            default: false,
            active: w.active,
            favorite: w.favorite,
            provision_status: w.provision_status.clone(),
        }
    }
}

/// The workspace commands act on when none is passed: `HOTDATA_WORKSPACE`,
/// else the front of the configured list (`workspaces use` moves its pick
/// there).
fn default_workspace_id() -> String {
    std::env::var("HOTDATA_WORKSPACE").unwrap_or_else(|_| {
        config::load("default")
            .ok()
            .and_then(|c| c.workspaces.first().map(|w| w.public_id.clone()))
            .unwrap_or_default()
    })
}

fn fetch_workspaces() -> Vec<Workspace> {
    let api = Api::new(None);
    let body = api.list_workspaces(None).unwrap_or_else(|e| e.exit());
    let default_id = default_workspace_id();
    body.workspaces
        .iter()
        .map(|w| {
            let mut ws = Workspace::from(w);
            ws.default = !default_id.is_empty() && ws.public_id == default_id;
            ws
        })
        .collect()
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
                     then 'hotdata workspaces use <workspace_id>'."
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
                        let marker = if w.default { "*" } else { "" };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_json_exposes_default_flag() {
        // `active` is a server workspace-state flag (true for every usable
        // workspace); `default` is the one the CLI acts on. Scripts need the
        // latter in `-o json` — the table view's DEFAULT column has no JSON
        // counterpart otherwise.
        let ws = Workspace {
            public_id: "work123".to_string(),
            name: "Default Workspace".to_string(),
            default: true,
            active: true,
            favorite: false,
            provision_status: "success".to_string(),
        };
        let json = serde_json::to_value(&ws).unwrap();
        assert_eq!(json["default"], serde_json::json!(true));
        assert_eq!(json["active"], serde_json::json!(true));
    }

    #[test]
    fn from_list_item_defaults_to_not_default() {
        // The wire item carries no default marker — it's stamped from local
        // config by fetch_workspaces, so the raw mapping must start false.
        let item = hotdata::models::WorkspaceListItem::new(
            "wid".to_string(),
            "n".to_string(),
            true,
            false,
            "success".to_string(),
        );
        assert!(!Workspace::from(&item).default);
    }
}
