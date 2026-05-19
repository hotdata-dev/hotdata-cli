use crate::api::ApiClient;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::Path;

const MANAGED_SOURCE_TYPE: &str = "managed";
const DEFAULT_SCHEMA: &str = "public";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Database {
    pub id: String,
    pub name: String,
    pub source_type: String,
}

#[derive(Deserialize)]
struct ListConnectionsResponse {
    connections: Vec<Database>,
}

#[derive(Deserialize, Serialize)]
struct DatabaseDetail {
    id: String,
    name: String,
    source_type: String,
    #[serde(default)]
    table_count: u64,
    #[serde(default)]
    synced_table_count: u64,
}

#[derive(Deserialize)]
struct InfoTable {
    #[allow(dead_code)]
    connection: String,
    schema: String,
    table: String,
    synced: bool,
    last_sync: Option<String>,
}

#[derive(Deserialize)]
struct InfoListResponse {
    tables: Vec<InfoTable>,
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct TableRow {
    full_name: String,
    schema: String,
    table: String,
    synced: bool,
    last_sync: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct CreateConnectionResponse {
    id: String,
    name: String,
    source_type: String,
}

#[derive(Deserialize)]
struct LoadManagedTableResponse {
    #[allow(dead_code)]
    connection_id: String,
    schema_name: String,
    table_name: String,
    row_count: u64,
    #[allow(dead_code)]
    arrow_schema_json: String,
}

fn is_managed(db: &Database) -> bool {
    db.source_type == MANAGED_SOURCE_TYPE
}

fn fail_not_managed(name: &str, source_type: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!(
        "{}",
        format!(
            "error: '{name}' is not a managed database (source_type: {source_type}). \
             Use `hotdata connections` for remote sources."
        )
        .red()
    );
    std::process::exit(1);
}

pub fn try_resolve_database(api: &ApiClient, name_or_id: &str) -> Result<Database, String> {
    let body: ListConnectionsResponse = api.get("/connections");
    let by_id = body
        .connections
        .iter()
        .find(|c| c.id == name_or_id)
        .cloned();
    let found = by_id.or_else(|| {
        body.connections
            .iter()
            .find(|c| c.name == name_or_id)
            .cloned()
    });
    match found {
        Some(db) if is_managed(&db) => Ok(db),
        Some(db) => Err(format!(
            "'{}' is not a managed database (source_type: {})",
            db.name, db.source_type
        )),
        None => Err(format!("no database named or with id '{name_or_id}'")),
    }
}

pub fn resolve_database(api: &ApiClient, name_or_id: &str) -> Database {
    match try_resolve_database(api, name_or_id) {
        Ok(db) => db,
        Err(e) => {
            use crossterm::style::Stylize;
            if e.contains("not a managed database") {
                eprintln!("{}", format!("error: {e}. Use `hotdata connections` for remote sources.").red());
            } else {
                eprintln!("{}", format!("error: {e}").red());
            }
            std::process::exit(1);
        }
    }
}

fn schema_name(schema: Option<&str>) -> &str {
    schema.unwrap_or(DEFAULT_SCHEMA)
}

/// Build managed-connection `config` with declared schemas/tables.
pub fn build_managed_config(schema: &str, tables: &[String]) -> serde_json::Value {
    if tables.is_empty() {
        return serde_json::json!({});
    }
    let table_objs: Vec<serde_json::Value> = tables
        .iter()
        .map(|t| serde_json::json!({ "name": t }))
        .collect();
    serde_json::json!({
        "schemas": [{ "name": schema, "tables": table_objs }]
    })
}

/// Request body for `POST /v1/connections` when creating a managed database.
pub fn create_connection_request(name: &str, schema: &str, tables: &[String]) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "source_type": MANAGED_SOURCE_TYPE,
        "config": build_managed_config(schema, tables),
        "skip_discovery": true,
    })
}

pub fn managed_table_load_path(connection_id: &str, schema: &str, table: &str) -> String {
    format!("/connections/{connection_id}/schemas/{schema}/tables/{table}/loads")
}

pub fn managed_table_delete_path(connection_id: &str, schema: &str, table: &str) -> String {
    format!("/connections/{connection_id}/schemas/{schema}/tables/{table}")
}

