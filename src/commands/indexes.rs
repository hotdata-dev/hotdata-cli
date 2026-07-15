use crate::client::sdk::{Api, ApiError, block, block_with_wakeup, none_if_404};
use crate::commands::databases;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::ControlFlow;

/// Subcommands for `hotdata indexes`.
#[derive(clap::Subcommand)]
pub enum IndexesCommands {
    /// List indexes in the active database, or all workspace indexes if none is set
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

    /// Create an index on a table.
    Create {
        /// SQL catalog alias of the target database (e.g. `--catalog airbnb`)
        #[arg(long)]
        catalog: Option<String>,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table name to index
        #[arg(long)]
        table: Option<String>,

        /// Column(s) to index (comma-separated)
        #[arg(long)]
        column: Option<String>,

        /// Index name (derived from table, columns, and type if omitted)
        #[arg(long)]
        name: Option<String>,

        /// Index type — required (no default; choose deliberately)
        #[arg(long, value_parser = ["sorted", "bm25", "vector"])]
        r#type: String,

        /// Distance metric for vector indexes
        #[arg(long, value_parser = ["l2", "cosine", "dot"])]
        metric: Option<String>,

        /// Create as a background job
        #[arg(long)]
        r#async: bool,

        /// Embedding provider ID — when set on a vector index over a text column,
        /// embeddings are generated automatically. Defaults to first system provider if omitted.
        #[arg(long = "embedding-provider-id")]
        embedding_provider_id: Option<String>,

        /// Override embedding output dimensions (vector indexes with auto-embedding only)
        #[arg(long)]
        dimensions: Option<u32>,

        /// Custom name for the generated embedding column (defaults to `{column}_embedding`)
        #[arg(long = "output-column")]
        output_column: Option<String>,

        /// Human-readable description of the embedding (e.g. "product titles")
        #[arg(long)]
        description: Option<String>,
    },

    /// Delete an index from a table
    Delete {
        /// SQL catalog alias of the target database or connection name (same flag as `indexes create`)
        #[arg(long)]
        catalog: String,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table name
        #[arg(long)]
        table: String,

        /// Index name
        #[arg(long)]
        name: String,
    },
}

#[derive(Deserialize, Serialize)]
struct Index {
    index_name: String,
    index_type: String,
    columns: Vec<String>,
    metric: Option<String>,
    /// Source text column for an embedding-backed vector index. Queries name it
    /// in `vector_distance(<source_column>, …)`, whereas `columns` holds the
    /// generated embedding column. Absent for BM25, sorted, and direct
    /// (existing-column) vector indexes. Older servers omit it entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_column: Option<String>,
    status: String,
    created_at: String,
    updated_at: String,
}

impl Index {
    /// Column a search query targets: the embedding **source** column when the
    /// index is auto-embed (`source_column` set), otherwise the first indexed
    /// column. For auto-embed indexes `columns` holds the generated embedding
    /// column, which the server's `vector_distance` rewrite does not match —
    /// the source column is what callers must name.
    fn search_column(&self) -> Option<String> {
        self.source_column
            .clone()
            .or_else(|| self.columns.first().cloned())
    }

    /// Whether `column` identifies this index for search: matches the embedding
    /// source column or any indexed column. Lets `--column <source>` resolve an
    /// auto-embed index even though the source column is not in `columns`.
    fn matches_search_column(&self, column: &str) -> bool {
        self.source_column.as_deref() == Some(column) || self.columns.iter().any(|c| c == column)
    }
}

#[derive(Serialize)]
struct IndexRow {
    #[serde(flatten)]
    inner: Index,
    #[serde(skip_serializing_if = "Option::is_none")]
    table: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse {
    indexes: Vec<Index>,
}

#[derive(Deserialize)]
struct InfoTable {
    connection: String,
    schema: String,
    table: String,
}

#[derive(Deserialize)]
struct ConnectionRef {
    id: String,
    name: String,
}

fn connection_label_to_id_map(connections: &[ConnectionRef]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for c in connections {
        m.insert(c.name.clone(), c.id.clone());
    }
    m
}

fn connection_lookup(api: &Api) -> Result<HashMap<String, String>, ApiError> {
    let resp = block(api.client().connections().list())?;
    let refs: Vec<ConnectionRef> = resp
        .connections
        .into_iter()
        .map(|c| ConnectionRef {
            id: c.id,
            name: c.name,
        })
        .collect();
    Ok(connection_label_to_id_map(&refs))
}

/// Pick the connection id to address a per-table index call with during a
/// connection-wide scan.
///
/// Prefers the caller-supplied `--connection-id`: it always resolves, including
/// for a database-scoped connection whose `information_schema` `label`
/// (`__db_*`) is absent from `connections list` (that listing hides
/// database-scoped connections, so `name_to_id` can't map it — #161). The scan's
/// tables are already filtered to that connection, so the supplied id is correct
/// for every row. With no `--connection-id` (the list-everything case), maps the
/// label back to an id, falling back to the label itself.
fn scan_connection_id<'a>(
    supplied: Option<&'a str>,
    label: &'a str,
    name_to_id: &'a HashMap<String, String>,
) -> &'a str {
    supplied
        .or_else(|| name_to_id.get(label).map(String::as_str))
        .unwrap_or(label)
}

