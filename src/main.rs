mod auth;
mod command;
mod config;
mod init;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{AuthCommands, Commands, WorkspaceCommands};

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
            Commands::Query { .. } => eprintln!("not yet implemented"),
            Commands::Profile { .. } => eprintln!("not yet implemented"),
            Commands::Workspace { command } => match command {
                WorkspaceCommands::List { format } => workspace::list(&format),
                _ => eprintln!("not yet implemented"),
            },
            Commands::Connections { .. } => eprintln!("not yet implemented"),
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
