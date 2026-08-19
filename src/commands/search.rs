//! `hotdata search` — search indexes as named objects.
//!
//! A reshape of the former flag-based `search`, plus `indexes` and
//! `embedding-providers`, into one namespace. An index is addressed by name;
//! `--type vector|text` maps onto the underlying `vector` / `bm25` index types.
//! The legacy flag form (`search "text" --table …`) is preserved via
//! [`legacy`] during migration.

use crate::client::sdk::Api;
use crate::commands::embedding_providers::{self, EmbeddingProvidersCommands};
use crate::commands::indexes::{self, IndexScope};
use crate::commands::{connections, databases, query};

/// Subcommands for `hotdata search`.
#[derive(clap::Subcommand)]
pub enum SearchCommands {
    /// Create a search index over a table column
    Create {
        /// Index name (derived from table, column, and type if omitted)
        name: Option<String>,

        /// Search type: `vector` (semantic) or `text` (BM25 full-text)
        #[arg(long, value_parser = ["vector", "text"])]
        r#type: String,

        /// Table to index (`catalog.schema.table`, or `schema.table` with an active database)
        #[arg(long = "from")]
        from: String,

        /// Column to index
        #[arg(long)]
        column: String,

        /// Distance metric for vector indexes
        #[arg(long, value_parser = ["l2", "cosine", "dot"])]
        metric: Option<String>,

        /// Embedding provider ID (vector over a text column → auto-embeddings)
        #[arg(long = "provider")]
        provider: Option<String>,

        /// Create as a background job
        #[arg(long)]
        r#async: bool,
    },

