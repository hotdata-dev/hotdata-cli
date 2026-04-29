use crate::api::ApiClient;
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
    status: String,
    created_at: String,
    updated_at: String,
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
struct InfoListResponse {
    tables: Vec<InfoTable>,
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
struct ConnectionRef {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct ConnectionsBody {
    connections: Vec<ConnectionRef>,
}

fn connection_label_to_id_map(connections: &[ConnectionRef]) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for c in connections {
        m.insert(c.name.clone(), c.id.clone());
    }
    m
}

fn connection_lookup(api: &ApiClient) -> HashMap<String, String> {
    let body: ConnectionsBody = api.get("/connections");
    connection_label_to_id_map(&body.connections)
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
    api: &ApiClient,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
) -> Vec<InfoTable> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params: Vec<(&str, Option<String>)> = Vec::new();
        if let Some(id) = connection_id {
            params.push(("connection_id", Some(id.to_string())));
        }
        if let Some(s) = schema {
            params.push(("schema", Some(s.to_string())));
        }
        if let Some(t) = table {
            params.push(("table", Some(t.to_string())));
        }
        if let Some(ref c) = cursor {
            params.push(("cursor", Some(c.clone())));
        }
        let body: InfoListResponse = api.get_with_params("/information_schema", &params);
        out.extend(body.tables);
        match information_schema_followup(body.has_more, body.next_cursor) {
            ControlFlow::Break(()) => break,
            ControlFlow::Continue(c) => cursor = Some(c),
        }
    }
    sort_info_tables(&mut out);
    out
}

fn list_one_table(api: &ApiClient, connection_id: &str, schema: &str, table: &str) -> Vec<Index> {
    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    let body: ListResponse = api.get(&path);
    body.indexes
}

fn list_one_dataset(api: &ApiClient, dataset_id: &str) -> Vec<Index> {
    let path = format!("/datasets/{dataset_id}/indexes");
    let body: ListResponse = api.get(&path);
    body.indexes
}

fn list_one_table_scan(
    api: &ApiClient,
    connection_id: &str,
    schema: &str,
    table: &str,
) -> Vec<Index> {
    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    match api.get_none_if_not_found::<ListResponse>(&path) {
        Some(body) => body.indexes,
        None => Vec::new(),
    }
}

pub fn list(
    workspace_id: &str,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    dataset_id: Option<&str>,
    format: &str,
) {
    let api = ApiClient::new(Some(workspace_id));

    let (rows, multi_table) = match (dataset_id, connection_id, schema, table) {
        (Some(did), _, _, _) => {
            let indexes = list_one_dataset(&api, did);
            let rows: Vec<IndexRow> = indexes
                .into_iter()
                .map(|i| IndexRow {
                    inner: i,
                    table: None,
                })
                .collect();
            (rows, false)
        }
        (None, Some(cid), Some(sch), Some(tbl)) => {
            let indexes = list_one_table(&api, cid, sch, tbl);
            let rows: Vec<IndexRow> = indexes
                .into_iter()
                .map(|i| IndexRow {
                    inner: i,
                    table: None,
                })
                .collect();
            (rows, false)
        }
        _ => {
            let tables = collect_tables(&api, connection_id, schema, table);
            let conn_ids = connection_lookup(&api);
            let api = api.clone();
            let per_table: Vec<(String, Vec<Index>)> = tables
                .par_iter()
                .map(|t| {
                    let cid = conn_ids
                        .get(&t.connection)
                        .map(String::as_str)
                        .unwrap_or(t.connection.as_str());
                    let full = format!("{}.{}.{}", t.connection, t.schema, t.table);
                    let indexes = list_one_table_scan(&api, cid, &t.schema, &t.table);
                    (full, indexes)
                })
                .collect();
            let mut rows: Vec<IndexRow> = Vec::new();
            for (full, indexes) in per_table {
                for i in indexes {
                    rows.push(IndexRow {
                        inner: i,
                        table: Some(full.clone()),
                    });
                }
            }
            (rows, true)
        }
    };

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
                crate::table::print(
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
                crate::table::print(
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
    Dataset {
        dataset_id: &'a str,
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
            IndexScope::Dataset { dataset_id } => format!("/datasets/{dataset_id}/indexes"),
        }
    }

    fn delete_path(&self, index_name: &str) -> String {
        match self {
            IndexScope::Connection {
                connection_id,
                schema,
                table,
            } => format!(
                "/connections/{connection_id}/tables/{schema}/{table}/indexes/{index_name}"
            ),
            IndexScope::Dataset { dataset_id } => {
                format!("/datasets/{dataset_id}/indexes/{index_name}")
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

    let api = ApiClient::new(Some(workspace_id));

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

    let (status, resp_body) = api.post_raw(&scope.create_path(), &body);

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

    let api = ApiClient::new(Some(workspace_id));
    let (status, resp_body) = api.delete_raw(&scope.delete_path(index_name));

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
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
    fn collect_tables_single_page() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/information_schema")
            .match_header("Authorization", "Bearer k")
            .match_header("X-Workspace-Id", "ws1")
            .with_status(200)
            .with_body(
                r#"{"tables":[
                {"connection":"c1","schema":"public","table":"z"},
                {"connection":"c1","schema":"public","table":"a"}
            ],"has_more":false,"next_cursor":null}"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws1"));
        let tables = collect_tables(&api, None, None, None);
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
                mockito::Matcher::Regex(r"^/connections/.+/tables/.+/.+/indexes$".into()),
            )
            .match_header("Authorization", "Bearer k")
            .with_status(404)
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws"));
        let rows = list_one_table_scan(&api, "cid", "sch", "tbl");
        mock.assert();
        assert!(rows.is_empty());
    }

    #[test]
    fn list_one_table_returns_indexes() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/connections/cid/tables/sch/tbl/indexes")
            .match_header("Authorization", "Bearer k")
            .with_status(200)
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

        let api = ApiClient::test_new(&server.url(), "k", None);
        let rows = list_one_table(&api, "cid", "sch", "tbl");
        mock.assert();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].index_name, "ix1");
    }

    #[test]
    fn list_one_table_scan_returns_indexes_on_200() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/connections/x/tables/s/t/indexes")
            .with_status(200)
            .with_body(r#"{"indexes":[]}"#)
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let rows = list_one_table_scan(&api, "x", "s", "t");
        mock.assert();
        assert!(rows.is_empty());
    }
}