pub fn load_table_request(upload_id: &str) -> serde_json::Value {
    serde_json::json!({
        "mode": "replace",
        "upload_id": upload_id,
    })
}

/// Returns true when `path` looks like a parquet file by extension.
pub fn is_parquet_path(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".parquet")
        || Path::new(path).extension().and_then(|e| e.to_str()) == Some("parquet")
}

fn table_rows_for_database(db_name: &str, tables: Vec<InfoTable>) -> Vec<TableRow> {
    tables
        .into_iter()
        .map(|t| TableRow {
            full_name: format!("{}.{}.{}", db_name, t.schema, t.table),
            schema: t.schema,
            table: t.table,
            synced: t.synced,
            last_sync: t.last_sync,
        })
        .collect()
}

fn upload_parquet_file(api: &ApiClient, path: &str) -> String {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error opening file '{path}': {e}");
            std::process::exit(1);
        }
    };

    if !is_parquet_path(path) {
        eprintln!(
            "error: managed table loads require a parquet file (got '{}'). \
             Convert your data to parquet or use `hotdata datasets create` for CSV/JSON.",
            path
        );
        std::process::exit(1);
    }

    let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let pb = ProgressBar::new(file_size);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    let reader = pb.wrap_read(f);

    let (status, resp_body) = api.post_body(
        "/files",
        "application/octet-stream",
        reader,
        Some(file_size),
    );
    pb.finish_and_clear();

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let body: serde_json::Value = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing upload response: {e}");
            std::process::exit(1);
        }
    };
    match body["id"].as_str() {
        Some(id) => id.to_string(),
        None => {
            eprintln!("error: upload response missing id");
            std::process::exit(1);
        }
    }
}

fn collect_tables(api: &ApiClient, connection_id: &str, schema: Option<&str>) -> Vec<InfoTable> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params: Vec<(&str, Option<String>)> = vec![
            ("connection_id", Some(connection_id.to_string())),
        ];
        if let Some(s) = schema {
            params.push(("schema", Some(s.to_string())));
        }
        if let Some(ref c) = cursor {
            params.push(("cursor", Some(c.clone())));
        }
        let body: InfoListResponse = api.get_with_params("/information_schema", &params);
        out.extend(body.tables);
        if !body.has_more {
            break;
        }
        let Some(c) = body.next_cursor else {
            break;
        };
        cursor = Some(c);
    }
    out.sort_by(|a, b| {
        a.schema
            .cmp(&b.schema)
            .then_with(|| a.table.cmp(&b.table))
    });
    out
}

pub fn list(workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListConnectionsResponse = api.get("/connections");
    let databases: Vec<&Database> = body
        .connections
        .iter()
        .filter(|c| is_managed(c))
        .collect();

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&databases).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&databases).unwrap()),
        "table" => {
            if databases.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No databases found.".dark_grey());
                eprintln!(
                    "{}",
                    "Create one with: hotdata databases create --name <name>".dark_grey()
                );
            } else {
                let rows: Vec<Vec<String>> = databases
                    .iter()
                    .map(|d| vec![d.name.clone(), d.id.clone()])
                    .collect();
                crate::table::print(&["NAME", "ID"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(workspace_id: &str, name_or_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, name_or_id);
    let detail: DatabaseDetail = api.get(&format!("/connections/{}", db.id));

    if detail.source_type != MANAGED_SOURCE_TYPE {
        fail_not_managed(&detail.name, &detail.source_type);
    }

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&detail).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&detail).unwrap()),
        "table" => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<16}", l).dark_grey().to_string();
            println!("{}{}", label("name:"), detail.name.clone().white());
            println!("{}{}", label("id:"), detail.id.dark_cyan());
            println!(
                "{}{} synced / {} total",
                label("tables:"),
                detail.synced_table_count.to_string().cyan(),
                detail.table_count.to_string().cyan(),
            );
            println!(
                "{}{}",
                label("sql_prefix:"),
                format!("{}.{{schema}}.{{table}}", detail.name).green()
            );
        }
        _ => unreachable!(),
    }
}

