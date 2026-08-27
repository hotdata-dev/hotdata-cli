//! `hotdata search` — search indexes as named objects.
//!
//! A reshape of the former flag-based `search`, plus `indexes` and
//! `embedding-providers`, into one namespace. The search action is
//! `search "<text>" --index <name>`; index management lives under `search
//! create|list|show|remove`, and `--type text|vector|sorted` maps onto the
//! underlying `bm25` / `vector` / `sorted` index types.

use crate::client::sdk::Api;
use crate::commands::embedding_providers::{self, EmbeddingProvidersCommands};
use crate::commands::indexes::{self, IndexScope};
use crate::commands::{databases, query};

/// Subcommands for `hotdata search`.
#[derive(clap::Subcommand)]
pub enum SearchCommands {
    /// Create a search index over a table column
    Create {
        /// Index name (derived from table, column, and type if omitted)
        name: Option<String>,

        /// Search type: `vector` (semantic), `text` (BM25 full-text), or `sorted`
        #[arg(long, value_parser = ["vector", "text", "sorted"])]
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

        /// Override embedding output dimensions (vector auto-embed only)
        #[arg(long)]
        dimensions: Option<u32>,

        /// Custom name for the generated embedding column (defaults to `{column}_embedding`)
        #[arg(long = "output-column")]
        output_column: Option<String>,

        /// Human-readable description of the embedding (e.g. "product titles")
        #[arg(long)]
        description: Option<String>,

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

        /// Database the index lives in (id; defaults to the active database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Remove a search index by name
    Remove {
        /// Index name
        name: String,

        /// Database the index lives in (id; defaults to the active database)
        #[arg(long, short = 'd')]
        database: Option<String>,
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
            dimensions,
            output_column,
            description,
            r#async,
        } => create(
            workspace_id,
            name.as_deref(),
            &r#type,
            &from,
            &column,
            metric.as_deref(),
            provider.as_deref(),
            dimensions,
            output_column.as_deref(),
            description.as_deref(),
            r#async,
        ),
        SearchCommands::List {
            schema,
            table,
            output,
        } => list(workspace_id, schema.as_deref(), table.as_deref(), &output),
        SearchCommands::Show {
            name,
            database,
            output,
        } => show(workspace_id, database.as_deref(), &name, &output),
        SearchCommands::Remove { name, database } => {
            remove(workspace_id, database.as_deref(), &name)
        }
        SearchCommands::Embeddings { command } => {
            embedding_providers::dispatch(workspace_id, command)
        }
    }
}

/// The instant database an index create targets, as named by `--from`.
enum FromTarget {
    /// A `schema.table` `--from`: the active database, already resolved by id.
    /// Carry the resolved database so `create` does **not** re-resolve it by
    /// catalog — a fork shares its source's catalog alias, so a catalog lookup
    /// is ambiguous even though the active-database id is unambiguous.
    Database(Box<databases::Database>),
    /// A `catalog.schema.table` `--from`: an explicit catalog alias still to be
    /// resolved to an instant database.
    Catalog(String),
}

/// Parse `catalog.schema.table` or `schema.table` (needs an active database) into
/// (target database, schema, table). Exits with a message on a bad shape.
fn parse_table(workspace_id: &str, table: &str) -> (FromTarget, String, String) {
    use crossterm::style::Stylize;
    let parts: Vec<&str> = table.splitn(3, '.').collect();
    match parts.as_slice() {
        [catalog, schema, tbl] => (
            FromTarget::Catalog(catalog.to_string()),
            schema.to_string(),
            tbl.to_string(),
        ),
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
            (
                FromTarget::Database(Box::new(db)),
                schema.to_string(),
                tbl.to_string(),
            )
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
/// `vector_distance(...)` for vector.
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
    dimensions: Option<u32>,
    output_column: Option<&str>,
    description: Option<&str>,
    async_mode: bool,
) {
    let index_type = match type_ {
        "text" => "bm25",
        "sorted" => "sorted",
        _ => "vector",
    };
    let (target, schema, table) = parse_table(workspace_id, from);
    let api = Api::new(Some(workspace_id));
    // Indexes are an instant-database concept (a plain connection is a legacy
    // concept being removed), so create must land on an instant database — the
    // same scope `search show`/`search remove` address. The active-database path
    // is already resolved; only an explicit catalog still needs resolving, and
    // its own error (e.g. an ambiguous forked-catalog alias) is surfaced as-is.
    let db = match target {
        FromTarget::Database(db) => *db,
        // Prefer the active database when the catalog is ambiguous — the same
        // rule `databases load --catalog` applies — so a `--from` naming the
        // active database's own catalog (e.g. `default` shared across forks)
        // resolves to it instead of erroring.
        FromTarget::Catalog(catalog) => {
            databases::try_resolve_database_preferring_active(&api, &catalog).unwrap_or_else(|e| {
                use crossterm::style::Stylize;
                eprintln!(
                    "{}",
                    format!(
                        "error: {e}\nSearch indexes are created on instant databases — pass an \
                         instant database's catalog or id, or 'schema.table' with an active \
                         database set via 'hotdata databases use <id>'."
                    )
                    .red()
                );
                std::process::exit(1);
            })
        }
    };
    let conn_id = db.default_connection_id;
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
        dimensions,
        output_column,
        description,
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

fn locate_or_exit(workspace_id: &str, database: Option<&str>, name: &str) -> indexes::LocatedIndex {
    // `--database` accepts a catalog or name as well as an id, like every
    // other database flag; locate_by_name needs the id.
    let database = databases::resolve_database_flag(workspace_id, database);
    indexes::locate_by_name(workspace_id, database.as_deref(), name).unwrap_or_else(|e| {
        use crossterm::style::Stylize;
        eprintln!("{}", e.red());
        std::process::exit(1);
    })
}

fn show(workspace_id: &str, database: Option<&str>, name: &str, output: &str) {
    let loc = locate_or_exit(workspace_id, database, name);
    let kind = match loc.index_type.as_str() {
        "bm25" => "text",
        "sorted" => "sorted",
        _ => "vector",
    };
    let table_fqn = format!("{}.{}.{}", loc.catalog, loc.schema, loc.table);
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
    database: Option<&str>,
    name: &str,
    text: &str,
    select: Option<&str>,
    limit: u32,
    output: &str,
) {
    let loc = locate_or_exit(workspace_id, database, name);
    if loc.index_type == "sorted" {
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!(
                "error: index '{name}' is a sorted index — not searchable. Use 'hotdata query' \
                 with a WHERE/ORDER BY filter instead."
            )
            .red()
        );
        std::process::exit(1);
    }
    let table_fqn = format!("{}.{}.{}", loc.catalog, loc.schema, loc.table);
    let sql = build_search_sql(
        &loc.index_type,
        &table_fqn,
        &loc.search_column,
        text,
        select,
        limit,
    );
    // Search generates HotSQL directly — never a foreign dialect.
    query::execute(&sql, workspace_id, Some(&loc.database_id), output, "hotsql");
}

fn remove(workspace_id: &str, database: Option<&str>, name: &str) {
    let loc = locate_or_exit(workspace_id, database, name);
    indexes::delete(
        workspace_id,
        IndexScope::Connection {
            connection_id: &loc.connection_id,
            schema: &loc.schema,
            table: &loc.table,
        },
        name,
    );
}
