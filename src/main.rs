mod api;
mod auth;
mod command;
mod config;
mod connections;
mod connections_new;
mod datasets;
mod embedding;
mod indexes;
mod jobs;
mod markdown;
mod queries;
mod query;
mod results;
mod sessions;
mod skill;
mod table;
mod tables;
mod util;
mod workspace;

use anstyle::AnsiColor;
use clap::{Parser, builder::Styles};
use command::{AuthCommands, Commands, ConnectionsCommands, ConnectionsCreateCommands, DatasetsCommands, IndexesCommands, JobsCommands, QueriesCommands, QueryCommands, ResultsCommands, SessionsCommands, SkillCommands, TablesCommands, WorkspaceCommands};

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
    #[arg(long, global = true)]
    debug: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

fn resolve_workspace(provided: Option<String>) -> String {
    // HOTDATA_WORKSPACE env var takes priority and blocks --workspace-id flag
    if let Ok(ws) = std::env::var("HOTDATA_WORKSPACE") {
        if let Some(ref flag) = provided {
            if flag != &ws {
                eprintln!("error: cannot override workspace -- locked by HOTDATA_WORKSPACE environment variable ({ws})");
                std::process::exit(1);
            }
        }
        return ws;
    }
    if sessions::find_session_run_ancestor().is_some() {
        eprintln!("error: workspace has been lost -- restart the process");
        std::process::exit(1);
    }
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
    if cli.debug {
        util::set_debug(true);
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
            Commands::Datasets { id, workspace_id, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    datasets::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(DatasetsCommands::List { limit, offset, output }) => {
                            datasets::list(&workspace_id, limit, offset, &output)
                        }
                        Some(DatasetsCommands::Create { label, table_name, file, upload_id, format, sql, query_id, url }) => {
                            if let Some(sql) = sql {
                                datasets::create_from_query(&workspace_id, &sql, label.as_deref(), table_name.as_deref())
                            } else if let Some(query_id) = query_id {
                                datasets::create_from_saved_query(&workspace_id, &query_id, label.as_deref(), table_name.as_deref())
                            } else if let Some(url) = url {
                                datasets::create_from_url(&workspace_id, &url, label.as_deref(), table_name.as_deref())
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
            Commands::Query { sql, workspace_id, connection, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(QueryCommands::Status { id }) => {
                        query::poll(&id, &workspace_id, &output)
                    }
                    None => {
                        match sql {
                            Some(sql) => query::execute(&sql, &workspace_id, connection.as_deref(), &output),
                            None => {
                                use clap::CommandFactory;
                                let mut cmd = Cli::command();
                                cmd.build();
                                cmd.find_subcommand_mut("query").unwrap().print_help().unwrap();
                            }
                        }
                    }
                }
            }
            Commands::Workspaces { command } => match command {
                WorkspaceCommands::List { output } => workspace::list(&output),
                WorkspaceCommands::Set { workspace_id } => workspace::set(workspace_id.as_deref()),
            },
            Commands::Connections { id, workspace_id, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    connections::get(&workspace_id, &id, &output)
                } else {
                    match command {
                        Some(ConnectionsCommands::New) => connections_new::run(&workspace_id),
                        Some(ConnectionsCommands::List { output }) => {
                            connections::list(&workspace_id, &output)
                        }
                        Some(ConnectionsCommands::Create { command, name, source_type, config, output }) => {
                            match command {
                                Some(ConnectionsCreateCommands::List { name, output }) => {
                                    match name.as_deref() {
                                        Some(name) => connections::types_get(&workspace_id, name, &output),
                                        None => connections::types_list(&workspace_id, &output),
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
                                        &output,
                                    )
                                }
                            }
                        }
                        Some(ConnectionsCommands::Refresh { connection_id }) => {
                            connections::refresh(&workspace_id, &connection_id)
                        }
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("connections").unwrap().print_help().unwrap();
                        }
                    }
                }
            },
            Commands::Tables { command } => match command {
                TablesCommands::List { workspace_id, connection_id, schema, table, limit, cursor, output } => {
                    let workspace_id = resolve_workspace(workspace_id);
                    tables::list(&workspace_id, connection_id.as_deref(), schema.as_deref(), table.as_deref(), limit, cursor.as_deref(), &output)
                }
            },
            Commands::Skills { command } => match command {
                SkillCommands::Install { project } => {
                    if project { skill::install_project() } else { skill::install() }
                }
                SkillCommands::Status => skill::status(),
            },
            Commands::Results { result_id, workspace_id, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(ResultsCommands::List { limit, offset, output }) => {
                        results::list(&workspace_id, limit, offset, &output)
                    }
                    None => {
                        match result_id {
                            Some(id) => results::get(&id, &workspace_id, &output),
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
            Commands::Jobs { id, workspace_id, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                if let Some(id) = id {
                    jobs::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(JobsCommands::List { job_type, status, all, limit, offset, output }) => {
                            jobs::list(&workspace_id, job_type.as_deref(), status.as_deref(), all, limit, offset, &output)
                        }
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("jobs").unwrap().print_help().unwrap();
                        }
                    }
                }
            }
            Commands::Indexes { workspace_id, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    IndexesCommands::List { connection_id, schema, table, output } => {
                        indexes::list(&workspace_id, &connection_id, &schema, &table, &output)
                    }
                    IndexesCommands::Create { connection_id, schema, table, name, columns, r#type, metric, r#async } => {
                        indexes::create(&workspace_id, &connection_id, &schema, &table, &name, &columns, &r#type, metric.as_deref(), r#async)
                    }
                }
            }
            Commands::Search { query, table, column, select, limit, model, workspace_id, output } => {
                let workspace_id = resolve_workspace(workspace_id);
                let select_cols = select.as_deref().unwrap_or("*");

                // Determine search mode:
                // 1. --model flag: embed the query text via the model provider
                // 2. No query + piped stdin: read vector from stdin
                // 3. Query text without --model: BM25 text search
                let sql = if let Some(ref model_name) = model {
                    let query_text = match query {
                        Some(ref q) => q.as_str(),
                        None => {
                            eprintln!("error: --model requires a search query text");
                            std::process::exit(1);
                        }
                    };
                    let vec = embedding::openai_embed(query_text, model_name);
                    let vec_str = embedding::vector_to_sql(&vec);
                    format!(
                        "SELECT {}, l2_distance({}, {}) as dist FROM {} ORDER BY dist LIMIT {}",
                        select_cols, column, vec_str, table, limit,
                    )
                } else if query.is_none() {
                    use std::io::IsTerminal;
                    if std::io::stdin().is_terminal() {
                        eprintln!("error: provide a search query or pipe a vector via stdin");
                        std::process::exit(1);
                    }
                    let vec = embedding::read_vector_from_stdin();
                    let vec_str = embedding::vector_to_sql(&vec);
                    format!(
                        "SELECT {}, l2_distance({}, {}) as dist FROM {} ORDER BY dist LIMIT {}",
                        select_cols, column, vec_str, table, limit,
                    )
                } else {
                    let bm25_columns = match select.as_deref() {
                        Some(cols) => format!("{}, score", cols),
                        None => "*".to_string(),
                    };
                    format!(
                        "SELECT {} FROM bm25_search('{}', '{}', '{}') ORDER BY score DESC LIMIT {}",
                        bm25_columns,
                        table.replace('\'', "''"),
                        column.replace('\'', "''"),
                        query.unwrap().replace('\'', "''"),
                        limit,
                    )
                };
                query::execute(&sql, &workspace_id, None, &output)
            }
            Commands::Queries { id, output, command } => {
                let workspace_id = resolve_workspace(None);
                if let Some(id) = id {
                    queries::get(&id, &workspace_id, &output)
                } else {
                    match command {
                        Some(QueriesCommands::List { limit, offset, output }) => {
                            queries::list(&workspace_id, limit, offset, &output)
                        }
                        Some(QueriesCommands::Run { id, output }) => {
                            queries::run(&id, &workspace_id, &output)
                        }
                        Some(QueriesCommands::Create { name, sql, description, tags, output }) => {
                            queries::create(&workspace_id, &name, &sql, description.as_deref(), tags.as_deref(), &output)
                        }
                        Some(QueriesCommands::Update { id, name, sql, description, tags, category, table_size, output }) => {
                            queries::update(&workspace_id, &id, name.as_deref(), sql.as_deref(), description.as_deref(), tags.as_deref(), category.as_deref(), table_size.as_deref(), &output)
                        }
                        None => {
                            use clap::CommandFactory;
                            let mut cmd = Cli::command();
                            cmd.build();
                            cmd.find_subcommand_mut("queries").unwrap().print_help().unwrap();
                        }
                    }
                }
            }
            Commands::Sessions { id, workspace_id, output, command } => {
                let workspace_id = resolve_workspace(workspace_id);
                match command {
                    Some(SessionsCommands::Run { name, cmd }) => {
                        sessions::run(id.as_deref(), &workspace_id, name.as_deref(), &cmd)
                    }
                    Some(SessionsCommands::List { output }) => {
                        sessions::list(&workspace_id, &output)
                    }
                    Some(SessionsCommands::New { name, output }) => {
                        sessions::new(&workspace_id, name.as_deref(), &output)
                    }
                    Some(SessionsCommands::Update { id: update_id, name, markdown, output }) => {
                        let session_id = update_id.or(id).or_else(|| {
                            config::load("default").ok().and_then(|p| p.session)
                        });
                        match session_id {
                            Some(sid) => sessions::update(&workspace_id, &sid, name.as_deref(), markdown.as_deref(), &output),
                            None => {
                                eprintln!("error: no session ID provided and no active session set. Use 'sessions new' or 'sessions set <id>'.");
                                std::process::exit(1);
                            }
                        }
                    }
                    Some(SessionsCommands::Read { id: read_id, lines, section, output }) => {
                        let target = sessions::resolve_read_target(read_id.or(id));
                        sessions::read(&target, &workspace_id, lines.as_deref(), section.as_deref(), &output);
                    }
                    Some(SessionsCommands::Outline { id: outline_id, output }) => {
                        let target = sessions::resolve_read_target(outline_id.or(id));
                        sessions::outline(&target, &workspace_id, &output);
                    }
                    Some(SessionsCommands::Set { id: set_id }) => {
                        sessions::set(set_id.as_deref(), &workspace_id)
                    }
                    None => {
                        match id {
                            Some(id) => sessions::get(&id, &workspace_id, &output),
                            None => {
                                use clap::CommandFactory;
                                let mut cmd = Cli::command();
                                cmd.build();
                                cmd.find_subcommand_mut("sessions").unwrap().print_help().unwrap();
                            }
                        }
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
