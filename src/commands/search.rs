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

        /// Table to index (`catalog.schema.table`, or `schema.table` in --database
        /// or the active database)
        #[arg(long = "from")]
        from: String,

        /// Database to create the index in (id; defaults to the active database).
        /// Use with a `schema.table` --from.
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Column to index
        #[arg(long)]
        column: String,

        /// Distance metric for vector indexes
        #[arg(long, value_parser = ["l2", "cosine", "dot"])]
        metric: Option<String>,

        /// Vector index algorithm (default: hnsw). `hnsw` searches an in-memory
        /// graph whose memory grows with the table. `ivf` clusters the vectors
        /// and reads only the nearest clusters, so its memory follows how much
        /// a search reads rather than the table's size. `ivf` needs a column that already holds
        /// vectors and the `l2` or `cosine` metric.
        #[arg(long, value_parser = ["hnsw", "ivf"])]
        algorithm: Option<String>,

        /// Number of clusters for an `ivf` index, 1–65536 (default: chosen from
        /// the table's size). Requires `--algorithm ivf`.
        #[arg(long)]
        nlist: Option<u32>,

        /// Fraction of an `ivf` index a search reads, greater than 0 and at most
        /// 1 (default: server's choice). Higher finds more true neighbours and
        /// takes longer. Requires `--algorithm ivf`.
        #[arg(long = "probe-fraction")]
        probe_fraction: Option<f64>,

        /// How precisely the vector index stores each vector value (default:
        /// the column's precision for hnsw, `int8` for ivf). `ivf` accepts
        /// `int8` and `float32`; `hnsw` accepts all but `int8`.
        #[arg(long = "vector-precision", value_parser = ["float64", "float32", "float16", "float8", "int8"])]
        vector_precision: Option<String>,

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
            database,
            column,
            metric,
            algorithm,
            nlist,
            probe_fraction,
            vector_precision,
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
            database.as_deref(),
            &column,
            metric.as_deref(),
            &indexes::VectorIndexOptions {
                algorithm: algorithm.as_deref(),
                nlist,
                probe_fraction,
                vector_precision: vector_precision.as_deref(),
            },
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
    /// A `schema.table` `--from`: `--database` or the active database, already
    /// resolved by id.
    /// Carry the resolved database so `create` does **not** re-resolve it by
    /// catalog — a fork shares its source's catalog alias, so a catalog lookup
    /// is ambiguous even though the active-database id is unambiguous.
    Database(Box<databases::Database>),
    /// A `catalog.schema.table` `--from`: an explicit catalog alias still to be
    /// resolved to an instant database.
    Catalog(String),
}

/// Split a `--from` into (catalog, schema, table). The catalog is `None` for a
/// `schema.table`, which names a table in `--database` or the active database.
/// A catalog together with `--database` is refused: they would name the
/// database twice, possibly differently.
fn split_from(
    from: &str,
    database: Option<&str>,
) -> Result<(Option<String>, String, String), &'static str> {
    let parts: Vec<&str> = from.splitn(3, '.').collect();
    match parts.as_slice() {
        [_, _, _] if database.is_some() => Err(
            "error: --database takes a 'schema.table' --from; drop the catalog from --from, \
             or drop --database",
        ),
        [catalog, schema, tbl] => Ok((
            Some(catalog.to_string()),
            schema.to_string(),
            tbl.to_string(),
        )),
        [schema, tbl] => Ok((None, schema.to_string(), tbl.to_string())),
        _ => Err("error: --from must be 'schema.table' or 'catalog.schema.table'"),
    }
}

/// Parse `catalog.schema.table`, or `schema.table` in `database` (else the
/// active database), into (target database, schema, table). Exits with a
/// message on a bad shape or when no database is known.
fn parse_table(
    workspace_id: &str,
    table: &str,
    database: Option<&str>,
) -> (FromTarget, String, String) {
    use crossterm::style::Stylize;
    let (catalog, schema, tbl) = split_from(table, database).unwrap_or_else(|e| {
        eprintln!("{}", e.red());
        std::process::exit(1);
    });
    if let Some(catalog) = catalog {
        return (FromTarget::Catalog(catalog), schema, tbl);
    }
    let db_id = database
        .map(str::to_string)
        .or_else(|| crate::config::load_current_database("default", workspace_id))
        .unwrap_or_else(|| {
            eprintln!(
                "{}",
                "error: use catalog.schema.table, pass --database <id>, or set an active \
                 database with `hotdata databases use <id>`."
                    .red()
            );
            std::process::exit(1);
        });
    let api = Api::new(Some(workspace_id));
    let db = databases::get_database(&api, &db_id).unwrap_or_else(|e| e.exit());
    (FromTarget::Database(Box::new(db)), schema, tbl)
}

