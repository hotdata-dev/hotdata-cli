mod cli;
mod client;
mod commands;
mod config;
mod output;
mod util;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use cli::Commands;
use client::credentials;
use commands::auth::{self, AuthCommands};
use commands::context::{self, ContextCommands};
use commands::databases::{self, DatabaseTablesCommands, DatabasesCommands};
use commands::ingest;
use commands::jobs::{self, JobsCommands};
use commands::queries::{self, QueriesCommands};
use commands::query::{self, QueryCommands};
use commands::results::{self, ResultsCommands};
use commands::skill::{self, SkillCommands};
use commands::tables;
use commands::workspace::{self, WorkspaceCommands};
use commands::{update, usage};

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

    /// Print verbose API request and response details
    #[arg(long, global = true, hide = true)]
    debug: bool,

    /// Disable interactive prompts; commands that need input will error instead
    #[arg(long = "no-input", global = true)]
    no_input: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Set once after workspace resolution so the database footer can reference it
/// without re-doing config I/O.
static ACTIVE_WORKSPACE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn resolve_workspace(provided: Option<String>) -> String {
    // HOTDATA_WORKSPACE env var takes priority and blocks --workspace-id flag
    if let Ok(ws) = std::env::var("HOTDATA_WORKSPACE") {
        if let Some(ref flag) = provided
            && flag != &ws
        {
            eprintln!(
                "error: cannot override workspace -- locked by HOTDATA_WORKSPACE environment variable ({ws})"
            );
            std::process::exit(1);
        }
        let _ = ACTIVE_WORKSPACE_ID.set(ws.clone());
        return ws;
    }
    let profile = config::load("default").unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    // An explicit --workspace-id always wins.
    if let Some(id) = provided {
        let _ = ACTIVE_WORKSPACE_ID.set(id.clone());
        return id;
    }
    // Otherwise the profile's default, computed by the same helper `auth
    // status` displays — so the reported workspace is the one commands hit.
    // For an api-key credential that's its own authorized workspace (a database
    // token's sole one, or the saved default when the key can reach it), not a
    // possibly-different CLI-session cache.
    match credentials::default_workspace_id(&profile) {
        Some(id) => {
            let _ = ACTIVE_WORKSPACE_ID.set(id.clone());
            id
        }
        None => {
            eprintln!(
                "error: no workspace-id provided and no default workspace found. \
                 Run 'hotdata auth login' or specify --workspace-id."
            );
            std::process::exit(1);
        }
    }
}

// libc::atexit (no extra crate needed — the symbol is linked by default).
// Callbacks registered here fire even when subcommands call
// `std::process::exit`, which Rust's `Drop` would otherwise miss.
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> i32;
}

extern "C" fn print_database_footer() {
    use crossterm::style::Stylize;
    use std::io::IsTerminal;
    // Human convenience only — stay quiet for piped/redirected/scripted
    // callers (who may capture stderr alongside machine output) so the footer
    // never mixes into their stream.
    if !std::io::stdout().is_terminal() {
        return;
    }
    if let Some(ws_id) = ACTIVE_WORKSPACE_ID.get()
        && let Some(id) = config::load_current_database("default", ws_id)
    {
        eprintln!(
            "{}",
            format!("current database: {id}  use 'hotdata databases use' to change").dark_grey(),
        );
    }
}

