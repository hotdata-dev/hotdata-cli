use crate::client::sdk::{Api, ApiError, block, block_with_wakeup, none_if_404};
use crate::commands::databases;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::ControlFlow;

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

/// A search index located by name within a managed database, for `search`'s
/// by-name addressing. Carries the database's real ids (never a `__db_*` label).
pub struct LocatedIndex {
    pub database_id: String,
    pub connection_id: String,
    pub catalog: String,
    pub schema: String,
    pub table: String,
    pub index_type: String,
    pub search_column: String,
    pub status: String,
    pub metric: Option<String>,
}

/// Find a search index by name within a managed database.
///
/// The database is required and addressed by id — an explicit `--database`, or
/// the active one set via `hotdata databases use <id>`. There is no
/// workspace-wide scan and no fallback to the internal `__db_*` connection
/// label: the database's own `default_connection_id` addresses the index API and
/// its `default_catalog` builds search SQL. Errors on no database, no match, or
/// an ambiguous name.
pub fn locate_by_name(
    workspace_id: &str,
    database: Option<&str>,
    name: &str,
) -> Result<LocatedIndex, String> {
    let api = Api::new(Some(workspace_id));
    let db_id = database
        .map(str::to_string)
        .or_else(|| crate::config::load_current_database("default", workspace_id))
        .ok_or_else(|| {
            "no database — pass --database <id> or set one with `hotdata databases use <id>`"
                .to_string()
        })?;
    let db = databases::get_database(&api, &db_id).unwrap_or_else(|e| e.exit());
    let connection_id = db.default_connection_id;
    let catalog = db
        .default_catalog
        .unwrap_or_else(|| db.name.unwrap_or_else(|| "default".to_string()));

    let rows = collect_connection_wide(&api, Some(&connection_id), None, None)
        .unwrap_or_else(|e| e.exit());
    let matches: Vec<&IndexRow> = rows.iter().filter(|r| r.inner.index_name == name).collect();
    match matches.as_slice() {
        [] => Err(format!(
            "No search index named '{name}' in this database — run 'hotdata search list' to see indexes."
        )),
        [one] => {
            let loc = one.table.clone().unwrap_or_default();
            let parts: Vec<&str> = loc.splitn(3, '.').collect();
            let (schema, table) = match parts.as_slice() {
                [_conn, s, t] => (s.to_string(), t.to_string()),
                _ => return Err(format!("Could not resolve the table for index '{name}'.")),
            };
            let search_column = one
                .inner
                .search_column()
                .ok_or_else(|| format!("Index '{name}' has no columns."))?;
            Ok(LocatedIndex {
                database_id: db_id,
                connection_id,
                catalog,
                schema,
                table,
                index_type: one.inner.index_type.clone(),
                search_column,
                status: one.inner.status.clone(),
                metric: one.inner.metric.clone(),
            })
        }
        _ => Err(format!(
            "Multiple indexes named '{name}' on this database — this by-name form needs a unique name."
        )),
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
                {"connection":"__db_abc","schema":"public","table":"vec_mid","synced":true,"partition_by":[],"sorted_by":[]}
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
                {"connection":"__db_abc","schema":"public","table":"listings","synced":true,"partition_by":[],"sorted_by":[]}
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
                {"connection":"Warehouse","schema":"public","table":"events","synced":true,"partition_by":[],"sorted_by":[]}
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
                {"connection":"__db_abc","schema":"public","table":"listings","synced":true,"partition_by":[],"sorted_by":[]}
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
                {"connection":"c1","schema":"public","table":"z","synced":true,"partition_by":[],"sorted_by":[]},
                {"connection":"c1","schema":"public","table":"a","synced":true,"partition_by":[],"sorted_by":[]}
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
}