/// Quote a column for use inside a wildcard `EXCLUDE` list. Index columns are
/// user-named (`search create --output-column`), so they can need quoting and
/// can contain a quote character.
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The default projection for a search: everything the table has, minus the
/// columns the index generated.
///
/// An auto-embed vector index materialises a `{column}_embedding` column on the
/// table, so a bare `*` returns a 1536-float list in every row — tens of
/// kilobytes per search the caller did not ask for, and a column claiming
/// terminal width the real ones need. Excluding it by default keeps the wire
/// small and the row readable; `--select '*'` asks for it back, and naming it
/// in `--select` still works.
fn default_projection(generated_columns: &[String]) -> String {
    if generated_columns.is_empty() {
        return "*".to_string();
    }
    let excluded: Vec<String> = generated_columns.iter().map(|c| quote_ident(c)).collect();
    format!("* EXCLUDE ({})", excluded.join(", "))
}

/// Build the SQL a search runs: `bm25_search(...)` for text, server-side
/// `vector_distance(...)` for vector.
fn build_search_sql(
    index_type: &str,
    table_fqn: &str,
    column: &str,
    query: &str,
    select: Option<&str>,
    generated_columns: &[String],
    limit: u32,
) -> String {
    match index_type {
        "bm25" => {
            let bm25_columns = match select {
                Some(cols) if cols.split(',').any(|c| c.trim() == "score") => cols.to_string(),
                Some(cols) => format!("{}, score", cols),
                None => default_projection(generated_columns),
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
            select
                .map(str::to_string)
                .unwrap_or_else(|| default_projection(generated_columns)),
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
    database: Option<&str>,
    column: &str,
    metric: Option<&str>,
    vector: &indexes::VectorIndexOptions<'_>,
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
    let (target, schema, table) = parse_table(workspace_id, from, database);
    let api = Api::new(Some(workspace_id));
    // Indexes are an instant-database concept (a plain connection is a legacy
    // concept being removed), so create must land on an instant database — the
    // same scope `search show`/`search remove` address. The active-database path
    // is already resolved; only an explicit catalog still needs resolving, and
    // its own error (e.g. an ambiguous forked-catalog alias) is surfaced as-is.
    let db = match target {
        FromTarget::Database(db) => *db,
        FromTarget::Catalog(catalog) => databases::try_resolve_database(&api, &catalog)
            .unwrap_or_else(|e| {
                use crossterm::style::Stylize;
                eprintln!(
                    "{}",
                    format!(
                        "error: {e}\nSearch indexes are created on instant databases — pass a \
                         instant database's catalog or id, or 'schema.table' with an active \
                         database set via 'hotdata databases use <id>'."
                    )
                    .red()
                );
                std::process::exit(1);
            }),
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
        vector,
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
    indexes::locate_by_name(workspace_id, database, name).unwrap_or_else(|e| {
        use crossterm::style::Stylize;
        eprintln!("{}", e.red());
        std::process::exit(1);
    })
}

fn show(workspace_id: &str, database: Option<&str>, name: &str, output: &str) {
    let loc = locate_or_exit(workspace_id, database, name);
    match output {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&show_value(name, &loc)).unwrap()
        ),
        "yaml" => print!(
            "{}",
            serde_yaml::to_string(&show_value(name, &loc)).unwrap()
        ),
        _ => {
            for line in show_lines(name, &loc) {
                println!("{line}");
            }
        }
    }
}

/// The `search` kind (`text` / `sorted` / `vector`) for an API index type.
fn search_kind(index_type: &str) -> &'static str {
    match index_type {
        "bm25" => "text",
        "sorted" => "sorted",
        _ => "vector",
    }
}

/// `search show -o json|yaml`. The vector fields are `null` when the API
/// leaves them out (non-vector indexes, or a server default applies).
fn show_value(name: &str, loc: &indexes::LocatedIndex) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "kind": search_kind(&loc.index_type),
        "index_type": loc.index_type,
        "table": format!("{}.{}.{}", loc.catalog, loc.schema, loc.table),
        "column": loc.search_column,
        "metric": loc.metric,
        "algorithm": loc.algorithm,
        "probe_fraction": loc.probe_fraction,
        "vector_precision": loc.vector_precision,
        "status": loc.status,
    })
}