    /// List search indexes
    List {
        /// Filter by schema name
        #[arg(long)]
        schema: Option<String>,

        /// Filter by table name
        #[arg(long)]
        table: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Show one search index by name
    Show {
        /// Index name
        name: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Remove a search index by name
    Remove {
        /// Index name
        name: String,
    },

    /// Manage embedding providers — the models behind vector search
    Embeddings {
        #[command(subcommand)]
        command: EmbeddingProvidersCommands,
    },
}

pub fn dispatch(workspace_id: &str, command: SearchCommands) {
    match command {
        SearchCommands::Create {
            name,
            r#type,
            from,
            column,
            metric,
            provider,
            r#async,
        } => create(
            workspace_id,
            name.as_deref(),
            &r#type,
            &from,
            &column,
            metric.as_deref(),
            provider.as_deref(),
            r#async,
        ),
        SearchCommands::List {
            schema,
            table,
            output,
        } => list(workspace_id, schema.as_deref(), table.as_deref(), &output),
        SearchCommands::Show { name, output } => show(workspace_id, &name, &output),
        SearchCommands::Remove { name } => remove(workspace_id, &name),
        SearchCommands::Embeddings { command } => {
            embedding_providers::dispatch(workspace_id, command)
        }
    }
}

/// Parse `catalog.schema.table` or `schema.table` (needs an active database) into
/// (catalog/connection name, schema, table). Exits with a message on a bad shape.
fn parse_table(workspace_id: &str, table: &str) -> (String, String, String) {
    use crossterm::style::Stylize;
    let parts: Vec<&str> = table.splitn(3, '.').collect();
    match parts.as_slice() {
        [catalog, schema, tbl] => (catalog.to_string(), schema.to_string(), tbl.to_string()),
        [schema, tbl] => {
            let db_id = crate::config::load_current_database("default", workspace_id)
                .unwrap_or_else(|| {
                    eprintln!(
                        "{}",
                        "error: use catalog.schema.table, or set an active database \
                         with `hotdata databases use <id>`."
                            .red()
                    );
                    std::process::exit(1);
                });
            let api = Api::new(Some(workspace_id));
            let db = databases::get_database(&api, &db_id).unwrap_or_else(|e| e.exit());
            let catalog = db
                .default_catalog
                .unwrap_or_else(|| db.name.unwrap_or_else(|| "default".to_string()));
            (catalog, schema.to_string(), tbl.to_string())
        }
        _ => {
            eprintln!(
                "{}",
                "error: --from must be 'schema.table' or 'catalog.schema.table'".red()
            );
            std::process::exit(1);
        }
    }
}

/// Build the SQL a search runs: `bm25_search(...)` for text, server-side
/// `vector_distance(...)` for vector. Shared by [`query_index`] and [`legacy`].
fn build_search_sql(
    index_type: &str,
    table_fqn: &str,
    column: &str,
    query: &str,
    select: Option<&str>,
    limit: u32,
) -> String {
    match index_type {
        "bm25" => {
            let bm25_columns = match select {
                Some(cols) if cols.split(',').any(|c| c.trim() == "score") => cols.to_string(),
                Some(cols) => format!("{}, score", cols),
                None => "*".to_string(),
            };
            format!(
                "SELECT {} FROM bm25_search('{}', '{}', '{}') ORDER BY score DESC LIMIT {}",
                bm25_columns,
                table_fqn.replace('\'', "''"),
                column.replace('\'', "''"),
                query.replace('\'', "''"),
                limit,
            )
        }
        // Server-side vector_distance resolves the embedding column, model, and
        // metric from the index metadata; the caller names the source column.
        _ => format!(
            "SELECT {}, vector_distance({}, '{}') AS dist FROM {} ORDER BY dist LIMIT {}",
            select.unwrap_or("*"),
            column,
            query.replace('\'', "''"),
            table_fqn,
            limit,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn create(
    workspace_id: &str,
    name: Option<&str>,
    type_: &str,
    from: &str,
    column: &str,
    metric: Option<&str>,
    provider: Option<&str>,
    async_mode: bool,
) {
    let index_type = if type_ == "text" { "bm25" } else { "vector" };
    let (conn_name, schema, table) = parse_table(workspace_id, from);
    let api = Api::new(Some(workspace_id));
    let conn_id = connections::resolve_connection_id(&api, &conn_name);
    let auto_name = format!("{table}_{}_{index_type}", column.replace(',', "_"));
    let index_name = name.unwrap_or(auto_name.as_str());
    indexes::create(
        workspace_id,
        IndexScope::Connection {
            connection_id: &conn_id,
            schema: &schema,
            table: &table,
        },
        index_name,
        column,
        index_type,
        metric,
        async_mode,
        provider,
        None,
        None,
        None,
    );
}

fn list(workspace_id: &str, schema: Option<&str>, table: Option<&str>, output: &str) {
    let api = Api::new(Some(workspace_id));
    let connection_id =
        crate::config::load_current_database("default", workspace_id).and_then(|db_id| {
            databases::get_database(&api, &db_id)
                .ok()
                .map(|db| db.default_connection_id)
        });
    indexes::list(
        workspace_id,
        connection_id.as_deref(),
        schema,
        table,
        output,
    );
}

fn locate_or_exit(workspace_id: &str, name: &str) -> indexes::LocatedIndex {
    indexes::locate_by_name(workspace_id, name).unwrap_or_else(|e| {
        use crossterm::style::Stylize;
        eprintln!("{}", e.red());
        std::process::exit(1);
    })
}

fn show(workspace_id: &str, name: &str, output: &str) {
    let loc = locate_or_exit(workspace_id, name);
    let kind = if loc.index_type == "bm25" {
        "text"
    } else {
        "vector"
    };
    let table_fqn = format!("{}.{}.{}", loc.connection, loc.schema, loc.table);
    let v = serde_json::json!({
        "name": name,
        "kind": kind,
        "index_type": loc.index_type,
        "table": table_fqn,
        "column": loc.search_column,
        "metric": loc.metric,
        "status": loc.status,
    });
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(&v).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&v).unwrap()),
        _ => {
            println!("name:    {name}");
            println!("kind:    {kind} ({})", loc.index_type);
            println!("table:   {table_fqn}");
            println!("column:  {}", loc.search_column);
            if let Some(m) = &loc.metric {
                println!("metric:  {m}");
            }
            println!("status:  {}", loc.status);
        }
    }
}

/// Run a search against the named index (`search "text" --index <name>`).
pub fn run(
    workspace_id: &str,
    name: &str,
    text: &str,
    select: Option<&str>,
    limit: u32,
    output: &str,
) {
    let loc = locate_or_exit(workspace_id, name);
    let table_fqn = format!("{}.{}.{}", loc.connection, loc.schema, loc.table);
    let sql = build_search_sql(
        &loc.index_type,
        &table_fqn,
        &loc.search_column,
        text,
        select,
        limit,
    );
    query::execute(&sql, workspace_id, None, output);
}

fn remove(workspace_id: &str, name: &str) {
    let loc = locate_or_exit(workspace_id, name);
    let api = Api::new(Some(workspace_id));
    let conn_id = connections::resolve_connection_id(&api, &loc.connection);
    indexes::delete(
        workspace_id,
        IndexScope::Connection {
            connection_id: &conn_id,
            schema: &loc.schema,
            table: &loc.table,
        },
        name,
    );
}
