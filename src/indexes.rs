use crate::api::ApiClient;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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

fn connection_lookup(api: &ApiClient) -> HashMap<String, String> {
    let body: ConnectionsBody = api.get("/connections");
    let mut m = HashMap::new();
    for c in body.connections {
        m.insert(c.id.clone(), c.id.clone());
        m.insert(c.name.clone(), c.id);
    }
    m
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
        if !body.has_more {
            break;
        }
        cursor = body.next_cursor;
    }
    out.sort_by(|a, b| {
        a.connection
            .cmp(&b.connection)
            .then_with(|| a.schema.cmp(&b.schema))
            .then_with(|| a.table.cmp(&b.table))
    });
    out
}

fn list_one_table(api: &ApiClient, connection_id: &str, schema: &str, table: &str) -> Vec<Index> {
    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    let body: ListResponse = api.get(&path);
    body.indexes
}

fn list_one_table_scan(api: &ApiClient, connection_id: &str, schema: &str, table: &str) -> Vec<Index> {
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
    format: &str,
) {
    let api = ApiClient::new(Some(workspace_id));

    let (rows, multi_table) = match (connection_id, schema, table) {
        (Some(cid), Some(sch), Some(tbl)) => {
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
                        "TABLE",
                        "NAME",
                        "TYPE",
                        "COLUMNS",
                        "METRIC",
                        "STATUS",
                        "CREATED",
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

pub fn create(
    workspace_id: &str,
    connection_id: &str,
    schema: &str,
    table: &str,
    name: &str,
    columns: &str,
    index_type: &str,
    metric: Option<&str>,
    async_mode: bool,
) {
    let api = ApiClient::new(Some(workspace_id));

    let cols: Vec<&str> = columns.split(',').map(str::trim).collect();
    let mut body = serde_json::json!({
        "index_name": name,
        "columns": cols,
        "index_type": index_type,
        "async": async_mode,
    });
    if let Some(m) = metric {
        body["metric"] = serde_json::json!(m);
    }

    let path = format!("/connections/{connection_id}/tables/{schema}/{table}/indexes");
    let (status, resp_body) = api.post_raw(&path, &body);

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    use crossterm::style::Stylize;
    if async_mode {
        let parsed: serde_json::Value = serde_json::from_str(&resp_body).unwrap_or_default();
        let job_id = parsed["job_id"].as_str().unwrap_or("unknown");
        println!("{}", "Index creation submitted.".green());
        println!("job_id: {}", job_id);
        println!("{}", "Use 'hotdata jobs <job_id>' to check status.".dark_grey());
    } else {
        println!("{}", "Index created.".green());
    }
}