/// `search show` table output: one `key: value` line per field the index has.
fn show_lines(name: &str, loc: &indexes::LocatedIndex) -> Vec<String> {
    let mut lines = vec![
        format!("name:             {name}"),
        format!(
            "kind:             {} ({})",
            search_kind(&loc.index_type),
            loc.index_type
        ),
        format!(
            "table:            {}.{}.{}",
            loc.catalog, loc.schema, loc.table
        ),
        format!("column:           {}", loc.search_column),
    ];
    if let Some(m) = &loc.metric {
        lines.push(format!("metric:           {m}"));
    }
    if let Some(a) = &loc.algorithm {
        lines.push(format!("algorithm:        {a}"));
    }
    if let Some(f) = loc.probe_fraction {
        lines.push(format!("probe_fraction:   {f}"));
    }
    if let Some(p) = &loc.vector_precision {
        lines.push(format!("vector_precision: {p}"));
    }
    lines.push(format!("status:           {}", loc.status));
    lines
}

/// The SQL distance function for a vector index's metric. The API defaults a
/// vector index with no recorded metric to `l2`.
fn distance_function(metric: Option<&str>) -> &'static str {
    match metric {
        Some("cosine") => "cosine_distance",
        Some("dot") => "negative_dot_product",
        _ => "l2_distance",
    }
}