/// One table to scan for indexes, paired with the connection id its per-table
/// index call must address. The `table.connection` field carries the display
/// label (a connection name, or a managed database's internal `__db_*` label),
/// which can differ from the real `conn_id` used for the API call.
struct ScanTarget {
    conn_id: String,
    table: InfoTable,
}

/// Resolve the `default_connection_id` of every managed database in the
/// workspace, in parallel.
///
/// These are exactly the connections the whole-workspace `information_schema`
/// enumeration omits and `connections list` hides (#168), so the unscoped scan
/// can't discover them any other way. `databases list` summaries don't carry the
/// connection id, so each database needs a `databases get`; a database deleted
/// between the list and the get (404) is skipped, any other error surfaces
/// loudly to match the rest of this path.
fn managed_db_connection_ids(api: &Api) -> Result<Vec<String>, ApiError> {
    let ids = databases::list_database_ids(api)?;
    let conn_ids: Result<Vec<Option<String>>, ApiError> = ids
        .par_iter()
        .map(|id| {
            Ok(none_if_404(databases::get_database(api, id))?.map(|db| db.default_connection_id))
        })
        .collect();
    Ok(conn_ids?.into_iter().flatten().collect())
}

/// Build the per-table scan list for a whole-workspace (unscoped) `indexes
/// list`.
///
/// The workspace-wide `information_schema` enumeration returns only
/// regular-connection tables — managed-database catalogs never appear there, and
/// `connections list` hides their connections (#168). So managed databases are
/// discovered separately via [`managed_db_connection_ids`] and each is scanned
/// with a connection-scoped `information_schema` call, exactly like the
/// `--connection-id` path. The two table sets are disjoint: a managed database's
/// connection is never returned by `connections list`.
fn workspace_scan_targets(
    api: &Api,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<Vec<ScanTarget>, ApiError> {
    // Regular connections: one workspace-wide enumeration, label (= connection
    // name) mapped back to its id, falling back to the label itself (#161).
    let name_to_id = connection_lookup(api)?;
    let mut targets: Vec<ScanTarget> = collect_tables(api, None, schema, table)?
        .into_iter()
        .map(|t| {
            let conn_id = scan_connection_id(None, &t.connection, &name_to_id).to_string();
            ScanTarget { conn_id, table: t }
        })
        .collect();

    // Managed databases: discover their hidden connections, then scan each
    // scoped (the per-connection enumeration is what surfaces `__db_*` tables).
    let db_conns = managed_db_connection_ids(api)?;
    let managed: Result<Vec<Vec<ScanTarget>>, ApiError> = db_conns
        .par_iter()
        .map(|conn| {
            collect_tables(api, Some(conn), schema, table).map(|tables| {
                tables
                    .into_iter()
                    .map(|t| ScanTarget {
                        conn_id: conn.clone(),
                        table: t,
                    })
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    targets.extend(managed?.into_iter().flatten());
    Ok(targets)
}

/// Gather index rows across a connection's (or the workspace's) tables — the
/// `indexes list` path when no full `connection.schema.table` triple is given.
///
/// With a `--connection-id`, enumerates that connection's tables and fetches
/// each table's indexes against it (the database-scoped case fixed in #161).
/// Without one, [`workspace_scan_targets`] assembles the list across both
/// regular connections and managed databases (#168). Skipped connections /
/// missing tables surface as no rows for that table, not an error.
fn collect_connection_wide(
    api: &Api,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<Vec<IndexRow>, ApiError> {
    let targets = match connection_id {
        Some(cid) => collect_tables(api, Some(cid), schema, table)?
            .into_iter()
            .map(|t| ScanTarget {
                conn_id: cid.to_string(),
                table: t,
            })
            .collect(),
        None => workspace_scan_targets(api, schema, table)?,
    };
    let per_table: Result<Vec<(String, Vec<Index>)>, ApiError> = targets
        .par_iter()
        .map(|tg| {
            let t = &tg.table;
            let full = format!("{}.{}.{}", t.connection, t.schema, t.table);
            let indexes = list_one_table_scan(api, &tg.conn_id, &t.schema, &t.table)?;
            Ok((full, indexes))
        })
        .collect();
    let mut rows: Vec<IndexRow> = Vec::new();
    for (full, indexes) in per_table? {
        for i in indexes {
            rows.push(IndexRow {
                inner: i,
                table: Some(full.clone()),
            });
        }
    }
    Ok(rows)
}

/// How to continue after merging one `/information_schema` page.
fn information_schema_followup(
    has_more: bool,
    next_cursor: Option<String>,
) -> ControlFlow<(), String> {
    if !has_more {
        return ControlFlow::Break(());
    }
    let Some(c) = next_cursor else {
        return ControlFlow::Break(());
    };
    ControlFlow::Continue(c)
}

fn sort_info_tables(tables: &mut [InfoTable]) {
    tables.sort_by(|a, b| {
        a.connection
            .cmp(&b.connection)
            .then_with(|| a.schema.cmp(&b.schema))
            .then_with(|| a.table.cmp(&b.table))
    });
}

fn collect_tables(
    api: &Api,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<Vec<InfoTable>, ApiError> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let resp = block(api.client().information_schema().get(
            connection_id,
            schema,
            table,
            None,
            None,
            cursor.as_deref(),
        ))?;
        out.extend(resp.tables.into_iter().map(|t| InfoTable {
            connection: t.connection,
            schema: t.schema,
            table: t.table,
        }));
        let next_cursor = resp.next_cursor.flatten();
        match information_schema_followup(resp.has_more, next_cursor) {
            ControlFlow::Break(()) => break,
            ControlFlow::Continue(c) => cursor = Some(c),
        }
    }
    sort_info_tables(&mut out);
    Ok(out)
}

fn list_one_table(
    api: &Api,
    connection_id: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<Index>, ApiError> {
    // The SDK's typed `IndexInfoResponse.status` is a closed `ready`/`pending`
    // enum; the CLI accepts any status string for display. Keep the CLI's own
    // tolerant deserialization via the seam's untyped GET escape hatch.
    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    let body: ListResponse = api.get_json(&path, &[])?;
    Ok(body.indexes)
}

fn list_one_table_scan(
    api: &Api,
    connection_id: &str,
    schema: &str,
    table: &str,
) -> Result<Vec<Index>, ApiError> {
    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    match none_if_404(api.get_json::<ListResponse>(&path, &[]))? {
        Some(body) => Ok(body.indexes),
        None => Ok(Vec::new()),
    }
}

/// Pure matching logic for search inference — extracted for testability.
///
/// Filters `indexes` to searchable types (`bm25`, `vector`), narrows by `hint_type` /
/// `hint_column` when provided, and returns `Ok((index_type, column))` on an unambiguous
/// match. Returns `Err(message)` on no match, multiple matches, or an index with no columns.
/// `location` is used only in error messages (e.g. `"mydb.public.listings"`).
fn resolve_search_params(
    indexes: &[Index],
    hint_type: Option<&str>,
    hint_column: Option<&str>,
    location: &str,
) -> Result<(String, String), String> {
    let matches: Vec<&Index> = indexes
        .iter()
        .filter(|i| {
            let t = i.index_type.as_str();
            (t == "bm25" || t == "vector")
                && hint_type.is_none_or(|ht| ht == t)
                && hint_column.is_none_or(|hc| i.matches_search_column(hc))
        })
        .collect();

    match matches.as_slice() {
        [] => {
            let what = match hint_type {
                Some(t) => format!("{} index", t),
                None => "BM25 or vector index".to_string(),
            };
            Err(format!(
                "No {} found on {} — run 'hotdata indexes create' first.",
                what, location
            ))
        }
        [one] => {
            let index_type = one.index_type.clone();
            let column = one
                .search_column()
                .ok_or_else(|| format!("Index '{}' has no columns.", one.index_name))?;
            Ok((index_type, column))
        }
        _ => {
            let types: Vec<&str> = matches.iter().map(|i| i.index_type.as_str()).collect();
            let cols: Vec<String> = matches
                .iter()
                .flat_map(|i| i.columns.iter().cloned())
                .collect();
            Err(format!(
                "Multiple search indexes found (types: {}, columns: {}) — specify --type and --column.",
                types.join(", "),
                cols.join(", ")
            ))
        }
    }
}

/// Infers `(index_type, column)` for `hotdata search` when `--type` or `--column` are omitted.
///
/// Fetches the indexes on `connection_name.schema.table`, filters to searchable types
/// (`bm25`, `vector`), and narrows further by `hint_type` / `hint_column` when provided.
/// Exits with an error when the result is ambiguous (multiple matches) or no index exists.
pub fn infer_for_search(
    workspace_id: &str,
    connection_name: &str,
    schema: &str,
    table: &str,
    hint_type: Option<&str>,
    hint_column: Option<&str>,
) -> (String, String) {
    use crossterm::style::Stylize;

    let api = Api::new(Some(workspace_id));

    // Resolve connection name → ID (falls back to managed database catalog lookup)
    let connection_id = crate::commands::connections::resolve_connection_id(&api, connection_name);

    // Fetch indexes for this table
    let indexes = list_one_table(&api, &connection_id, schema, table).unwrap_or_else(|e| e.exit());

    let location = format!("{}.{}.{}", connection_name, schema, table);
    match resolve_search_params(&indexes, hint_type, hint_column, &location) {
        Ok(result) => result,
        Err(msg) => {
            eprintln!("{}", msg.red());
            std::process::exit(1);
        }
    }
}

pub fn list(
    workspace_id: &str,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    format: &str,
) {
    let api = Api::new(Some(workspace_id));

    // One spinner over the whole fetch — the unscoped path is a
    // whole-workspace scan (many requests) that otherwise sits silent.
    // The database discovery inside is deliberately spinner-less
    // (databases::list_database_ids) so nothing fights for the line.
    let spinner = crate::util::spinner("Loading indexes…");
    let result = match (connection_id, schema, table) {
        (Some(cid), Some(sch), Some(tbl)) => list_one_table(&api, cid, sch, tbl).map(|indexes| {
            let rows: Vec<IndexRow> = indexes
                .into_iter()
                .map(|i| IndexRow {
                    inner: i,
                    table: None,
                })
                .collect();
            (rows, false)
        }),
        _ => collect_connection_wide(&api, connection_id, schema, table).map(|rows| (rows, true)),
    };
    spinner.finish_and_clear();
    let (rows, multi_table) = result.unwrap_or_else(|e| e.exit());

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&rows).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&rows).unwrap()),
        "table" => {
            if rows.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No indexes found.".dark_grey());
            } else if multi_table {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r.table.clone().unwrap_or_default(),
                            r.inner.index_name.clone(),
                            r.inner.index_type.clone(),
                            r.inner.columns.join(", "),
                            r.inner.metric.clone().unwrap_or_default(),
                            r.inner.status.clone(),
                            crate::util::format_date(&r.inner.created_at),
                        ]
                    })
                    .collect();
                crate::output::table::print(
                    &[
                        "TABLE", "NAME", "TYPE", "COLUMNS", "METRIC", "STATUS", "CREATED",
                    ],
                    &table_rows,
                );
            } else {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r.inner.index_name.clone(),
                            r.inner.index_type.clone(),
                            r.inner.columns.join(", "),
                            r.inner.metric.clone().unwrap_or_default(),
                            r.inner.status.clone(),
                            crate::util::format_date(&r.inner.created_at),
                        ]
                    })
                    .collect();
                crate::output::table::print(
                    &["NAME", "TYPE", "COLUMNS", "METRIC", "STATUS", "CREATED"],
                    &table_rows,
                );
            }
        }
        _ => unreachable!(),
    }
}