fn main() {
    // Register before `Cli::parse`, since `--help` / `--version` exit
    // from inside the parser. Safety: `atexit` is async-signal-safe;
    // the callback only reads env vars / files and writes to stderr.
    unsafe { atexit(print_database_footer) };

    dotenvy::dotenv().ok();
    let cli = Cli::parse();

    if let Some(key) = cli.api_key {
        config::set_api_key_flag(key);
    }
    if cli.debug {
        util::set_debug(true);
    }
    if cli.no_input {
        util::set_no_input(true);
    }

    let skip_skill_auto_update = cli.command.is_none()
        || matches!(
            &cli.command,
            Some(Commands::Manage {
                command: cli::ManageCommands::Skills { .. }
            })
        );
    if !skip_skill_auto_update {
        skill::maybe_auto_update_after_cli_upgrade();
    }

    // A newer release may be incompatible with the API, so gate API-touching
    // commands behind an up-to-date check: prompt to upgrade and, on decline
    // or a failed upgrade, exit *without* running the command. Exempt the
    // commands that don't hit the API (bare help, completions) and the
    // upgrader itself. No-op for non-interactive/CI sessions, so automation is
    // never blocked (see `update::should_check`).
    let gate_update = !matches!(
        &cli.command,
        None | Some(Commands::Auth { command: None })
            | Some(Commands::Manage {
                command: cli::ManageCommands::Completions { .. } | cli::ManageCommands::Upgrade,
            })
    );
    if gate_update {
        update::enforce_latest_or_exit();
    }

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
        Some(cmd) => match cmd {
            Commands::Auth { command } => match command {
                Some(AuthCommands::Login) => auth::login(),
                Some(AuthCommands::Register { email }) => auth::register(email),
                Some(AuthCommands::Status) => auth::status("default"),
                Some(AuthCommands::Logout) => auth::logout("default"),
                None => {
                    use clap::CommandFactory;
                    let mut cmd = Cli::command();
                    cmd.build();
                    cmd.find_subcommand_mut("auth")
                        .unwrap()
                        .print_help()
                        .unwrap();
                }
            },
            Commands::Query {
                sql,
                workspace_id,
                database,
                dialect,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(QueryCommands::Status { id }) => {
                        query::poll(&id, &workspace_id, database.as_deref(), &output)
                    }
                    None => match sql {
                        Some(sql) => query::execute(
                            &sql,
                            &workspace_id,
                            database.as_deref(),
                            &output,
                            &dialect,
                        ),
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("query")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    },
                }
            }
            Commands::Workspaces { command } => match command {
                WorkspaceCommands::List { output } => workspace::list(&output),
                WorkspaceCommands::Set { workspace_id } => workspace::set(workspace_id.as_deref()),
            },
            Commands::Databases {
                name_or_id,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(name_or_id) = name_or_id {
                    databases::get(&workspace_id, &name_or_id, &output);
                } else {
                    match command {
                        Some(DatabasesCommands::List {
                            output,
                            limit,
                            cursor,
                        }) => databases::list(&workspace_id, &output, limit, cursor.as_deref()),
                        Some(DatabasesCommands::Count { output }) => {
                            databases::count(&workspace_id, &output)
                        }
                        Some(DatabasesCommands::Show { name_or_id, output }) => {
                            databases::get(&workspace_id, &name_or_id, &output)
                        }
                        Some(DatabasesCommands::Create {
                            name,
                            catalog,
                            schema,
                            tables,
                            expires_at,
                            attach,
                            output,
                        }) => databases::create(
                            &workspace_id,
                            name.as_deref(),
                            catalog.as_deref(),
                            &schema,
                            &tables,
                            expires_at.as_deref(),
                            &attach,
                            &output,
                        ),
                        Some(DatabasesCommands::Fork {
                            database,
                            name,
                            expires_at,
                            output,
                        }) => databases::fork(
                            &workspace_id,
                            database.as_deref(),
                            name.as_deref(),
                            expires_at.as_deref(),
                            &output,
                        ),
                        Some(DatabasesCommands::Attach {
                            catalog,
                            database,
                            alias,
                        }) => databases::attach(
                            &workspace_id,
                            &catalog,
                            database.as_deref(),
                            alias.as_deref(),
                        ),
                        Some(DatabasesCommands::Detach { catalog, database }) => {
                            databases::detach(&workspace_id, &catalog, database.as_deref())
                        }
                        Some(DatabasesCommands::Set { id }) => databases::set(&workspace_id, &id),
                        Some(DatabasesCommands::Unset) => databases::unset(&workspace_id),
                        Some(DatabasesCommands::Delete { name_or_id }) => {
                            databases::delete(&workspace_id, &name_or_id)
                        }
                        Some(DatabasesCommands::Load {
                            catalog,
                            schema,
                            table,
                            file,
                            url,
                            upload_id,
                            result_id,
                            append,
                        }) => databases::tables_load(
                            &workspace_id,
                            Some(catalog.as_str()),
                            &table,
                            Some(schema.as_str()),
                            file.as_deref(),
                            url.as_deref(),
                            upload_id.as_deref(),
                            result_id.as_deref(),
                            append,
                        ),
                        Some(DatabasesCommands::Tables { database, command }) => match command {
                            Some(DatabaseTablesCommands::List {
                                database: db_flag,
                                schema,
                                table,
                                limit,
                                cursor,
                                output,
                            }) => {
                                let db = db_flag.as_deref().or(database.as_deref());
                                // Scope to a database when one is addressable
                                // (flag / group positional / active), else list
                                // every table in the workspace.
                                if db.is_some()
                                    || crate::config::load_current_database(
                                        "default",
                                        &workspace_id,
                                    )
                                    .is_some()
                                {
                                    databases::tables_list(
                                        &workspace_id,
                                        db,
                                        schema.as_deref(),
                                        table.as_deref(),
                                        limit,
                                        cursor.as_deref(),
                                        &output,
                                    )
                                } else {
                                    tables::list(
                                        &workspace_id,
                                        schema.as_deref(),
                                        table.as_deref(),
                                        limit,
                                        cursor.as_deref(),
                                        &output,
                                    )
                                }
                            }
                            Some(DatabaseTablesCommands::Show { table, output }) => {
                                tables::show(&workspace_id, &table, &output)
                            }
                            Some(DatabaseTablesCommands::Load {
                                database: db_flag,
                                table,
                                schema,
                                file,
                                url,
                                upload_id,
                                result_id,
                                append,
                            }) => databases::tables_load(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                &table,
                                Some(schema.as_str()),
                                file.as_deref(),
                                url.as_deref(),
                                upload_id.as_deref(),
                                result_id.as_deref(),
                                append,
                            ),
                            Some(DatabaseTablesCommands::Delete {
                                database: db_flag,
                                table,
                                schema,
                            }) => databases::tables_delete(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                &table,
                                Some(schema.as_str()),
                            ),
                            None => {
                                if let Some(ref db) = database {
                                    databases::tables_list(
                                        &workspace_id,
                                        Some(db.as_str()),
                                        None,
                                        None,
                                        None,
                                        None,
                                        "table",
                                    )
                                } else {
                                    use clap::CommandFactory;
                                    let mut cmd = Cli::command();
                                    cmd.build();
                                    cmd.find_subcommand_mut("databases")
                                        .expect("databases subcommand not found")
                                        .find_subcommand_mut("tables")
                                        .expect("tables subcommand not found")
                                        .print_help()
                                        .expect("failed to print help");
                                }
                            }
                        },
                        Some(DatabasesCommands::Context { database, command }) => {
                            let database_id = database
                                .or_else(|| {
                                    config::load_current_database("default", &workspace_id)
                                })
                                .unwrap_or_else(|| {
                                    eprintln!(
                                        "error: no active database. Pass -d/--database <id> or set one with 'hotdata databases use <id>'."
                                    );
                                    std::process::exit(1);
                                });
                            match command {
                                ContextCommands::List { output, prefix } => context::list(
                                    &workspace_id,
                                    &database_id,
                                    prefix.as_deref(),
                                    &output,
                                ),
                                ContextCommands::Show { name } => {
                                    context::show(&workspace_id, &database_id, &name)
                                }
                                ContextCommands::Pull {
                                    name,
                                    force,
                                    dry_run,
                                } => context::pull(
                                    &workspace_id,
                                    &database_id,
                                    &name,
                                    force,
                                    dry_run,
                                ),
                                ContextCommands::Push { name, dry_run } => {
                                    context::push(&workspace_id, &database_id, &name, dry_run)
                                }
                            }
                        }
                        Some(DatabasesCommands::Query {
                            sql,
                            database,
                            dialect,
                            output,
                            command,
                        }) => match command {
                            Some(QueryCommands::Status { id }) => {
                                query::poll(&id, &workspace_id, database.as_deref(), &output)
                            }
                            None => match sql {
                                Some(sql) => query::execute(
                                    &sql,
                                    &workspace_id,
                                    database.as_deref(),
                                    &output,
                                    &dialect,
                                ),
                                None => {
                                    use clap::CommandFactory;
                                    let mut cmd = Cli::command();
                                    cmd.build();
                                    cmd.find_subcommand_mut("databases")
                                        .unwrap()
                                        .find_subcommand_mut("query")
                                        .unwrap()
                                        .print_help()
                                        .unwrap();
                                }
                            },
                        },
                        Some(DatabasesCommands::Queries {
                            id,
                            database,
                            output,
                            command,
                        }) => {
                            if let Some(id) = id {
                                queries::get(&id, &workspace_id, database.as_deref(), &output)
                            } else {
                                match command {
                                    Some(QueriesCommands::List {
                                        limit,
                                        cursor,
                                        status,
                                        output,
                                    }) => queries::list(
                                        &workspace_id,
                                        database.as_deref(),
                                        Some(limit),
                                        cursor.as_deref(),
                                        status.as_deref(),
                                        &output,
                                    ),
                                    None => {
                                        use clap::CommandFactory;
                                        let mut cmd = Cli::command();
                                        cmd.build();
                                        cmd.find_subcommand_mut("databases")
                                            .unwrap()
                                            .find_subcommand_mut("queries")
                                            .unwrap()
                                            .print_help()
                                            .unwrap();
                                    }
                                }
                            }
                        }
                        Some(DatabasesCommands::Results {
                            result_id,
                            database,
                            output,
                            command,
                        }) => match command {
                            Some(ResultsCommands::Show { id, output }) => {
                                results::get(&id, &workspace_id, database.as_deref(), &output)
                            }
                            Some(ResultsCommands::List {
                                limit,
                                offset,
                                output,
                            }) => results::list(
                                &workspace_id,
                                database.as_deref(),
                                limit,
                                offset,
                                &output,
                            ),
                            None => match result_id {
                                Some(id) => {
                                    results::get(&id, &workspace_id, database.as_deref(), &output)
                                }
                                None => {
                                    use clap::CommandFactory;
                                    let mut cmd = Cli::command();
                                    cmd.build();
                                    cmd.find_subcommand_mut("databases")
                                        .unwrap()
                                        .find_subcommand_mut("results")
                                        .unwrap()
                                        .print_help()
                                        .unwrap();
                                }
                            },
                        },
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("databases")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    }
                }
            }
            Commands::Jobs {
                id,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    jobs::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(JobsCommands::List {
                            job_type,
                            status,
                            all,
                            limit,
                            offset,
                            output,
                        }) => jobs::list(
                            &workspace_id,
                            job_type.as_deref(),
                            status.as_deref(),
                            all,
                            limit,
                            offset,
                            &output,
                        ),
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("jobs")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    }
                }
            }
            Commands::Ingest {
                workspace_id,
                output,
                command,
            } => {
                // Answered BEFORE the workspace is resolved. That a verb no
                // longer exists is a fact about the command surface, not about
                // the caller's workspace — so gating it behind resolution
                // replaces the explanation with an unrelated auth error, and
                // the person most likely to type a retired verb is the one
                // returning to the tool after a while, who may well not be
                // logged in.
                if let ingest::IngestCommands::Removed(argv) = &command {
                    ingest::removed(argv);
                }
                let workspace_id = resolve_workspace(workspace_id);
                ingest::dispatch(&workspace_id, &output, command);
            }
            Commands::Search {
                query,
                index,
                database,
                select,
                limit,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(command) => commands::search::dispatch(&workspace_id, command),
                    None => match (query, index) {
                        (Some(query), Some(index)) => commands::search::run(
                            &workspace_id,
                            database.as_deref(),
                            &index,
                            &query,
                            select.as_deref(),
                            limit,
                            &output,
                        ),
                        (Some(_), None) => {
                            use crossterm::style::Stylize;
                            eprintln!(
                                "{}",
                                "error: pass --index <name> to run a search (see `hotdata search list`), or use a subcommand: create, list, show, remove, embeddings.".red()
                            );
                            std::process::exit(2);
                        }
                        (None, _) => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("search")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    },
                }
            }
            Commands::Manage { command } => match command {
                cli::ManageCommands::Usage {
                    since,
                    workspace_id,
                    output,
                } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    usage::usage(&workspace_id, since.as_deref(), &output);
                }
                cli::ManageCommands::Completions { shell } => {
                    use clap::CommandFactory;
                    use clap_complete::generate;
                    let shell: clap_complete::Shell = shell.into();
                    let mut cmd = Cli::command();
                    generate(shell, &mut cmd, "hotdata", &mut std::io::stdout());
                }
                cli::ManageCommands::Upgrade => update::run_upgrade(),
                cli::ManageCommands::Skills { command } => match command {
                    SkillCommands::Install { project } => {
                        if project {
                            skill::install_project()
                        } else {
                            skill::install()
                        }
                    }
                    SkillCommands::Status | SkillCommands::List => skill::status(),
                },
            },
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
