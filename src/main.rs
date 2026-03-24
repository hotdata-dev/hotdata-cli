mod auth;
mod command;
mod config;
mod connections;
mod connections_new;
mod datasets;
mod query;
mod results;
mod skill;
mod table;
mod tables;
mod util;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{AuthCommands, Commands, ConnectionsCommands, ConnectionsCreateCommands, DatasetsCommands, ResultsCommands, SkillCommands, TablesCommands, WorkspaceCommands};

#[derive(Parser)]
#[command(name = "hotdata", version, about = concat!("Hotdata CLI - Command line interface for Hotdata (v", env!("CARGO_PKG_VERSION"), ")"), long_about = None, disable_version_flag = true)]
#[command(styles=get_styles())]
struct Cli {
    /// Print version
    #[arg(short = 'v', short_aliases = ['V'], long, action = clap::ArgAction::Version)]
    version: Option<bool>,

    /// API key (overrides env var and config file)
    #[arg(long, global = true)]
    api_key: Option<String>,

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

    if let Some(key) = cli.api_key {
        config::set_api_key_flag(key);
    }

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
        Some(cmd) => match cmd {
            Commands::Auth { command } => match command {
                None => auth::login(),
                Some(AuthCommands::Status) => auth::status("default"),
                Some(AuthCommands::Logout) => auth::logout("default"),
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
                        Some(DatasetsCommands::Create { label, table_name, file, upload_id, format, sql, query_id }) => {
                            if let Some(sql) = sql {
                                datasets::create_from_query(&workspace_id, &sql, label.as_deref(), table_name.as_deref())
                            } else if let Some(query_id) = query_id {
                                datasets::create_from_saved_query(&workspace_id, &query_id, label.as_deref(), table_name.as_deref())
                            } else {
                                datasets::create_from_upload(&workspace_id, label.as_deref(), table_name.as_deref(), file.as_deref(), upload_id.as_deref(), &format)
                            }
                        }
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("datasets").unwrap().print_help().unwrap();
                        }
                    }
                }
            }
            Commands::Query { sql, workspace_id, connection, format } => {
                let workspace_id = resolve_workspace(workspace_id);
                query::execute(&sql, &workspace_id, connection.as_deref(), &format)
            }
            Commands::Workspaces { command } => match command {
                WorkspaceCommands::List { format } => workspace::list(&format),
                WorkspaceCommands::Set { workspace_id } => workspace::set(workspace_id.as_deref()),
                _ => eprintln!("not yet implemented"),
            },
            Commands::Connections { workspace_id, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    ConnectionsCommands::New => connections_new::run(&workspace_id),
                    ConnectionsCommands::List { format } => {
                        connections::list(&workspace_id, &format)
                    }
                    ConnectionsCommands::Create { command, name, source_type, config, format } => {
                        match command {
                            Some(ConnectionsCreateCommands::List { name, format }) => {
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
                                connections::create(
                                    &workspace_id,
                                    &name.unwrap(),
                                    &source_type.unwrap(),
                                    &config.unwrap(),
                                    &format,
                                )
                            }
                        }
                    }
                    ConnectionsCommands::Refresh { connection_id } => {
                        connections::refresh(&workspace_id, &connection_id)
                    }
                    _ => eprintln!("not yet implemented"),
                }
            },
            Commands::Tables { command } => match command {
                TablesCommands::List { workspace_id, connection_id, schema, table, limit, cursor, format } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    tables::list(&workspace_id, connection_id.as_deref(), schema.as_deref(), table.as_deref(), limit, cursor.as_deref(), &format)
                }
            },
            Commands::Skills { command } => match command {
                SkillCommands::Install { project } => {
                    if project { skill::install_project() } else { skill::install() }
                }
                SkillCommands::Status => skill::status(),
            },
            Commands::Results { result_id, workspace_id, format, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(ResultsCommands::List { limit, offset, format }) => {
                        results::list(&workspace_id, limit, offset, &format)
                    }
                    None => {
                        match result_id {
                            Some(id) => results::get(&id, &workspace_id, &format),
                            None => {
                                use clap::CommandFactory;
                                let mut cmd = Cli::command();
                                cmd.build();
                                cmd.find_subcommand_mut("results").unwrap().print_help().unwrap();
                            }
                        }
                    }
                }
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
