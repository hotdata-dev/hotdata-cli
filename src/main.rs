mod cli;
mod client;
mod commands;
mod config;
mod output;
mod util;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use cli::Commands;
use client::{credentials, database_session, sdk};
use commands::auth::{self, AuthCommands};
use commands::connections;
use commands::context::{self, ContextCommands};
use commands::databases::{self, DatabaseTablesCommands, DatabasesCommands};
use commands::embedding_providers::{self, EmbeddingProvidersCommands};
use commands::indexes::{self, IndexesCommands};
use commands::ingest;
use commands::jobs::{self, JobsCommands};
use commands::queries::{self, QueriesCommands};
use commands::query::{self, QueryCommands};
use commands::results::{self, ResultsCommands};
use commands::skill::{self, SkillCommands};
use commands::tables::{self, TablesCommands};
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
    // Inside a `databases run` child the parent already announced the
    // database at spawn, so stay silent here.
    if database_session::database_token_in_use().is_some() {
        return;
    }
    if let Some(ws_id) = ACTIVE_WORKSPACE_ID.get()
        && let Some(id) = config::load_current_database("default", ws_id)
    {
        eprintln!(
            "{}",
            format!("current database: {id}  use 'hotdata databases set' to change").dark_grey(),
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

    let skip_skill_auto_update =
        cli.command.is_none() || matches!(&cli.command, Some(Commands::Skills { .. }));
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
        None | Some(Commands::Upgrade)
            | Some(Commands::Completions { .. })
            | Some(Commands::Auth { command: None })
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
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(QueryCommands::Status { id }) => {
                        query::poll(&id, &workspace_id, database.as_deref(), &output)
                    }
                    None => match sql {
                        Some(sql) => {
                            query::execute(&sql, &workspace_id, database.as_deref(), &output)
                        }
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
                // `databases <id> run ...` should mint a token for <id>, not
                // short to `show`. Route Run before the name_or_id show-shorthand;
                // --database on the subcommand takes precedence over the group
                // positional. Other subcommands keep the existing semantics: a
                // group-level name_or_id is treated as a `show` shorthand.
                if let Some(DatabasesCommands::Run {
                    database,
                    name,
                    schema,
                    tables,
                    expires_at,
                    cmd,
                }) = command
                {
                    let db = database.as_deref().or(name_or_id.as_deref());
                    databases::run(
                        db,
                        &workspace_id,
                        name.as_deref(),
                        &schema,
                        &tables,
                        expires_at.as_deref(),
                        &cmd,
                    );
                } else if let Some(name_or_id) = name_or_id {
                    databases::get(&workspace_id, &name_or_id, &output);
                } else {
                    match command {
                        Some(DatabasesCommands::List { output }) => {
                            databases::list(&workspace_id, &output)
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
                        Some(DatabasesCommands::Attach {
                            connection,
                            database,
                            alias,
                        }) => databases::attach(
                            &workspace_id,
                            &connection,
                            database.as_deref(),
                            alias.as_deref(),
                        ),
                        Some(DatabasesCommands::Detach {
                            connection,
                            database,
                        }) => databases::detach(&workspace_id, &connection, database.as_deref()),
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
                        }) => databases::tables_load(
                            &workspace_id,
                            Some(catalog.as_str()),
                            &table,
                            Some(schema.as_str()),
                            file.as_deref(),
                            url.as_deref(),
                            upload_id.as_deref(),
                            result_id.as_deref(),
                        ),
                        Some(DatabasesCommands::Tables { database, command }) => match command {
                            Some(DatabaseTablesCommands::List {
                                database: db_flag,
                                schema,
                                output,
                            }) => databases::tables_list(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                schema.as_deref(),
                                None,
                                None,
                                None,
                                &output,
                            ),
                            Some(DatabaseTablesCommands::Load {
                                database: db_flag,
                                table,
                                schema,
                                file,
                                url,
                                upload_id,
                                result_id,
                            }) => databases::tables_load(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                &table,
                                Some(schema.as_str()),
                                file.as_deref(),
                                url.as_deref(),
                                upload_id.as_deref(),
                                result_id.as_deref(),
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
                        Some(DatabasesCommands::Run { .. }) => {
                            // Handled by the Run-first if-let above.
                            unreachable!("Run handled before name_or_id shorthand");
                        }
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
            Commands::Tables { command } => match command {
                TablesCommands::Show { table, output } => {
                    let workspace_id = resolve_workspace(None);
                    tables::show(&workspace_id, &table, &output)
                }
                TablesCommands::List {
                    workspace_id,
                    schema,
                    table,
                    limit,
                    cursor,
                    output,
                } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    if crate::config::load_current_database("default", &workspace_id).is_some() {
                        databases::tables_list(
                            &workspace_id,
                            None,
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
            },
            Commands::Skills { command } => match command {
                SkillCommands::Install { project } => {
                    if project {
                        skill::install_project()
                    } else {
                        skill::install()
                    }
                }
                SkillCommands::Status | SkillCommands::List => skill::status(),
            },
            Commands::Results {
                result_id,
                workspace_id,
                database,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(ResultsCommands::Show { id, output }) => {
                        results::get(&id, &workspace_id, database.as_deref(), &output)
                    }
                    Some(ResultsCommands::List {
                        limit,
                        offset,
                        output,
                    }) => results::list(&workspace_id, database.as_deref(), limit, offset, &output),
                    None => match result_id {
                        Some(id) => results::get(&id, &workspace_id, database.as_deref(), &output),
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("results")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    },
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
                let workspace_id = resolve_workspace(workspace_id);
                ingest::dispatch(&workspace_id, &output, command);
            }
            Commands::Indexes {
                workspace_id,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    IndexesCommands::List {
                        schema,
                        table,
                        output,
                    } => {
                        let connection_id =
                            crate::config::load_current_database("default", &workspace_id)
                                .and_then(|db_id| {
                                    let api = sdk::Api::new(Some(&workspace_id));
                                    crate::commands::databases::get_database(&api, &db_id)
                                        .ok()
                                        .map(|db| db.default_connection_id)
                                });
                        indexes::list(
                            &workspace_id,
                            connection_id.as_deref(),
                            schema.as_deref(),
                            table.as_deref(),
                            &output,
                        )
                    }
                    IndexesCommands::Create {
                        catalog,
                        schema,
                        table,
                        column,
                        name,
                        r#type,
                        metric,
                        r#async,
                        embedding_provider_id,
                        dimensions,
                        output_column,
                        description,
                    } => {
                        let api = sdk::Api::new(Some(&workspace_id));
                        let catalog_or_conn = catalog.as_deref().unwrap_or_else(|| {
                            eprintln!("error: --catalog is required");
                            std::process::exit(1);
                        });
                        let tbl = table.as_deref().unwrap_or_else(|| {
                            eprintln!("error: --table is required");
                            std::process::exit(1);
                        });
                        let cols = column.as_deref().unwrap_or_else(|| {
                            eprintln!("error: --column is required");
                            std::process::exit(1);
                        });
                        let conn_id = connections::resolve_connection_id(&api, catalog_or_conn);
                        let auto_name = format!(
                            "{tbl}_{cols}_{type}",
                            cols = cols.replace(',', "_"),
                            type = r#type
                        );
                        let index_name = name.unwrap_or(auto_name);
                        indexes::create(
                            &workspace_id,
                            indexes::IndexScope::Connection {
                                connection_id: &conn_id,
                                schema: &schema,
                                table: tbl,
                            },
                            &index_name,
                            cols,
                            &r#type,
                            metric.as_deref(),
                            r#async,
                            embedding_provider_id.as_deref(),
                            dimensions,
                            output_column.as_deref(),
                            description.as_deref(),
                        )
                    }
                    IndexesCommands::Delete {
                        catalog,
                        schema,
                        table,
                        name,
                    } => {
                        let api = sdk::Api::new(Some(&workspace_id));
                        let conn_id = connections::resolve_connection_id(&api, &catalog);
                        indexes::delete(
                            &workspace_id,
                            indexes::IndexScope::Connection {
                                connection_id: &conn_id,
                                schema: &schema,
                                table: &table,
                            },
                            &name,
                        );
                    }
                }
            }
            Commands::EmbeddingProviders {
                workspace_id,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    EmbeddingProvidersCommands::List { output } => {
                        embedding_providers::list(&workspace_id, &output)
                    }
                    EmbeddingProvidersCommands::Get { id, output } => {
                        embedding_providers::get(&workspace_id, &id, &output)
                    }
                    EmbeddingProvidersCommands::Create {
                        name,
                        provider_type,
                        config,
                        provider_api_key,
                        secret_name,
                        output,
                    } => embedding_providers::create(
                        &workspace_id,
                        &name,
                        &provider_type,
                        config.as_deref(),
                        provider_api_key.as_deref(),
                        secret_name.as_deref(),
                        &output,
                    ),
                    EmbeddingProvidersCommands::Update {
                        id,
                        name,
                        config,
                        provider_api_key,
                        secret_name,
                        output,
                    } => embedding_providers::update(
                        &workspace_id,
                        &id,
                        name.as_deref(),
                        config.as_deref(),
                        provider_api_key.as_deref(),
                        secret_name.as_deref(),
                        &output,
                    ),
                    EmbeddingProvidersCommands::Delete { id } => {
                        embedding_providers::delete(&workspace_id, &id)
                    }
                }
            }
            Commands::Search {
                query,
                r#type,
                table,
                column,
                select,
                limit,
                workspace_id,
                output,
            } => {
                let workspace_id = resolve_workspace(workspace_id);

                // Parse `catalog.schema.table` or `schema.table` (requires active database).
                let parts: Vec<&str> = table.splitn(3, '.').collect();
                let (conn_name, schema, table_name) = match parts.as_slice() {
                    [catalog, schema, tbl] => {
                        (catalog.to_string(), schema.to_string(), tbl.to_string())
                    }
                    [schema, tbl] => {
                        // Two-part: use active database's catalog.
                        let db_id = crate::config::load_current_database("default", &workspace_id)
                            .unwrap_or_else(|| {
                                use crossterm::style::Stylize;
                                eprintln!(
                                    "{}",
                                    "error: use catalog.schema.table, or set an active database \
                                     with `hotdata databases set <id>`."
                                        .red()
                                );
                                std::process::exit(1);
                            });
                        let api = sdk::Api::new(Some(&workspace_id));
                        let db = crate::commands::databases::get_database(&api, &db_id)
                            .unwrap_or_else(|e| e.exit());
                        let catalog = db
                            .default_catalog
                            .unwrap_or_else(|| db.name.unwrap_or_else(|| "default".to_string()));
                        (catalog, schema.to_string(), tbl.to_string())
                    }
                    _ => {
                        use crossterm::style::Stylize;
                        eprintln!(
                            "{}",
                            "error: --table must be 'schema.table' or 'catalog.schema.table'".red()
                        );
                        std::process::exit(1);
                    }
                };
                let normalized_table = format!("{}.{}.{}", conn_name, schema, table_name);

                // Infer --type and --column from the table's indexes when either is omitted.
                let (resolved_type, resolved_column) = if r#type.is_some() && column.is_some() {
                    (r#type.unwrap(), column.unwrap())
                } else {
                    let (inferred_type, inferred_column) = indexes::infer_for_search(
                        &workspace_id,
                        &conn_name,
                        &schema,
                        &table_name,
                        r#type.as_deref(),
                        column.as_deref(),
                    );
                    (
                        r#type.unwrap_or(inferred_type),
                        column.unwrap_or(inferred_column),
                    )
                };

                let select_cols = select.as_deref().unwrap_or("*");

                let sql = match resolved_type.as_str() {
                    "bm25" => {
                        let bm25_columns = match select.as_deref() {
                            Some(cols) if cols.split(',').any(|c| c.trim() == "score") => {
                                cols.to_string()
                            }
                            Some(cols) => format!("{}, score", cols),
                            None => "*".to_string(),
                        };
                        format!(
                            "SELECT {} FROM bm25_search('{}', '{}', '{}') ORDER BY score DESC LIMIT {}",
                            bm25_columns,
                            normalized_table.replace('\'', "''"),
                            resolved_column.replace('\'', "''"),
                            query.replace('\'', "''"),
                            limit,
                        )
                    }
                    // Server-side vector_distance: resolves the embedding column, model,
                    // and metric from the index metadata. The user names the source text column.
                    "vector" => format!(
                        "SELECT {}, vector_distance({}, '{}') AS dist FROM {} ORDER BY dist LIMIT {}",
                        select_cols,
                        resolved_column,
                        query.replace('\'', "''"),
                        normalized_table,
                        limit,
                    ),
                    _ => unreachable!(),
                };
                query::execute(&sql, &workspace_id, None, &output)
            }
            Commands::Queries {
                id,
                database,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(None);
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
                            cmd.find_subcommand_mut("queries")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    }
                }
            }
            Commands::Context {
                workspace_id,
                database_id,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                let database_id = database_id
                    .or_else(|| config::load_current_database("default", &workspace_id))
                    .unwrap_or_else(|| {
                        eprintln!(
                            "error: no active database. Use 'hotdata databases set <id>' to set one, or pass --database-id."
                        );
                        std::process::exit(1);
                    });
                match command {
                    ContextCommands::List { output, prefix } => {
                        context::list(&workspace_id, &database_id, prefix.as_deref(), &output)
                    }
                    ContextCommands::Show { name } => {
                        context::show(&workspace_id, &database_id, &name)
                    }
                    ContextCommands::Pull {
                        name,
                        force,
                        dry_run,
                    } => context::pull(&workspace_id, &database_id, &name, force, dry_run),
                    ContextCommands::Push { name, dry_run } => {
                        context::push(&workspace_id, &database_id, &name, dry_run)
                    }
                }
            }
            Commands::Usage {
                since,
                workspace_id,
                output,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                usage::usage(&workspace_id, since.as_deref(), &output);
            }
            Commands::Completions { shell } => {
                use clap::CommandFactory;
                use clap_complete::generate;
                let shell: clap_complete::Shell = shell.into();
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "hotdata", &mut std::io::stdout());
            }
            Commands::Upgrade => update::run_upgrade(),
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
