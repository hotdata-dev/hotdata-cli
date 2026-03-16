mod auth;
mod command;
mod config;
mod connections;
mod datasets;
mod init;
mod query;
mod results;
mod skill;
mod tables;
mod util;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{AuthCommands, Commands, ConnectionsCommands, ConnectionsCreateCommands, DatasetsCommands, SkillCommands, TablesCommands, WorkspaceCommands};

#[derive(Parser)]
#[command(name = "hotdata", version, about = concat!("HotData CLI - Command line interface for HotData (v", env!("CARGO_PKG_VERSION"), ")"), long_about = None, disable_version_flag = true)]
#[command(styles=get_styles())]
struct Cli {
    /// Print version
    #[arg(short = 'v', short_aliases = ['V'], long, action = clap::ArgAction::Version)]
    version: Option<bool>,

    #[command(subcommand)]
    command: Option<Commands>,
}

fn resolve_workspace(provided: Option<String>) -> String {
    match config::load("default") {
        Ok(profile) => match config::resolve_workspace_id(provided, &profile) {
            Ok(id) => id,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
        Some(cmd) => match cmd {
            Commands::Init => init::run(),
            Commands::Info => eprintln!("not yet implemented"),
            Commands::Auth { command } => match command {
                AuthCommands::Login => auth::login(),
                AuthCommands::Status { profile } => auth::status(&profile),
                _ => eprintln!("not yet implemented"),
            },
            Commands::Datasets { id, workspace_id, format, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    datasets::get(&id, &workspace_id, &format)
                } else {
                    match command {
                        Some(DatasetsCommands::List { limit, offset, format }) => {
                            datasets::list(&workspace_id, limit, offset, &format)
                        }
                        Some(DatasetsCommands::Create { label, table_name, file, upload_id, format }) => {
                            datasets::create(&workspace_id, label.as_deref(), table_name.as_deref(), file.as_deref(), upload_id.as_deref(), &format)
                        }
                        None => {
                            use clap::CommandFactory;
                            Cli::command().find_subcommand_mut("datasets").unwrap().print_help().unwrap();
                            println!();
                        }
                    }
                }
            }
            Commands::Query { sql, workspace_id, connection, format } => {
                let workspace_id = resolve_workspace(workspace_id);
                query::execute(&sql, &workspace_id, connection.as_deref(), &format)
            }
            Commands::Profile { .. } => eprintln!("not yet implemented"),
            Commands::Workspaces { command } => match command {
                WorkspaceCommands::List { format } => workspace::list(&format),
                _ => eprintln!("not yet implemented"),
            },
            Commands::Connections { command } => match command {
                ConnectionsCommands::Create { command, workspace_id, name, source_type, config, secret_id, secret_name, format } => {
                    match command {
                        Some(ConnectionsCreateCommands::List { name, workspace_id, format }) => {
                            let workspace_id = resolve_workspace(workspace_id);
                            match name.as_deref() {
                                Some(name) => connections::types_get(&workspace_id, name, &format),
                                None => connections::types_list(&workspace_id, &format),
                            }
                        }
                        None => {
                            let missing: Vec<&str> = [
                                name.is_none().then_some("--name"),
                                source_type.is_none().then_some("--type"),
                                config.is_none().then_some("--config"),
                            ].into_iter().flatten().collect();
                            if !missing.is_empty() {
                                eprintln!("error: missing required arguments: {}", missing.join(", "));
                                std::process::exit(1);
                            }
                            let workspace_id = resolve_workspace(workspace_id);
                            connections::create(
                                &workspace_id,
                                &name.unwrap(),
                                &source_type.unwrap(),
                                &config.unwrap(),
                                secret_id.as_deref(),
                                secret_name.as_deref(),
                                &format,
                            )
                        }
                    }
                },
                ConnectionsCommands::List { workspace_id, format } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    connections::list(&workspace_id, &format)
                }
                _ => eprintln!("not yet implemented"),
            },
            Commands::Tables { command } => match command {
                TablesCommands::List { workspace_id, connection_id, schema, table, limit, cursor, format } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    tables::list(&workspace_id, connection_id.as_deref(), schema.as_deref(), table.as_deref(), limit, cursor.as_deref(), &format)
                }
            },
            Commands::Skill { command } => match command {
                SkillCommands::Install { project } => {
                    if project { skill::install_project() } else { skill::install() }
                }
                SkillCommands::Status => skill::status(),
            },
            Commands::Results { result_id, workspace_id, format } => {
                let workspace_id = resolve_workspace(workspace_id);
                results::get(&result_id, &workspace_id, &format)
            }
        },
    }
}

pub fn get_styles() -> clap::builder::Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}
