mod command;

use clap::Parser;
use command::Commands;

#[derive(Parser)]
#[command(name = "hotdata", version, about = concat!("HotData CLI - Command line interface for HotData (v", env!("CARGO_PKG_VERSION"), ")"), long_about = None, disable_version_flag = true)]
struct Cli {
    /// Print version
    #[arg(short = 'v', short_aliases = ['V'], long, action = clap::ArgAction::Version)]
    version: Option<bool>,

    #[command(subcommand)]
    command: Option<Commands>,
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
        Some(cmd) => match cmd {
            Commands::Init => eprintln!("not yet implemented"),
            Commands::Info => eprintln!("not yet implemented"),
            Commands::Auth { .. } => eprintln!("not yet implemented"),
            Commands::Datasets { .. } => eprintln!("not yet implemented"),
            Commands::Query { .. } => eprintln!("not yet implemented"),
            Commands::Profile { .. } => eprintln!("not yet implemented"),
            Commands::Workspace { .. } => eprintln!("not yet implemented"),
            Commands::Connections { .. } => eprintln!("not yet implemented"),
        },
    }
}