/// Why `search "<text>"` cannot run against an `ivf` index, and the SQL that
/// queries it instead. An `ivf` index is built over a column that already
/// holds vectors, so there is no embedding model to turn the search text into
/// a query vector; the caller supplies one.
fn ivf_search_error(name: &str, loc: &indexes::LocatedIndex, limit: u32) -> String {
    format!(
        "error: index '{name}' is an ivf index, which 'hotdata search' cannot query: it \
         indexes a column that already holds vectors, so there is no model to embed the \
         search text. Query it with SQL and your own query vector, ordering by the \
         index's distance function:\n  hotdata query 'SELECT * FROM {}.{}.{} ORDER BY \
         {}({}, [<query vector>]) LIMIT {limit}'",
        loc.catalog,
        loc.schema,
        loc.table,
        distance_function(loc.metric.as_deref()),
        loc.search_column,
    )
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
    if loc.algorithm.as_deref() == Some("ivf") {
        use crossterm::style::Stylize;
        eprintln!("{}", ivf_search_error(name, &loc, limit).red());
        std::process::exit(1);
    }
    let table_fqn = format!("{}.{}.{}", loc.catalog, loc.schema, loc.table);
    let sql = build_search_sql(
        &loc.index_type,
        &table_fqn,
        &loc.search_column,
        text,
        select,
        &loc.generated_columns,
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Wrapper {
        #[command(subcommand)]
        cmd: SearchCommands,
    }

    fn parse(args: &[&str]) -> Result<SearchCommands, clap::Error> {
        Wrapper::try_parse_from(std::iter::once("t").chain(args.iter().copied())).map(|w| w.cmd)
    }

    const CREATE_VECTOR: &[&str] = &[
        "create",
        "--type",
        "vector",
        "--from",
        "db.public.docs",
        "--column",
        "embedding",
    ];

    fn ivf_located() -> indexes::LocatedIndex {
        indexes::LocatedIndex {
            database_id: "db1".into(),
            connection_id: "conn1".into(),
            catalog: "shop".into(),
            schema: "public".into(),
            table: "docs".into(),
            index_type: "vector".into(),
            search_column: "embedding".into(),
            generated_columns: Vec::new(),
            status: "ready".into(),
            metric: Some("cosine".into()),
            algorithm: Some("ivf".into()),
            probe_fraction: Some(0.05),
            vector_precision: Some("int8".into()),
        }
    }

    #[test]
    fn show_reports_vector_index_settings() {
        let loc = ivf_located();
        let lines = show_lines("docs_ivf", &loc);
        assert!(
            lines.contains(&"algorithm:        ivf".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"probe_fraction:   0.05".to_string()),
            "{lines:?}"
        );
        assert!(
            lines.contains(&"vector_precision: int8".to_string()),
            "{lines:?}"
        );

        let v = show_value("docs_ivf", &loc);
        assert_eq!(v["algorithm"], "ivf");
        assert_eq!(v["probe_fraction"], 0.05);
        assert_eq!(v["vector_precision"], "int8");
        assert_eq!(v["table"], "shop.public.docs");
    }

    #[test]
    fn show_omits_vector_settings_the_index_does_not_have() {
        let loc = indexes::LocatedIndex {
            index_type: "bm25".into(),
            search_column: "body".into(),
            metric: None,
            algorithm: None,
            probe_fraction: None,
            vector_precision: None,
            ..ivf_located()
        };
        let lines = show_lines("docs_body", &loc).join("\n");
        for key in [
            "metric:",
            "algorithm:",
            "probe_fraction:",
            "vector_precision:",
        ] {
            assert!(!lines.contains(key), "{key} in {lines}");
        }
        assert!(show_value("docs_body", &loc)["algorithm"].is_null());
    }

    #[test]
    fn ivf_search_error_points_at_sql_with_the_metric_distance_function() {
        let msg = ivf_search_error("docs_ivf", &ivf_located(), 10);
        assert!(msg.contains("ivf index"), "{msg}");
        assert!(
            msg.contains(
                "SELECT * FROM shop.public.docs ORDER BY cosine_distance(embedding, [<query vector>]) LIMIT 10"
            ),
            "{msg}"
        );
    }

    #[test]
    fn distance_function_follows_the_metric() {
        assert_eq!(distance_function(Some("l2")), "l2_distance");
        assert_eq!(distance_function(Some("cosine")), "cosine_distance");
        assert_eq!(distance_function(Some("dot")), "negative_dot_product");
        // The API's default metric for a vector index is l2.
        assert_eq!(distance_function(None), "l2_distance");
    }

    #[test]
    fn split_from_accepts_both_shapes() {
        assert_eq!(
            split_from("shop.public.docs", None).unwrap(),
            (Some("shop".into()), "public".into(), "docs".into())
        );
        assert_eq!(
            split_from("public.docs", Some("db1")).unwrap(),
            (None, "public".into(), "docs".into())
        );
        assert_eq!(
            split_from("public.docs", None).unwrap(),
            (None, "public".into(), "docs".into())
        );
    }

    #[test]
    fn split_from_refuses_a_catalog_with_database_and_bad_shapes() {
        assert!(split_from("shop.public.docs", Some("db1")).is_err());
        assert!(split_from("docs", None).is_err());
    }

    #[test]
    fn create_accepts_database_flag() {
        let cmd = parse(&[
            "create",
            "--type",
            "vector",
            "--from",
            "public.docs",
            "--column",
            "embedding",
            "-d",
            "db1",
        ])
        .unwrap();
        assert!(matches!(cmd, SearchCommands::Create { database: Some(ref d), .. } if d == "db1"));
    }

    #[test]
    fn create_leaves_vector_options_unset_by_default() {
        match parse(CREATE_VECTOR).unwrap() {
            SearchCommands::Create {
                algorithm,
                nlist,
                probe_fraction,
                vector_precision,
                ..
            } => {
                assert_eq!(algorithm, None);
                assert_eq!(nlist, None);
                assert_eq!(probe_fraction, None);
                assert_eq!(vector_precision, None);
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_parses_ivf_options() {
        let mut args = CREATE_VECTOR.to_vec();
        args.extend([
            "--algorithm",
            "ivf",
            "--nlist",
            "1024",
            "--probe-fraction",
            "0.05",
            "--vector-precision",
            "int8",
        ]);
        match parse(&args).unwrap() {
            SearchCommands::Create {
                algorithm,
                nlist,
                probe_fraction,
                vector_precision,
                ..
            } => {
                assert_eq!(algorithm.as_deref(), Some("ivf"));
                assert_eq!(nlist, Some(1024));
                assert_eq!(probe_fraction, Some(0.05));
                assert_eq!(vector_precision.as_deref(), Some("int8"));
            }
            _ => panic!("expected Create"),
        }
    }

    #[test]
    fn create_rejects_unknown_algorithm_and_precision() {
        let mut args = CREATE_VECTOR.to_vec();
        args.extend(["--algorithm", "flat"]);
        assert!(parse(&args).is_err());

        let mut args = CREATE_VECTOR.to_vec();
        args.extend(["--vector-precision", "int4"]);
        assert!(parse(&args).is_err());
    }

    const EMB: &str = "txt_embedding";

    fn vector_sql(select: Option<&str>, generated: &[String]) -> String {
        build_search_sql(
            "vector",
            "cat.public.d",
            "txt",
            "puppy",
            select,
            generated,
            10,
        )
    }

    #[test]
    fn vector_search_excludes_the_generated_embedding_column() {
        let sql = vector_sql(None, &[EMB.to_string()]);
        assert!(
            sql.starts_with(r#"SELECT * EXCLUDE ("txt_embedding"), vector_distance("#),
            "sql: {sql}"
        );
    }

    #[test]
    fn a_direct_vector_index_generates_nothing_so_the_star_stays_bare() {
        // No embedding column was materialised, so there is nothing to exclude
        // and the projection must not grow an empty EXCLUDE list.
        let sql = vector_sql(None, &[]);
        assert!(sql.starts_with("SELECT *, vector_distance("), "sql: {sql}");
        assert!(!sql.contains("EXCLUDE"), "sql: {sql}");
    }

    #[test]
    fn an_explicit_select_is_honoured_verbatim() {
        // `--select '*'` is the documented way to ask for the embedding back,
        // so it must survive untouched even when a generated column exists.
        let sql = vector_sql(Some("*"), &[EMB.to_string()]);
        assert!(sql.starts_with("SELECT *, vector_distance("), "sql: {sql}");
        assert!(!sql.contains("EXCLUDE"), "sql: {sql}");

        // And naming the embedding column explicitly still reaches it.
        let sql = vector_sql(Some("id, txt_embedding"), &[EMB.to_string()]);
        assert!(
            sql.starts_with("SELECT id, txt_embedding, vector_distance("),
            "sql: {sql}"
        );
    }

    #[test]
    fn bm25_search_keeps_a_bare_star_and_the_score() {
        // A BM25 index generates no columns, so its projection never grows an
        // EXCLUDE list. Nor can another index supply one: `locate_by_name`
        // reports only the named index's own generated columns, and the server
        // refuses to put an embedding-backed vector index on a table that
        // carries any other index ("Embedding-backed vector indexes cannot
        // coexist with other indexes on the same table"). Both arms of that
        // are why this stays `*`.
        let sql = build_search_sql("bm25", "cat.public.d", "body", "puppy", None, &[], 10);
        assert!(sql.starts_with("SELECT * FROM bm25_search("), "sql: {sql}");
        assert!(!sql.contains("EXCLUDE"), "sql: {sql}");

        // An explicit --select still gets `score` appended, unchanged.
        let sql = build_search_sql("bm25", "cat.public.d", "body", "puppy", Some("id"), &[], 10);
        assert!(
            sql.starts_with("SELECT id, score FROM bm25_search("),
            "sql: {sql}"
        );
    }

    #[test]
    fn the_projection_helper_is_shared_and_general() {
        // `default_projection` serves both index kinds, so it is specified on
        // its own rather than only through the branch that can reach it today.
        assert_eq!(default_projection(&[]), "*");
        assert_eq!(
            default_projection(&["txt_embedding".to_string()]),
            r#"* EXCLUDE ("txt_embedding")"#
        );
    }

    #[test]
    fn a_generated_column_name_is_quoted_for_the_exclude_list() {
        // Index columns are user-named via `search create --output-column`, so
        // a name needing quotes (or containing one) must not break the SQL.
        assert_eq!(quote_ident("plain"), r#""plain""#);
        assert_eq!(quote_ident("has space"), r#""has space""#);
        assert_eq!(quote_ident(r#"we"ird"#), r#""we""ird""#);
    }

    #[test]
    fn several_generated_columns_are_all_excluded() {
        let sql = vector_sql(
            None,
            &["a_embedding".to_string(), "b_embedding".to_string()],
        );
        assert!(
            sql.contains(r#"* EXCLUDE ("a_embedding", "b_embedding")"#),
            "sql: {sql}"
        );
    }
}