pub fn create(workspace_id: &str, name: &str, schema: &str, tables: &[String], format: &str) {
    use crossterm::style::Stylize;

    let body = create_connection_request(name, schema, tables);

    let api = ApiClient::new(Some(workspace_id));
    let spinner = (format == "table").then(|| crate::util::spinner("Creating database..."));
    let (status, resp_body) = api.post_raw("/connections", &body);
    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let result: CreateConnectionResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&result).unwrap()),
        "table" => {
            println!("{}", "Database created".green());
            println!("name: {}", result.name);
            println!("id:   {}", result.id);
            println!(
                "load: hotdata databases tables load {} <table> --file ./data.parquet",
                result.name
            );
        }
        _ => unreachable!(),
    }
}

pub fn delete(workspace_id: &str, name_or_id: &str) {
    use crossterm::style::Stylize;

    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, name_or_id);
    let (status, resp_body) = api.delete_raw(&format!("/connections/{}", db.id));

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    println!(
        "{}",
        format!("Database '{}' deleted.", db.name).green()
    );
}

pub fn tables_list(
    workspace_id: &str,
    database: &str,
    schema: Option<&str>,
    format: &str,
) {
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, database);
    let tables = collect_tables(&api, &db.id, schema);

    let rows = table_rows_for_database(&db.name, tables);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&rows).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&rows).unwrap()),
        "table" => {
            if rows.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No tables found.".dark_grey());
            } else {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|r| {
                        vec![
                            r.full_name.clone(),
                            r.synced.to_string(),
                            r.last_sync
                                .as_deref()
                                .map(crate::util::format_date)
                                .unwrap_or_else(|| "-".to_string()),
                        ]
                    })
                    .collect();
                crate::table::print(&["TABLE", "SYNCED", "LAST_SYNC"], &table_rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn tables_load(
    workspace_id: &str,
    database: &str,
    table: &str,
    schema: Option<&str>,
    file: Option<&str>,
    upload_id: Option<&str>,
) {
    use crossterm::style::Stylize;

    if file.is_some() && upload_id.is_some() {
        eprintln!("error: pass either --file or --upload-id, not both");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, database);
    let schema = schema_name(schema);

    let upload_id = match (upload_id, file) {
        (Some(id), None) => id.to_string(),
        (None, Some(path)) => upload_parquet_file(&api, path),
        (None, None) => {
            eprintln!("error: --file <path> or --upload-id <id> is required");
            std::process::exit(1);
        }
        (Some(_), Some(_)) => unreachable!(),
    };

    let path = managed_table_load_path(&db.id, schema, table);
    let body = load_table_request(&upload_id);

    let spinner = crate::util::spinner("Loading table...");
    let (status, resp_body) = api.post_raw(&path, &body);
    spinner.finish_and_clear();

    if !status.is_success() {
        let msg = crate::util::api_error(resp_body);
        if msg.contains("not declared") {
            eprintln!("{}", msg.red());
            eprintln!(
                "{}",
                format!(
                    "Declare the table when creating the database, e.g.:\n  \
                     hotdata databases create --name {} --table {}",
                    db.name, table
                )
                .dark_grey()
            );
        } else {
            eprintln!("{}", msg.red());
        }
        std::process::exit(1);
    }

    let result: LoadManagedTableResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    let full_name = format!("{}.{}.{}", db.name, result.schema_name, result.table_name);
    println!("{}", "Table loaded".green());
    println!("full_name: {}", full_name.green());
    println!("rows:      {}", result.row_count);
}

pub fn tables_delete(
    workspace_id: &str,
    database: &str,
    table: &str,
    schema: Option<&str>,
) {
    use crossterm::style::Stylize;

    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, database);
    let schema = schema_name(schema);

    let path = managed_table_delete_path(&db.id, schema, table);
    let (status, resp_body) = api.delete_raw(&path);

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    println!(
        "{}",
        format!("Table '{}.{}.{}' deleted.", db.name, schema, table).green()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_name_defaults_to_public() {
        assert_eq!(schema_name(None), "public");
        assert_eq!(schema_name(Some("custom")), "custom");
    }

    #[test]
    fn build_managed_config_empty_without_tables() {
        assert_eq!(build_managed_config("public", &[]), serde_json::json!({}));
    }

    #[test]
    fn build_managed_config_declares_tables() {
        let cfg = build_managed_config("public", &["orders".to_string(), "customers".to_string()]);
        assert_eq!(
            cfg,
            serde_json::json!({
                "schemas": [{
                    "name": "public",
                    "tables": [{ "name": "orders" }, { "name": "customers" }]
                }]
            })
        );
    }

    #[test]
    fn is_managed_only_matches_managed_type() {
        let db = Database {
            id: "c1".into(),
            name: "sales".into(),
            source_type: "managed".into(),
        };
        assert!(is_managed(&db));
        let pg = Database {
            id: "c2".into(),
            name: "warehouse".into(),
            source_type: "postgres".into(),
        };
        assert!(!is_managed(&pg));
    }

    #[test]
    fn resolve_database_by_name_and_id() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/connections")
            .with_status(200)
            .with_body(
                r#"{"connections":[
                {"id":"conn_abc","name":"sales","source_type":"managed"},
                {"id":"conn_xyz","name":"warehouse","source_type":"postgres"}
            ]}"#,
            )
            .expect(2)
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws"));
        let by_name = resolve_database(&api, "sales");
        assert_eq!(by_name.id, "conn_abc");
        let by_id = resolve_database(&api, "conn_abc");
        assert_eq!(by_id.name, "sales");
        mock.assert();
    }

    #[test]
    fn try_resolve_database_rejects_non_managed() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/connections")
            .with_status(200)
            .with_body(
                r#"{"connections":[{"id":"c1","name":"warehouse","source_type":"postgres"}]}"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let err = try_resolve_database(&api, "warehouse").unwrap_err();
        assert!(err.contains("not a managed database"));
        mock.assert();
    }

    #[test]
    fn try_resolve_database_not_found() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/connections")
            .with_status(200)
            .with_body(r#"{"connections":[]}"#)
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let err = try_resolve_database(&api, "missing").unwrap_err();
        assert!(err.contains("no database named"));
        mock.assert();
    }

    #[test]
    fn create_connection_request_includes_declared_tables() {
        let body = create_connection_request(
            "sales",
            "public",
            &["orders".to_string(), "customers".to_string()],
        );
        assert_eq!(body["name"], "sales");
        assert_eq!(body["source_type"], "managed");
        assert_eq!(body["skip_discovery"], true);
        assert_eq!(
            body["config"]["schemas"][0]["tables"][0]["name"],
            "orders"
        );
    }

    #[test]
    fn create_connection_request_empty_config_without_tables() {
        let body = create_connection_request("sales", "public", &[]);
        assert_eq!(body["config"], serde_json::json!({}));
    }

    #[test]
    fn managed_table_paths() {
        assert_eq!(
            managed_table_load_path("conn1", "public", "orders"),
            "/connections/conn1/schemas/public/tables/orders/loads"
        );
        assert_eq!(
            managed_table_delete_path("conn1", "analytics", "events"),
            "/connections/conn1/schemas/analytics/tables/events"
        );
    }

    #[test]
    fn load_table_request_is_replace_mode() {
        let body = load_table_request("upl_abc");
        assert_eq!(body["mode"], "replace");
        assert_eq!(body["upload_id"], "upl_abc");
    }

    #[test]
    fn is_parquet_path_by_extension() {
        assert!(is_parquet_path("/data/orders.parquet"));
        assert!(is_parquet_path("/data/ORDERS.PARQUET"));
        assert!(is_parquet_path("file.parquet"));
        assert!(!is_parquet_path("/data/orders.csv"));
        assert!(!is_parquet_path("/data/orders"));
    }

    #[test]
    fn table_rows_for_database_builds_full_names() {
        let rows = table_rows_for_database(
            "sales",
            vec![InfoTable {
                connection: "sales".into(),
                schema: "public".into(),
                table: "orders".into(),
                synced: true,
                last_sync: Some("2026-05-19T00:00:00Z".into()),
            }],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].full_name, "sales.public.orders");
        assert!(rows[0].synced);
    }

    #[test]
    fn collect_tables_follows_cursor() {
        let mut server = mockito::Server::new();
        let page1 = server
            .mock("GET", "/information_schema")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("connection_id".into(), "conn1".into()),
                mockito::Matcher::UrlEncoded("cursor".into(), "cur2".into()),
            ]))
            .with_status(200)
            .with_body(
                r#"{"tables":[{"connection":"sales","schema":"public","table":"b","synced":true,"last_sync":null}],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let page0 = server
            .mock("GET", "/information_schema")
            .match_query(mockito::Matcher::UrlEncoded(
                "connection_id".into(),
                "conn1".into(),
            ))
            .with_status(200)
            .with_body(
                r#"{"tables":[{"connection":"sales","schema":"public","table":"a","synced":false,"last_sync":null}],"has_more":true,"next_cursor":"cur2"}"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws"));
        let tables = collect_tables(&api, "conn1", None);
        page0.assert();
        page1.assert();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].table, "a");
        assert_eq!(tables[1].table, "b");
    }

    #[test]
    fn create_posts_managed_connection_with_schemas() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/connections")
            .match_header("X-Workspace-Id", "ws-test")
            .with_status(201)
            .with_body(
                r#"{"id":"conn_new","name":"mydb","source_type":"managed","tables_discovered":1,"discovery_status":"skipped"}"#,
            )
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&create_connection_request(
                    "mydb",
                    "public",
                    &["gdp".to_string()],
                ))
                .unwrap(),
            ))
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws-test"));
        let body = create_connection_request("mydb", "public", &["gdp".to_string()]);
        let (status, resp_body) = api.post_raw("/connections", &body);
        assert_eq!(status.as_u16(), 201);
        let parsed: CreateConnectionResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(parsed.name, "mydb");
        assert_eq!(parsed.source_type, "managed");
        mock.assert();
    }

    #[test]
    fn tables_load_posts_replace_with_upload_id() {
        let mut server = mockito::Server::new();
        let list = server
            .mock("GET", "/connections")
            .with_status(200)
            .with_body(
                r#"{"connections":[{"id":"conn1","name":"sales","source_type":"managed"}]}"#,
            )
            .create();
        let load = server
            .mock("POST", "/connections/conn1/schemas/public/tables/orders/loads")
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&load_table_request("upl_123")).unwrap(),
            ))
            .with_status(200)
            .with_body(
                r#"{
                "connection_id":"conn1",
                "schema_name":"public",
                "table_name":"orders",
                "row_count":42,
                "arrow_schema_json":"{}"
            }"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws1"));
        let db = resolve_database(&api, "sales");
        let path = managed_table_load_path(&db.id, "public", "orders");
        let body = load_table_request("upl_123");
        let (status, resp_body) = api.post_raw(&path, &body);
        assert!(status.is_success());
        let parsed: LoadManagedTableResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(parsed.row_count, 42);
        assert_eq!(parsed.table_name, "orders");
        list.assert();
        load.assert();
    }

    #[test]
    fn tables_delete_calls_managed_table_endpoint() {
        let mut server = mockito::Server::new();
        let list = server
            .mock("GET", "/connections")
            .with_status(200)
            .with_body(
                r#"{"connections":[{"id":"conn1","name":"sales","source_type":"managed"}]}"#,
            )
            .create();
        let delete = server
            .mock("DELETE", "/connections/conn1/schemas/public/tables/orders")
            .with_status(204)
            .with_body("")
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let db = resolve_database(&api, "sales");
        let path = managed_table_delete_path(&db.id, "public", "orders");
        let (status, _) = api.delete_raw(&path);
        assert_eq!(status.as_u16(), 204);
        list.assert();
        delete.assert();
    }

    #[test]
    fn load_response_parses_row_count_and_names() {
        let body = r#"{
            "connection_id":"conn1",
            "schema_name":"analytics",
            "table_name":"events",
            "row_count":99,
            "arrow_schema_json":"{}"
        }"#;
        let parsed: LoadManagedTableResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.schema_name, "analytics");
        assert_eq!(parsed.table_name, "events");
        assert_eq!(parsed.row_count, 99);
    }
}
