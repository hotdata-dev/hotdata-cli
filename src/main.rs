mod auth;
mod command;
mod config;
mod connections;
mod init;
mod query;
mod results;
mod tables;
mod util;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{AuthCommands, Commands, ConnectionsCommands, TablesCommands, WorkspaceCommands};

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
            Commands::Datasets { .. } => eprintln!("not yet implemented"),
            Commands::Query { sql, workspace_id, connection, format } => {
                query::execute(&sql, &workspace_id, connection.as_deref(), &format)
            }
            Commands::Profile { .. } => eprintln!("not yet implemented"),
            Commands::Workspace { command } => match command {
                WorkspaceCommands::List { format } => workspace::list(&format),
                _ => eprintln!("not yet implemented"),
            },
            Commands::Connections { command } => match command {
                ConnectionsCommands::List { workspace_id, format } => {
                    connections::list(&workspace_id, &format)
                }
                _ => eprintln!("not yet implemented"),
            },
            Commands::Tables { command } => match command {
                TablesCommands::List { workspace_id, connection_id, format } => {
                    tables::list(&workspace_id, connection_id.as_deref(), &format)
                }
            },
            Commands::Results { result_id, workspace_id, format } => results::get(&result_id, &workspace_id, &format),
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
