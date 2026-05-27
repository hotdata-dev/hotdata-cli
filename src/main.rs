mod api;
mod auth;
mod command;
mod config;
mod connections;
mod connections_new;
mod context;
mod databases;
mod datasets;
mod embedding_providers;
mod indexes;
mod jobs;
mod jwt;
mod queries;
mod query;
mod results;
mod sandbox;
mod sandbox_session;
mod skill;
mod table;
mod tables;
mod update;
mod util;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{
    AuthCommands, Commands, ConnectionsCommands, ConnectionsCreateCommands, ContextCommands,
    DatabaseTablesCommands, DatabasesCommands, DatasetsCommands, EmbeddingProvidersCommands,
    IndexesCommands, JobsCommands, QueriesCommands, QueryCommands, ResultsCommands,
    SandboxCommands, SkillCommands, TablesCommands, WorkspaceCommands,
};

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
    if sandbox::find_sandbox_run_ancestor().is_some() {
        eprintln!("error: workspace has been lost -- restart the process");
        std::process::exit(1);
    }
    match config::load("default") {
        Ok(profile) => match config::resolve_workspace_id(provided, &profile) {
            Ok(id) => {
                let _ = ACTIVE_WORKSPACE_ID.set(id.clone());
                id
            }
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

// libc::atexit (no extra crate needed — the symbol is linked by default).
// Callbacks registered here fire even when subcommands call
// `std::process::exit`, which Rust's `Drop` would otherwise miss.
unsafe extern "C" {
    fn atexit(callback: extern "C" fn()) -> i32;
}

/// Runs once at process exit. Prints a sandbox footer on stderr when
/// the CLI is running under an on-disk sandbox session (i.e. the user
/// ran `hotdata sandbox set <id>` to enter it from this shell). Stays
/// silent when the sandbox comes from `HOTDATA_SANDBOX_TOKEN` in the
/// environment: that means we're inside a `sandbox run` child, and
/// the parent already announced the sandbox once at spawn time.
/// Stderr keeps stdout clean for callers parsing JSON/YAML output.
extern "C" fn print_sandbox_footer() {
    use crossterm::style::Stylize;

    // Inside a `sandbox run` child — parent printed the banner already.
    if sandbox_session::sandbox_token_in_use().is_some() {
        return;
    }
    let Some(session) = sandbox_session::load() else {
        return;
    };
    if session.sandbox_id.is_empty() {
        return;
    }
    eprintln!(
        "{}",
        format!(
            "current sandbox: {} use 'hotdata sandbox set' to change",
            session.sandbox_id
        )
        .dark_grey(),
    );
}

extern "C" fn print_database_footer() {
    use crossterm::style::Stylize;
    if let Some(ws_id) = ACTIVE_WORKSPACE_ID.get() {
        if let Some(id) = config::load_current_database("default", ws_id) {
            eprintln!(
                "{}",
                format!("current database: {id}  use 'hotdata databases set' to change")
                    .dark_grey(),
            );
        }
    }
}

fn main() {
    // Register before `Cli::parse`, since `--help` / `--version` exit
    // from inside the parser. Safety: `atexit` is async-signal-safe;
    // the callback only reads env vars / files and writes to stderr.
    unsafe { atexit(print_sandbox_footer) };
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

    // Kick off the update check in the background so it runs concurrently
    // with the command.  We join and print after the command finishes so the
    // notice always appears at the bottom of the output.  Skipped during
    // `hotdata update` itself so it doesn't talk over the updater's output.
    let update_handle = if !matches!(&cli.command, Some(Commands::Update)) {
        update::spawn_update_check()
    } else {
        None
    };

    match cli.command {
        None => {
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
        Some(cmd) => match cmd {
            Commands::Auth { command } => match command {
                None | Some(AuthCommands::Login) => auth::login(),
                Some(AuthCommands::Register { email }) => auth::register(email),
                Some(AuthCommands::Status) => auth::status("default"),
                Some(AuthCommands::Logout) => auth::logout("default"),
            },
            Commands::Datasets {
                id,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    datasets::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(DatasetsCommands::List {
                            limit,
                            offset,
                            output,
                        }) => datasets::list(&workspace_id, limit, offset, &output),
                        Some(DatasetsCommands::Create {
                            name,
                            description,
                            sql,
                            query_id,
                            output,
                        }) => {
                            if let Some(sql) = sql {
                                datasets::create_from_query(
                                    &workspace_id,
                                    &sql,
                                    description.as_deref(),
                                    &name,
                                    &output,
                                )
                            } else {
                                datasets::create_from_saved_query(
                                    &workspace_id,
                                    query_id.as_deref().unwrap_or_else(|| unreachable!("clap enforces --sql or --query-id")),
                                    description.as_deref(),
                                    &name,
                                    &output,
                                )
                            }
                        }
                        Some(DatasetsCommands::Update {
                            id,
                            description,
                            name,
                            output,
                        }) => datasets::update(
                            &id,
                            &workspace_id,
                            description.as_deref(),
                            name.as_deref(),
                            &output,
                        ),
                        Some(DatasetsCommands::Refresh { id, r#async }) => {
                            datasets::refresh(&workspace_id, &id, r#async)
                        }
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("datasets")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    }
                }
            }
            Commands::Query {
                sql,
                workspace_id,
                connection,
                database,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(QueryCommands::Status { id }) => query::poll(&id, &workspace_id, &output),
                    None => match sql {
                        Some(sql) => {
                            query::execute(
                                &sql,
                                &workspace_id,
                                connection.as_deref(),
                                database.as_deref(),
                                &output,
                            )
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
            Commands::Connections {
                id,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    connections::get(&workspace_id, &id, &output)
                } else {
                    match command {
                        Some(ConnectionsCommands::New) => connections_new::run(&workspace_id),
                        Some(ConnectionsCommands::List { output }) => {
                            connections::list(&workspace_id, &output)
                        }
                        Some(ConnectionsCommands::Create {
                            command,
                            name,
                            source_type,
                            config,
                            output,
                        }) => match command {
                            Some(ConnectionsCreateCommands::List { name, output }) => {
                                match name.as_deref() {
                                    Some(name) => {
                                        connections::types_get(&workspace_id, name, &output)
                                    }
                                    None => connections::types_list(&workspace_id, &output),
                                }
                            }
                            None => {
                                let missing: Vec<&str> = [
                                    name.is_none().then_some("--name"),
                                    source_type.is_none().then_some("--type"),
                                    config.is_none().then_some("--config"),
                                ]
                                .into_iter()
                                .flatten()
                                .collect();
                                if !missing.is_empty() {
                                    eprintln!(
                                        "error: missing required arguments: {}",
                                        missing.join(", ")
                                    );
                                    std::process::exit(1);
                                }
                                connections::create(
                                    &workspace_id,
                                    &name.unwrap(),
                                    &source_type.unwrap(),
                                    &config.unwrap(),
                                    &output,
                                )
                            }
                        },
                        Some(ConnectionsCommands::Refresh {
                            connection_id,
                            data,
                            schema,
                            table,
                            r#async,
                            include_uncached,
                        }) => connections::refresh(
                            &workspace_id,
                            &connection_id,
                            data,
                            schema.as_deref(),
                            table.as_deref(),
                            r#async,
                            include_uncached,
                        ),
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("connections")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    }
                }
            }
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
                        Some(DatabasesCommands::List { output }) => {
                            databases::list(&workspace_id, &output)
                        }
                        Some(DatabasesCommands::Show { name_or_id, output }) => {
                            databases::get(&workspace_id, &name_or_id, &output)
                        }
                        Some(DatabasesCommands::Create {
                            description,
                            schema,
                            tables,
                            expires_at,
                            output,
                        }) => databases::create(
                            &workspace_id,
                            description.as_deref(),
                            &schema,
                            &tables,
                            expires_at.as_deref(),
                            &output,
                        ),
                        Some(DatabasesCommands::Set { id_or_description }) => {
                            databases::set(&workspace_id, &id_or_description)
                        }
                        Some(DatabasesCommands::Delete { name_or_id }) => {
                            databases::delete(&workspace_id, &name_or_id)
                        }
                        Some(DatabasesCommands::Load {
                            target,
                            file,
                            url,
                            upload_id,
                        }) => {
                            let (database, schema, table) = parse_db_target(&target);
                            databases::tables_load(
                                &workspace_id,
                                Some(database.as_str()),
                                &table,
                                Some(schema.as_str()),
                                file.as_deref(),
                                url.as_deref(),
                                upload_id.as_deref(),
                            )
                        }
                        Some(DatabasesCommands::Tables { database, command }) => match command {
                            Some(DatabaseTablesCommands::List {
                                database: db_flag,
                                schema,
                                output,
                            }) => databases::tables_list(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                schema.as_deref(),
                                &output,
                            ),
                            Some(DatabaseTablesCommands::Load {
                                database: db_flag,
                                table,
                                schema,
                                file,
                                url,
                                upload_id,
                            }) => databases::tables_load(
                                &workspace_id,
                                db_flag.as_deref().or(database.as_deref()),
                                &table,
                                Some(schema.as_str()),
                                file.as_deref(),
                                url.as_deref(),
                                upload_id.as_deref(),
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
                TablesCommands::List {
                    workspace_id,
                    connection_id,
                    schema,
                    table,
                    limit,
                    cursor,
                    output,
                } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    tables::list(
                        &workspace_id,
                        connection_id.as_deref(),
                        schema.as_deref(),
                        table.as_deref(),
                        limit,
                        cursor.as_deref(),
                        &output,
                    )
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
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(ResultsCommands::List {
                        limit,
                        offset,
                        output,
                    }) => results::list(&workspace_id, limit, offset, &output),
                    None => match result_id {
                        Some(id) => results::get(&id, &workspace_id, &output),
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
            Commands::Indexes {
                workspace_id,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    IndexesCommands::List {
                        connection_id,
                        schema,
                        table,
                        dataset_id,
                        output,
                    } => indexes::list(
                        &workspace_id,
                        connection_id.as_deref(),
                        schema.as_deref(),
                        table.as_deref(),
                        dataset_id.as_deref(),
                        &output,
                    ),
                    IndexesCommands::Create {
                        target,
                        dataset_id,
                        columns,
                        name,
                        r#type,
                        metric,
                        r#async,
                        embedding_provider_id,
                        dimensions,
                        output_column,
                        description,
                    } => {
                        let api = api::ApiClient::new(Some(&workspace_id));
                        let (scope, resolved_columns, auto_name) =
                            match (target.as_deref(), dataset_id.as_deref()) {
                                (Some(tgt), None) => {
                                    let (conn_name, schema, table, cols) =
                                        parse_index_target(tgt);
                                    let conn_id =
                                        connections::resolve_connection_id(&api, &conn_name);
                                    let auto = format!(
                                        "{table}_{cols}_{type}",
                                        cols = cols.join("_"),
                                        type = r#type
                                    );
                                    (
                                        (conn_id, schema, table),
                                        cols.join(","),
                                        auto,
                                    )
                                }
                                (None, Some(did)) => {
                                    let cols =
                                        columns.as_deref().unwrap_or_else(|| {
                                            eprintln!(
                                                "error: --columns is required with --dataset-id"
                                            );
                                            std::process::exit(1);
                                        });
                                    let auto = format!(
                                        "dataset_{cols}_{type}",
                                        cols = cols.replace(',', "_"),
                                        type = r#type
                                    );
                                    (
                                        (did.to_string(), String::new(), String::new()),
                                        cols.to_string(),
                                        auto,
                                    )
                                }
                                _ => {
                                    eprintln!(
                                        "error: provide either <target> (e.g. airbnb.listings[col1,col2]) or --dataset-id with --columns"
                                    );
                                    std::process::exit(1);
                                }
                            };
                        let index_name = name.unwrap_or(auto_name);
                        let is_dataset = dataset_id.is_some();
                        let (conn_id, schema, table) = scope;
                        let resolved_scope = if is_dataset {
                            indexes::IndexScope::Dataset { dataset_id: &conn_id }
                        } else {
                            indexes::IndexScope::Connection {
                                connection_id: &conn_id,
                                schema: &schema,
                                table: &table,
                            }
                        };
                        indexes::create(
                            &workspace_id,
                            resolved_scope,
                            &index_name,
                            &resolved_columns,
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
                        connection_id,
                        schema,
                        table,
                        dataset_id,
                        name,
                    } => {
                        let scope = match (
                            dataset_id.as_deref(),
                            connection_id.as_deref(),
                            schema.as_deref(),
                            table.as_deref(),
                        ) {
                            (Some(did), _, _, _) => indexes::IndexScope::Dataset { dataset_id: did },
                            (None, Some(cid), Some(sch), Some(tbl)) => {
                                indexes::IndexScope::Connection {
                                    connection_id: cid,
                                    schema: sch,
                                    table: tbl,
                                }
                            }
                            _ => {
                                eprintln!(
                                    "error: provide either --dataset-id or all three of --connection-id, --schema, --table"
                                );
                                std::process::exit(1);
                            }
                        };
                        indexes::delete(&workspace_id, scope, &name);
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

                // Parse `connection.table` or `connection.schema.table`.
                // Schema defaults to `public` when omitted.
                let parts: Vec<&str> = table.splitn(4, '.').collect();
                let (conn_name, schema, table_name) = match parts.as_slice() {
                    [conn, schema, tbl] => {
                        (conn.to_string(), schema.to_string(), tbl.to_string())
                    }
                    [conn, tbl] => (conn.to_string(), "public".to_string(), tbl.to_string()),
                    _ => {
                        eprintln!(
                            "error: --table must be 'connection.table' or 'connection.schema.table'"
                        );
                        std::process::exit(1);
                    }
                };
                let normalized_table = format!("{}.{}.{}", conn_name, schema, table_name);

                // Infer --type and --column from the table's indexes when either is omitted.
                let (resolved_type, resolved_column) =
                    if r#type.is_some() && column.is_some() {
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
                query::execute(&sql, &workspace_id, None, None, &output)
            }
            Commands::Queries {
                id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(None);
                if let Some(id) = id {
                    queries::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(QueriesCommands::List {
                            limit,
                            cursor,
                            status,
                            output,
                        }) => queries::list(
                            &workspace_id,
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
            Commands::Sandbox {
                id,
                workspace_id,
                output,
                command,
            } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(SandboxCommands::Run { name, cmd }) => {
                        sandbox::run(id.as_deref(), &workspace_id, name.as_deref(), &cmd)
                    }
                    Some(SandboxCommands::List { output }) => sandbox::list(&workspace_id, &output),
                    Some(SandboxCommands::New { name, output }) => {
                        sandbox::new(&workspace_id, name.as_deref(), &output)
                    }
                    Some(SandboxCommands::Update {
                        id: update_id,
                        name,
                        markdown,
                        output,
                    }) => {
                        let sandbox_id = update_id
                            .or(id)
                            .or_else(|| config::load("default").ok().and_then(|p| p.sandbox));
                        match sandbox_id {
                            Some(sid) => sandbox::update(
                                &workspace_id,
                                &sid,
                                name.as_deref(),
                                markdown.as_deref(),
                                &output,
                            ),
                            None => {
                                eprintln!(
                                    "error: no sandbox ID provided and no active sandbox set. Use 'sandbox new' or 'sandbox set <id>'."
                                );
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(SandboxCommands::Read) => {
                        let sandbox_id = id
                            .or_else(|| std::env::var("HOTDATA_SANDBOX").ok())
                            .or_else(|| config::load("default").ok().and_then(|p| p.sandbox));
                        match sandbox_id {
                            Some(sid) => sandbox::read(&sid, &workspace_id),
                            None => {
                                eprintln!(
                                    "error: no active sandbox. Use 'sandbox new' or 'sandbox set <id>'."
                                );
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(SandboxCommands::Set { id: set_id }) => {
                        sandbox::set(set_id.as_deref(), &workspace_id)
                    }
                    Some(SandboxCommands::Delete { id: delete_id }) => {
                        sandbox::delete(&delete_id, &workspace_id)
                    }
                    None => match id {
                        Some(id) => sandbox::get(&id, &workspace_id, &output),
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("sandbox")
                                .unwrap()
                                .print_help()
                                .unwrap();
                        }
                    },
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
            Commands::Completions { shell } => {
                use clap::CommandFactory;
                use clap_complete::generate;
                let shell: clap_complete::Shell = shell.into();
                let mut cmd = Cli::command();
                generate(shell, &mut cmd, "hotdata", &mut std::io::stdout());
            }
            Commands::Update => update::run_update(),
        },
    }

    // Print update notice after command output (joined from background thread).
    update::maybe_print_update_notice(update_handle);
}

/// Parse a database target like `airbnb.listings` or `airbnb.public.listings`
/// into `(database, schema, table)`. Schema defaults to `public`.
fn parse_db_target(target: &str) -> (String, String, String) {
    let parts: Vec<&str> = target.splitn(4, '.').collect();
    match parts.as_slice() {
        [db, tbl] => (db.to_string(), "public".to_string(), tbl.to_string()),
        [db, schema, tbl] => (db.to_string(), schema.to_string(), tbl.to_string()),
        _ => {
            eprintln!(
                "error: target must be 'database.table' or 'database.schema.table'"
            );
            std::process::exit(1);
        }
    }
}

/// Parse an index target like `airbnb.listings[col1,col2]` or
/// `airbnb.public.listings[col1,col2]` into `(conn_name, schema, table, columns)`.
/// Schema defaults to `public` when only two dot-parts are given.
fn parse_index_target(target: &str) -> (String, String, String, Vec<String>) {
    let Some(bracket_pos) = target.find('[') else {
        eprintln!(
            "error: target must include columns in brackets, e.g. airbnb.listings[col1,col2]"
        );
        std::process::exit(1);
    };
    if !target.ends_with(']') {
        eprintln!(
            "error: target bracket is not closed — use e.g. 'airbnb.listings[col1,col2]'"
        );
        std::process::exit(1);
    }
    let table_part = &target[..bracket_pos];
    let cols_raw = &target[bracket_pos + 1..target.len() - 1];

    let parts: Vec<&str> = table_part.splitn(4, '.').collect();
    let (conn, schema, table) = match parts.as_slice() {
        [c, t] => (c.to_string(), "public".to_string(), t.to_string()),
        [c, s, t] => (c.to_string(), s.to_string(), t.to_string()),
        _ => {
            eprintln!(
                "error: target must be 'connection.table[cols]' or 'connection.schema.table[cols]'"
            );
            std::process::exit(1);
        }
    };

    let columns: Vec<String> = cols_raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if columns.is_empty() {
        eprintln!("error: no columns specified in brackets");
        std::process::exit(1);
    }

    (conn, schema, table, columns)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- parse_db_target ---

    #[test]
    fn db_target_two_parts_defaults_schema_to_public() {
        let (db, schema, table) = parse_db_target("airbnb.listings");
        assert_eq!(db, "airbnb");
        assert_eq!(schema, "public");
        assert_eq!(table, "listings");
    }

    #[test]
    fn db_target_three_parts_uses_explicit_schema() {
        let (db, schema, table) = parse_db_target("airbnb.staging.listings");
        assert_eq!(db, "airbnb");
        assert_eq!(schema, "staging");
        assert_eq!(table, "listings");
    }

    // --- parse_index_target ---

    #[test]
    fn index_target_two_parts_defaults_schema_to_public() {
        let (conn, schema, table, cols) = parse_index_target("airbnb.listings[description]");
        assert_eq!(conn, "airbnb");
        assert_eq!(schema, "public");
        assert_eq!(table, "listings");
        assert_eq!(cols, vec!["description"]);
    }

    #[test]
    fn index_target_three_parts_uses_explicit_schema() {
        let (conn, schema, table, cols) =
            parse_index_target("airbnb.public.listings[name,description]");
        assert_eq!(conn, "airbnb");
        assert_eq!(schema, "public");
        assert_eq!(table, "listings");
        assert_eq!(cols, vec!["name", "description"]);
    }

    #[test]
    fn index_target_multiple_columns() {
        let (_, _, _, cols) = parse_index_target("db.tbl[a,b,c]");
        assert_eq!(cols, vec!["a", "b", "c"]);
    }

    #[test]
    fn index_target_trims_column_whitespace() {
        let (_, _, _, cols) = parse_index_target("db.tbl[a, b]");
        assert_eq!(cols, vec!["a", "b"]);
    }
}

pub fn get_styles() -> clap::builder::Styles {
    Styles::styled()
        .header(AnsiColor::Yellow.on_default())
        .usage(AnsiColor::Green.on_default())
        .literal(AnsiColor::Green.on_default())
        .placeholder(AnsiColor::Green.on_default())
}