/// Where an index is being created or deleted.
pub enum IndexScope<'a> {
    Connection {
        connection_id: &'a str,
        schema: &'a str,
        table: &'a str,
    },
}

impl IndexScope<'_> {
    fn create_path(&self) -> String {
        match self {
            IndexScope::Connection {
                connection_id,
                schema,
                table,
            } => format!("/connections/{connection_id}/tables/{schema}/{table}/indexes"),
        }
    }

    // Retained for path-shape regression tests; delete now routes through the
    // SDK `indexes()` handle by scope variant rather than a formatted path.
    #[cfg_attr(not(test), allow(dead_code))]
    fn delete_path(&self, index_name: &str) -> String {
        match self {
            IndexScope::Connection {
                connection_id,
                schema,
                table,
            } => {
                format!("/connections/{connection_id}/tables/{schema}/{table}/indexes/{index_name}")
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    workspace_id: &str,
    scope: IndexScope<'_>,
    name: &str,
    columns: &str,
    index_type: &str,
    metric: Option<&str>,
    async_mode: bool,
    embedding_provider_id: Option<&str>,
    dimensions: Option<u32>,
    output_column: Option<&str>,
    description: Option<&str>,
) {
    use crossterm::style::Stylize;

    let cols: Vec<&str> = columns.split(',').map(str::trim).collect();

    let auto_embed_set = embedding_provider_id.is_some()
        || dimensions.is_some()
        || output_column.is_some()
        || description.is_some();
    if auto_embed_set && index_type != "vector" {
        eprintln!(
            "{}",
            "--embedding-provider-id, --dimensions, --output-column, and --description are only valid with --type vector".red()
        );
        std::process::exit(1);
    }
    if index_type == "vector" && cols.len() != 1 {
        eprintln!(
            "{}",
            "--type vector requires exactly one column in --columns".red()
        );
        std::process::exit(1);
    }

    let api = Api::new(Some(workspace_id));

    let mut body = serde_json::json!({
        "index_name": name,
        "columns": cols,
        "index_type": index_type,
        "async": async_mode,
    });
    if let Some(m) = metric {
        body["metric"] = serde_json::json!(m);
    }
    if let Some(p) = embedding_provider_id {
        body["embedding_provider_id"] = serde_json::json!(p);
    }
    if let Some(d) = dimensions {
        body["dimensions"] = serde_json::json!(d);
    }
    if let Some(o) = output_column {
        body["output_column"] = serde_json::json!(o);
    }
    if let Some(d) = description {
        body["description"] = serde_json::json!(d);
    }

    // POST stays on the seam's raw helper: the SDK's `create_index` deserializes
    // into `IndexInfoResponse`, which has no job `id` field, so the async-mode
    // `job_id` output below could not be recovered from the typed model.
    let (status, resp_body) = api
        .post_raw(&scope.create_path(), &body)
        .unwrap_or_else(|e| e.exit());

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    if async_mode {
        let parsed: serde_json::Value = serde_json::from_str(&resp_body).unwrap_or_default();
        let job_id = parsed["id"].as_str().unwrap_or("unknown");
        println!("{}", "Index creation submitted.".green());
        println!("job_id: {}", job_id);
        println!(
            "{}",
            format!("Use 'hotdata jobs {}' to check status.", job_id).dark_grey()
        );
    } else {
        println!("{}", "Index created.".green());
    }
}

pub fn delete(workspace_id: &str, scope: IndexScope<'_>, index_name: &str) {
    use crossterm::style::Stylize;

    let api = Api::new(Some(workspace_id));
    let result = match scope {
        IndexScope::Connection {
            connection_id,
            schema,
            table,
        } => block_with_wakeup(
            &api,
            "Deleting index…",
            api.client()
                .indexes()
                .delete_index(connection_id, schema, table, index_name),
        ),
    };

    if let Err(e) = result {
        let body = match e {
            crate::client::sdk::ApiError::Status { body, .. } => body,
            crate::client::sdk::ApiError::Transport(msg) => msg,
        };
        eprintln!("{}", crate::util::api_error(body).red());
        std::process::exit(1);
    }

    println!("{}", format!("Index '{}' deleted.", index_name).green());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn information_schema_followup_terminates_when_not_has_more() {
        assert!(matches!(
            information_schema_followup(false, Some("c".into())),
            ControlFlow::Break(())
        ));
    }

    #[test]
    fn index_scope_connection_paths() {
        let scope = IndexScope::Connection {
            connection_id: "conn1",
            schema: "public",
            table: "users",
        };
        assert_eq!(
            scope.create_path(),
            "/connections/conn1/tables/public/users/indexes"
        );
        assert_eq!(
            scope.delete_path("idx_email"),
            "/connections/conn1/tables/public/users/indexes/idx_email"
        );
    }

    #[test]
    fn information_schema_followup_breaks_when_more_but_no_cursor() {
        assert!(matches!(
            information_schema_followup(true, None),
            ControlFlow::Break(())
        ));
    }

    #[test]
    fn information_schema_followup_continues_with_cursor() {
        assert!(matches!(
            information_schema_followup(true, Some("next".into())),
            ControlFlow::Continue(ref s) if s == "next"
        ));
    }

    #[test]
    fn sort_info_tables_orders_by_connection_schema_table() {
        let mut tables = vec![
            InfoTable {
                connection: "b".into(),
                schema: "s".into(),
                table: "t2".into(),
            },
            InfoTable {
                connection: "a".into(),
                schema: "s".into(),
                table: "t1".into(),
            },
        ];
        sort_info_tables(&mut tables);
        assert_eq!(tables[0].table, "t1");
        assert_eq!(tables[1].table, "t2");
    }

    #[test]
    fn connection_label_to_id_map_maps_names_only() {
        let connections = vec![
            ConnectionRef {
                id: "conn-id".into(),
                name: "Warehouse".into(),
            },
            ConnectionRef {
                id: "other".into(),
                name: "Lake".into(),
            },
        ];
        let m = connection_label_to_id_map(&connections);
        assert_eq!(m.get("Warehouse").map(String::as_str), Some("conn-id"));
        assert_eq!(m.get("Lake").map(String::as_str), Some("other"));
        assert!(!m.contains_key("conn-id"));
    }

    #[test]
    fn scan_connection_id_prefers_supplied_id_over_label_map() {
        // #161: a managed database's catalog surfaces under an internal
        // `__db_*` label that `connections list` hides, so the name→id map is
        // empty for it. The supplied --connection-id must win regardless.
        let empty = HashMap::new();
        assert_eq!(
            scan_connection_id(Some("conn-real"), "__db_jz50abc", &empty),
            "conn-real"
        );
        // Even when the label *is* in the map, the supplied id takes precedence.
        let mut m = HashMap::new();
        m.insert("__db_jz50abc".to_string(), "conn-mapped".to_string());
        assert_eq!(
            scan_connection_id(Some("conn-real"), "__db_jz50abc", &m),
            "conn-real"
        );
    }

    #[test]
    fn scan_connection_id_maps_label_when_no_supplied_id() {
        let mut m = HashMap::new();
        m.insert("Warehouse".to_string(), "conn-id".to_string());
        assert_eq!(scan_connection_id(None, "Warehouse", &m), "conn-id");
    }

    #[test]
    fn scan_connection_id_falls_back_to_label_when_unmapped() {
        let empty = HashMap::new();
        assert_eq!(scan_connection_id(None, "Warehouse", &empty), "Warehouse");
    }

    #[test]
    fn collect_connection_wide_uses_supplied_id_for_db_scoped_label() {
        // #161 regression: information_schema reports a managed database's
        // catalog under an internal `__db_*` label, but the per-table index
        // call must use the supplied --connection-id. The indexes endpoint is
        // mocked ONLY for the real id (`conn-real`); had the scan used the
        // `__db_*` label (the old behavior), it would miss this mock. No
        // `connections list` mock is needed — a supplied id skips that lookup.
        let mut server = mockito::Server::new();
        let info = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":100,"tables":[
                {"connection":"__db_abc","schema":"public","table":"vec_mid","synced":true}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let idx = server
            .mock(
                "GET",
                "/v1/connections/conn-real/tables/public/vec_mid/indexes",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{"index_name":"vec_mid_idx","index_type":"vector",
                "columns":["c"],"metric":"cosine","status":"ready",
                "created_at":"2020-01-01T00:00:00Z","updated_at":"2020-01-01T00:00:00Z"}]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let rows = collect_connection_wide(&api, Some("conn-real"), None, None).unwrap();
        info.assert();
        idx.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].inner.index_name, "vec_mid_idx");
        assert_eq!(rows[0].table.as_deref(), Some("__db_abc.public.vec_mid"));
    }

    #[test]
    fn collect_connection_wide_unscoped_discovers_managed_db_indexes() {
        // #168: unscoped `indexes list` in a managed-only workspace (the real
        // production shape — `connections list` is empty because it hides
        // database-scoped connections, and the workspace-wide
        // `information_schema` returns no managed tables). The scan must
        // rediscover the managed database via `databases list` → `databases get`
        // → default_connection_id, then a connection-scoped `information_schema`
        // surfaces its `__db_*` table and the per-table indexes call resolves.
        let mut server = mockito::Server::new();
        // No regular connections.
        let conns = server
            .mock("GET", "/v1/connections")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"connections":[]}"#)
            .create();
        // Workspace-wide enumeration (no connection_id query) → no tables.
        let info_ws = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::Exact(String::new()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"count":0,"limit":100,"tables":[],"has_more":false,"next_cursor":null}"#)
            .create();
        // The managed database is discovered here.
        let dbs = server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"dbidabc","name":"airbnb","default_catalog":"default","default_schema":"main"}]}"#,
            )
            .create();
        let db = server
            .mock("GET", "/v1/databases/dbidabc")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"dbidabc","name":"airbnb","default_catalog":"default","default_schema":"main",
                "default_connection_id":"conn-managed","attachments":[]}"#,
            )
            .create();
        // Connection-scoped enumeration surfaces the managed table.
        let info_scoped = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::UrlEncoded(
                "connection_id".into(),
                "conn-managed".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":100,"tables":[
                {"connection":"__db_abc","schema":"public","table":"listings","synced":true}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let idx = server
            .mock(
                "GET",
                "/v1/connections/conn-managed/tables/public/listings/indexes",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{"index_name":"listings_desc_bm25","index_type":"bm25",
                "columns":["description"],"metric":null,"status":"ready",
                "created_at":"2020-01-01T00:00:00Z","updated_at":"2020-01-01T00:00:00Z"}]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let rows = collect_connection_wide(&api, None, None, None).unwrap();
        conns.assert();
        info_ws.assert();
        dbs.assert();
        db.assert();
        info_scoped.assert();
        idx.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].inner.index_name, "listings_desc_bm25");
        assert_eq!(rows[0].table.as_deref(), Some("__db_abc.public.listings"));
    }

    #[test]
    fn collect_connection_wide_unscoped_unions_regular_and_managed() {
        // The unscoped scan unions regular-connection tables (workspace-wide
        // enumeration, label = connection name mapped to its id) with managed
        // databases (discovered separately, #168). The two sets are disjoint, so
        // both indexes appear exactly once.
        let mut server = mockito::Server::new();
        let conns = server
            .mock("GET", "/v1/connections")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"connections":[{"id":"conn-reg","name":"Warehouse","source_type":"postgres"}]}"#,
            )
            .create();
        // Workspace-wide enumeration returns the regular connection's table.
        let info_ws = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::Exact(String::new()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":100,"tables":[
                {"connection":"Warehouse","schema":"public","table":"events","synced":true}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let reg_idx = server
            .mock(
                "GET",
                "/v1/connections/conn-reg/tables/public/events/indexes",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{"index_name":"events_bm25","index_type":"bm25",
                "columns":["body"],"metric":null,"status":"ready",
                "created_at":"2020-01-01T00:00:00Z","updated_at":"2020-01-01T00:00:00Z"}]}"#,
            )
            .create();
        let dbs = server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"dbidabc","name":"airbnb","default_catalog":"default","default_schema":"main"}]}"#,
            )
            .create();
        let db = server
            .mock("GET", "/v1/databases/dbidabc")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"dbidabc","name":"airbnb","default_catalog":"default","default_schema":"main",
                "default_connection_id":"conn-managed","attachments":[]}"#,
            )
            .create();
        let info_scoped = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::UrlEncoded(
                "connection_id".into(),
                "conn-managed".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":100,"tables":[
                {"connection":"__db_abc","schema":"public","table":"listings","synced":true}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let managed_idx = server
            .mock(
                "GET",
                "/v1/connections/conn-managed/tables/public/listings/indexes",
            )
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{"index_name":"listings_desc_bm25","index_type":"bm25",
                "columns":["description"],"metric":null,"status":"ready",
                "created_at":"2020-01-01T00:00:00Z","updated_at":"2020-01-01T00:00:00Z"}]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let mut rows = collect_connection_wide(&api, None, None, None).unwrap();
        conns.assert();
        info_ws.assert();
        reg_idx.assert();
        dbs.assert();
        db.assert();
        info_scoped.assert();
        managed_idx.assert();
        rows.sort_by(|a, b| a.inner.index_name.cmp(&b.inner.index_name));
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].inner.index_name, "events_bm25");
        assert_eq!(rows[0].table.as_deref(), Some("Warehouse.public.events"));
        assert_eq!(rows[1].inner.index_name, "listings_desc_bm25");
        assert_eq!(rows[1].table.as_deref(), Some("__db_abc.public.listings"));
    }

    #[test]
    fn collect_tables_single_page() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/information_schema")
            .match_header("Authorization", "Bearer k")
            .match_header("X-Workspace-Id", "ws1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":2,"limit":100,"tables":[
                {"connection":"c1","schema":"public","table":"z","synced":true},
                {"connection":"c1","schema":"public","table":"a","synced":true}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws1"));
        let tables = collect_tables(&api, None, None, None).unwrap();
        mock.assert();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].table, "a");
        assert_eq!(tables[1].table, "z");
    }

    #[test]
    fn list_one_table_scan_returns_empty_on_404() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock(
                "GET",
                mockito::Matcher::Regex(r"^/v1/connections/.+/tables/.+/.+/indexes$".into()),
            )
            .match_header("Authorization", "Bearer k")
            .with_status(404)
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let rows = list_one_table_scan(&api, "cid", "sch", "tbl").unwrap();
        mock.assert();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_one_table_returns_indexes() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/connections/cid/tables/sch/tbl/indexes")
            .match_header("Authorization", "Bearer k")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{
                "index_name":"ix1",
                "index_type":"btree",
                "columns":["c1"],
                "metric":null,
                "status":"ready",
                "created_at":"2020-01-01T00:00:00Z",
                "updated_at":"2020-01-01T00:00:00Z"
            }]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", None);
        let rows = list_one_table(&api, "cid", "sch", "tbl").unwrap();
        mock.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index_name, "ix1");
    }

    #[test]
    fn list_one_table_keeps_non_enum_status_via_untyped_parse() {
        // Regression: the SDK's typed `IndexStatus` only models `ready`/`pending`.
        // The CLI's untyped `get_json` path must still accept any status string so
        // the list display never breaks on a backend status the SDK can't model.
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/connections/cid/tables/sch/tbl/indexes")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"indexes":[{
                "index_name":"ix1",
                "index_type":"bm25",
                "columns":["c1"],
                "metric":null,
                "status":"building",
                "created_at":"2020-01-01T00:00:00Z",
                "updated_at":"2020-01-01T00:00:00Z"
            }]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", None);
        let rows = list_one_table(&api, "cid", "sch", "tbl").unwrap();
        mock.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "building");
    }

    #[test]
    fn list_one_table_scan_returns_indexes_on_200() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/connections/x/tables/s/t/indexes")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"indexes":[]}"#)
            .create();

        let api = Api::test_new(&server.url(), "k", None);
        let rows = list_one_table_scan(&api, "x", "s", "t").unwrap();
        mock.assert();
        assert!(rows.is_empty());
    }

    fn make_index(name: &str, index_type: &str, columns: &[&str]) -> Index {
        Index {
            index_name: name.into(),
            index_type: index_type.into(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            metric: None,
            source_column: None,
            status: "ready".into(),
            created_at: "2020-01-01T00:00:00Z".into(),
            updated_at: "2020-01-01T00:00:00Z".into(),
        }
    }

    /// An auto-embed vector index: `columns` holds the generated embedding
    /// column, while the source text column lives in `source_column`.
    fn make_embedding_index(name: &str, source: &str, output: &str) -> Index {
        Index {
            source_column: Some(source.into()),
            ..make_index(name, "vector", &[output])
        }
    }

    #[test]
    fn resolve_search_params_single_bm25_returns_type_and_column() {
        let indexes = vec![make_index("fts", "bm25", &["description"])];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert_eq!(result, Ok(("bm25".into(), "description".into())));
    }

    #[test]
    fn resolve_search_params_single_vector_returns_type_and_column() {
        let indexes = vec![make_index("vec", "vector", &["embedding"])];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert_eq!(result, Ok(("vector".into(), "embedding".into())));
    }

    #[test]
    fn resolve_search_params_embedding_index_returns_source_column() {
        // #162: auto-embed index — inference must yield the source column
        // (`content`), not the generated embedding column (`content_embedding`),
        // since the server's vector_distance rewrite matches the source column.
        let indexes = vec![make_embedding_index("vec", "content", "content_embedding")];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert_eq!(result, Ok(("vector".into(), "content".into())));
    }

    #[test]
    fn resolve_search_params_deserializes_real_server_response() {
        // #162: guard the serde contract end-to-end. This is a verbatim
        // `GET .../indexes` body from a runtimedb auto-embed index (note the
        // generated `content_embedding` in `columns` and the `source_column`
        // field). Inference must parse it and resolve the source column.
        let body = r#"{"indexes":[{"index_name":"embed_small_idx","index_type":"vector","columns":["content_embedding"],"status":"ready","updated_at":"2026-06-18T07:33:19.656Z","created_at":"2026-06-18T07:33:19.669550Z","metric":"cosine","source_column":"content"}]}"#;
        let parsed: ListResponse = serde_json::from_str(body).expect("parse index list");
        let result = resolve_search_params(&parsed.indexes, None, None, "vtest.public.embed_small");
        assert_eq!(result, Ok(("vector".into(), "content".into())));
    }

    #[test]
    fn resolve_search_params_embedding_index_hint_type_only() {
        // #162: `--type vector` with `--column` omitted must still resolve the
        // source column rather than the embedding output column.
        let indexes = vec![make_embedding_index("vec", "content", "content_embedding")];
        let result = resolve_search_params(&indexes, Some("vector"), None, "db.public.t");
        assert_eq!(result, Ok(("vector".into(), "content".into())));
    }

    #[test]
    fn resolve_search_params_embedding_index_hint_source_column() {
        // #162: `--column content` (the source column) with `--type` omitted
        // must match the auto-embed index even though `content` is not in
        // `columns` (which holds `content_embedding`).
        let indexes = vec![make_embedding_index("vec", "content", "content_embedding")];
        let result = resolve_search_params(&indexes, None, Some("content"), "db.public.t");
        assert_eq!(result, Ok(("vector".into(), "content".into())));
    }

    #[test]
    fn resolve_search_params_non_search_indexes_ignored() {
        let indexes = vec![
            make_index("sorted_idx", "sorted", &["created_at"]),
            make_index("fts", "bm25", &["body"]),
        ];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert_eq!(result, Ok(("bm25".into(), "body".into())));
    }

    #[test]
    fn resolve_search_params_hint_type_narrows_to_single() {
        let indexes = vec![
            make_index("fts", "bm25", &["description"]),
            make_index("vec", "vector", &["embedding"]),
        ];
        let result = resolve_search_params(&indexes, Some("bm25"), None, "db.public.t");
        assert_eq!(result, Ok(("bm25".into(), "description".into())));
    }

    #[test]
    fn resolve_search_params_hint_column_narrows_to_single() {
        let indexes = vec![
            make_index("fts_desc", "bm25", &["description"]),
            make_index("fts_name", "bm25", &["name"]),
        ];
        let result = resolve_search_params(&indexes, None, Some("name"), "db.public.t");
        assert_eq!(result, Ok(("bm25".into(), "name".into())));
    }

    #[test]
    fn resolve_search_params_no_search_indexes_returns_error() {
        let indexes = vec![make_index("sorted_idx", "sorted", &["id"])];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("No BM25 or vector index found")
        );
    }

    #[test]
    fn resolve_search_params_no_index_error_mentions_hint_type() {
        let indexes = vec![make_index("fts", "bm25", &["description"])];
        let result = resolve_search_params(&indexes, Some("vector"), None, "db.public.t");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("vector index"));
    }

    #[test]
    fn resolve_search_params_multiple_matches_returns_error() {
        let indexes = vec![
            make_index("fts_desc", "bm25", &["description"]),
            make_index("fts_name", "bm25", &["name"]),
        ];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .contains("Multiple search indexes found")
        );
    }

    #[test]
    fn resolve_search_params_index_with_no_columns_returns_error() {
        let indexes = vec![make_index("fts", "bm25", &[])];
        let result = resolve_search_params(&indexes, None, None, "db.public.t");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("has no columns"));
    }

    mod list_args {
        use crate::commands::indexes::IndexesCommands;
        use clap::Parser;

        #[derive(Parser)]
        struct Wrapper {
            #[command(subcommand)]
            cmd: IndexesCommands,
        }

        fn parse(args: &[&str]) -> Result<IndexesCommands, clap::Error> {
            Wrapper::try_parse_from(std::iter::once("t").chain(args.iter().copied())).map(|w| w.cmd)
        }

        #[test]
        fn list_parses_with_no_flags() {
            assert!(matches!(
                parse(&["list"]).unwrap(),
                IndexesCommands::List {
                    schema: None,
                    table: None,
                    ..
                }
            ));
        }

        #[test]
        fn list_rejects_connection_id_flag() {
            assert!(parse(&["list", "--connection-id", "conn1"]).is_err());
        }

        #[test]
        fn list_accepts_schema_and_table_filters() {
            let cmd = parse(&["list", "--schema", "public", "--table", "orders"]).unwrap();
            assert!(matches!(
                cmd,
                IndexesCommands::List { schema, table, .. }
                if schema.as_deref() == Some("public") && table.as_deref() == Some("orders")
            ));
        }
    }

    mod delete_args {
        use crate::commands::indexes::IndexesCommands;
        use clap::Parser;

        #[derive(Parser)]
        struct Wrapper {
            #[command(subcommand)]
            cmd: IndexesCommands,
        }

        fn parse(args: &[&str]) -> Result<IndexesCommands, clap::Error> {
            Wrapper::try_parse_from(std::iter::once("t").chain(args.iter().copied())).map(|w| w.cmd)
        }

        #[test]
        fn delete_catalog_defaults_schema_to_public() {
            let cmd = parse(&[
                "delete",
                "--catalog",
                "vtest",
                "--table",
                "hits",
                "--name",
                "idx",
            ])
            .unwrap();
            match cmd {
                IndexesCommands::Delete {
                    catalog,
                    schema,
                    table,
                    name,
                } => {
                    assert_eq!(catalog, "vtest");
                    assert_eq!(schema, "public"); // defaulted, parity with `create`
                    assert_eq!(table, "hits");
                    assert_eq!(name, "idx");
                }
                _ => panic!("expected Delete"),
            }
        }

        #[test]
        fn delete_requires_catalog() {
            // --catalog is now required
            assert!(parse(&["delete", "--table", "hits", "--name", "idx"]).is_err());
        }

        #[test]
        fn delete_requires_table() {
            assert!(parse(&["delete", "--catalog", "x", "--name", "idx"]).is_err());
        }
    }
}
