use crate::api::ApiClient;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEFAULT_SCHEMA: &str = "public";

/// Summary row returned by `GET /databases` (no `default_connection_id`).
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct DatabaseSummary {
    id: String,
    description: Option<String>,
}

#[derive(Deserialize)]
struct ListDatabasesResponse {
    databases: Vec<DatabaseSummary>,
}

/// Full record returned by `GET /databases/{id}`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Database {
    pub id: String,
    pub description: Option<String>,
    pub default_connection_id: String,
    #[serde(default)]
    attachments: Vec<DatabaseAttachment>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
struct DatabaseAttachment {
    connection_id: String,
    alias: Option<String>,
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
struct CreateDatabaseResponse {
    id: String,
    description: Option<String>,
    default_connection_id: String,
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

fn fetch_database(api: &ApiClient, id: &str) -> Database {
    api.get(&format!("/databases/{id}"))
}

pub fn try_resolve_database(api: &ApiClient, id_or_description: &str) -> Result<Database, String> {
    // Try a direct id lookup first — avoids the list round-trip for the common case.
    // Percent-encode the segment so descriptions containing spaces or other URL-unsafe
    // characters don't cause a URL parse error before the list fallback can run.
    let encoded = urlencoding::encode(id_or_description);
    if let Some(db) = api.get_none_if_not_found(&format!("/databases/{encoded}")) {
        return Ok(db);
    }

    // Fall back to listing and matching by description.
    let body: ListDatabasesResponse = api.get("/databases");
    let desc_matches: Vec<&DatabaseSummary> = body
        .databases
        .iter()
        .filter(|d| d.description.as_deref() == Some(id_or_description))
        .collect();

    match desc_matches.len() {
        0 => Err(format!(
            "no database with id or description '{id_or_description}'"
        )),
        1 => Ok(fetch_database(api, &desc_matches[0].id)),
        _ => Err(format!(
            "multiple databases have description '{}' — use the database id instead",
            id_or_description
        )),
    }
}

pub fn resolve_database(api: &ApiClient, id_or_description: &str) -> Database {
    match try_resolve_database(api, id_or_description) {
        Ok(db) => db,
        Err(e) => {
            use crossterm::style::Stylize;
            eprintln!("{}", format!("error: {e}").red());
            std::process::exit(1);
        }
    }
}

fn schema_name(schema: Option<&str>) -> &str {
    schema.unwrap_or(DEFAULT_SCHEMA)
}

/// Build the request body for `POST /v1/databases`.
pub fn create_database_request(
    description: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
) -> serde_json::Value {
    let mut req = serde_json::Map::new();

    if let Some(desc) = description {
        req.insert(
            "description".to_string(),
            serde_json::Value::String(desc.to_string()),
        );
    }

    if !tables.is_empty() {
        let table_objs: Vec<serde_json::Value> = tables
            .iter()
            .map(|t| serde_json::json!({ "name": t }))
            .collect();
        req.insert(
            "schemas".to_string(),
            serde_json::json!([{ "name": schema, "tables": table_objs }]),
        );
    }

    if let Some(exp) = expires_at {
        req.insert(
            "expires_at".to_string(),
            serde_json::Value::String(exp.to_string()),
        );
    }

    serde_json::Value::Object(req)
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

fn table_rows(tables: Vec<InfoTable>) -> Vec<TableRow> {
    tables
        .into_iter()
        .map(|t| TableRow {
            full_name: format!("default.{}.{}", t.schema, t.table),
            schema: t.schema,
            table: t.table,
            synced: t.synced,
            last_sync: t.last_sync,
        })
        .collect()
}

fn finish_upload(
    api: &ApiClient,
    reader: impl std::io::Read + Send + 'static,
    size: Option<u64>,
    pb: &ProgressBar,
) -> String {
    let (status, resp_body) = api.post_body("/files", "application/octet-stream", reader, size);
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

fn upload_parquet_file(api: &ApiClient, path: &str) -> String {
    if !is_parquet_path(path) {
        eprintln!(
            "error: managed table loads require a parquet file (got '{}'). \
             Convert your data to parquet or use `hotdata datasets create` for CSV/JSON.",
            path
        );
        std::process::exit(1);
    }

    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error opening file '{path}': {e}");
            std::process::exit(1);
        }
    };

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
    finish_upload(api, reader, Some(file_size), &pb)
}

fn upload_parquet_url(api: &ApiClient, url: &str) -> String {
    if !is_parquet_path(url) {
        eprintln!(
            "error: managed table loads require a parquet URL ending in .parquet (got '{url}')."
        );
        std::process::exit(1);
    }

    let resp = match reqwest::blocking::get(url) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error fetching '{url}': {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        eprintln!(
            "error: remote server returned {} for '{url}'",
            resp.status()
        );
        std::process::exit(1);
    }

    let content_length = resp.content_length();
    let pb = match content_length {
        Some(len) => {
            let pb = ProgressBar::new(len);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
                )
                .unwrap()
                .progress_chars("=>-"),
            );
            pb
        }
        None => {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::with_template("{spinner:.green} {bytes} downloaded ({elapsed})")
                    .unwrap(),
            );
            pb
        }
    };
    let reader = pb.wrap_read(resp);
    finish_upload(api, reader, content_length, &pb)
}

fn collect_tables(api: &ApiClient, connection_id: &str, schema: Option<&str>) -> Vec<InfoTable> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut params: Vec<(&str, Option<String>)> =
            vec![("connection_id", Some(connection_id.to_string()))];
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
    let body: ListDatabasesResponse = api.get("/databases");

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.databases).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.databases).unwrap()),
        "table" => {
            if body.databases.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No databases found.".dark_grey());
                eprintln!(
                    "{}",
                    "Create one with: hotdata databases create".dark_grey()
                );
            } else {
                let rows: Vec<Vec<String>> = body
                    .databases
                    .iter()
                    .map(|d| {
                        vec![
                            d.description.as_deref().unwrap_or("-").to_string(),
                            d.id.clone(),
                        ]
                    })
                    .collect();
                crate::table::print(&["DESCRIPTION", "ID"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(workspace_id: &str, id_or_description: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, id_or_description);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&db).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&db).unwrap()),
        "table" => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<24}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), db.id.clone().dark_cyan());
            println!(
                "{}{}",
                label("description:"),
                db.description.as_deref().unwrap_or("-").white()
            );
            println!(
                "{}{}",
                label("default_connection_id:"),
                db.default_connection_id.clone().dark_cyan()
            );
            println!(
                "{}{}",
                label("sql_prefix:"),
                "default.{schema}.{table}  (pass X-Database-Id header when querying)".green()
            );
            if !db.attachments.is_empty() {
                println!("{}({})", label("attached catalogs:"), db.attachments.len());
                for a in &db.attachments {
                    let alias = a
                        .alias
                        .as_deref()
                        .map(|al| format!(" as {al}"))
                        .unwrap_or_default();
                    println!(
                        "  {}{}",
                        a.connection_id.clone().dark_cyan(),
                        alias.dark_grey()
                    );
                }
            }
        }
        _ => unreachable!(),
    }
}

pub fn create(
    workspace_id: &str,
    description: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
    format: &str,
) {
    use crossterm::style::Stylize;

    let body = create_database_request(description, schema, tables, expires_at);

    let api = ApiClient::new(Some(workspace_id));
    let spinner = (format == "table").then(|| crate::util::spinner("Creating database..."));
    let (status, resp_body) = api.post_raw("/databases", &body);
    if let Some(s) = &spinner {
        s.finish_and_clear();
    }

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let result: CreateDatabaseResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = crate::config::save_current_database("default", workspace_id, &result.id) {
        use crossterm::style::Stylize;
        eprintln!("{}", format!("warning: database created but could not set as current: {e}").yellow());
    }

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&result).unwrap()),
        "table" => {
            println!("{}", "Database created".green());
            if let Some(desc) = &result.description {
                println!("description: {desc}");
            }
            println!("id:          {}", result.id);
            println!();
            println!(
                "{}",
                format!(
                    concat!(
                        "Load a table:\n",
                        "  hotdata databases load --file <path.parquet> {}.<table_name>\n",
                        "\nQuery with:\n",
                        "  hotdata query --database {} \"SELECT * FROM default.public.<table> LIMIT 10\"\n",
                        "\n  Tip: use 'default.<schema>.<table>' as the SQL prefix (not the database or connection id)\n",
                        "  Column names are case-sensitive — wrap uppercase names in double quotes",
                    ),
                    result.id, result.id
                )
                .dark_grey()
            );
        }
        _ => unreachable!(),
    }
}

pub fn set(workspace_id: &str, id_or_description: &str) {
    use crossterm::style::Stylize;
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, id_or_description);
    if let Err(e) = crate::config::save_current_database("default", workspace_id, &db.id) {
        eprintln!("{}", format!("error saving current database: {e}").red());
        std::process::exit(1);
    }
    println!("{}", format!("Current database set to {}", db.id).green());
}

fn resolve_current_database(provided: Option<&str>, workspace_id: &str) -> String {
    if let Some(id) = provided {
        return id.to_string();
    }
    match crate::config::load_current_database("default", workspace_id) {
        Some(id) => id,
        None => {
            use crossterm::style::Stylize;
            eprintln!(
                "{}",
                "error: no current database set. Use 'hotdata databases set <id>' or pass a database id.".red()
            );
            std::process::exit(1);
        }
    }
}

pub fn delete(workspace_id: &str, id_or_description: &str) {
    use crossterm::style::Stylize;

    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, id_or_description);
    let (status, resp_body) = api.delete_raw(&format!("/databases/{}", db.id));

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    // If the deleted database was the current one, clear it so subsequent
    // commands don't silently send a stale X-Database-Id header.
    if crate::config::load_current_database("default", workspace_id).as_deref() == Some(&db.id) {
        let _ = crate::config::clear_current_database("default", workspace_id);
    }

    println!("{}", "Database deleted.".green());
}

pub fn tables_list(workspace_id: &str, database: Option<&str>, schema: Option<&str>, format: &str) {
    let database = resolve_current_database(database, workspace_id);
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let tables = collect_tables(&api, &db.default_connection_id, schema);

    let rows = table_rows(tables);

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
    database: Option<&str>,
    table: &str,
    schema: Option<&str>,
    file: Option<&str>,
    url: Option<&str>,
    upload_id: Option<&str>,
) {
    use crossterm::style::Stylize;

    let database = resolve_current_database(database, workspace_id);
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let schema = schema_name(schema);

    // clap enforces mutual exclusion; only one of these is ever Some.
    let upload_id = match (upload_id, file, url) {
        (Some(id), None, None) => id.to_string(),
        (None, Some(path), None) => upload_parquet_file(&api, path),
        (None, None, Some(u)) => upload_parquet_url(&api, u),
        (None, None, None) => {
            eprintln!("error: --file <path>, --url <url>, or --upload-id <id> is required");
            std::process::exit(1);
        }
        _ => unreachable!(),
    };

    let path = managed_table_load_path(&db.default_connection_id, schema, table);
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
                "Declare the table when creating the database, e.g.:\n  \
                 hotdata databases create --table <table>"
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

    let full_name = format!("default.{}.{}", result.schema_name, result.table_name);
    println!("{}", "Table loaded".green());
    println!("full_name: {}", full_name.clone().green());
    println!("rows:      {}", result.row_count);
    println!();
    println!(
        "{}",
        format!(
            concat!(
                "Query it now:\n",
                "  hotdata query \"SELECT * FROM {} LIMIT 10\"\n",
                "\n  Tip: column names are case-sensitive.\n",
                "  Wrap uppercase names in double quotes: SELECT \"MyColumn\" FROM {} LIMIT 10",
            ),
            full_name, full_name
        )
        .dark_grey()
    );
}

pub fn tables_delete(workspace_id: &str, database: Option<&str>, table: &str, schema: Option<&str>) {
    use crossterm::style::Stylize;

    let database = resolve_current_database(database, workspace_id);
    let api = ApiClient::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let schema = schema_name(schema);

    let path = managed_table_delete_path(&db.default_connection_id, schema, table);
    let (status, resp_body) = api.delete_raw(&path);

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    println!(
        "{}",
        format!("Table 'default.{}.{}' deleted.", schema, table).green()
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
    fn create_database_request_empty_without_description_or_tables() {
        let req = create_database_request(None, "public", &[], None);
        assert_eq!(req, serde_json::json!({}));
    }

    #[test]
    fn create_database_request_includes_description() {
        let req = create_database_request(Some("my db"), "public", &[], None);
        assert_eq!(req["description"], "my db");
        assert!(req.get("schemas").is_none());
    }

    #[test]
    fn create_database_request_includes_schemas_when_tables_declared() {
        let req = create_database_request(
            Some("sales"),
            "public",
            &["orders".to_string(), "customers".to_string()],
            None,
        );
        assert_eq!(req["description"], "sales");
        assert_eq!(req["schemas"][0]["name"], "public");
        assert_eq!(req["schemas"][0]["tables"][0]["name"], "orders");
        assert_eq!(req["schemas"][0]["tables"][1]["name"], "customers");
    }

    #[test]
    fn create_database_request_schemas_without_description() {
        let req = create_database_request(None, "analytics", &["events".to_string()], None);
        assert!(req.get("description").is_none());
        assert_eq!(req["schemas"][0]["name"], "analytics");
    }

    #[test]
    fn create_database_request_includes_expires_at_when_provided() {
        let req = create_database_request(None, "public", &[], Some("24h"));
        assert_eq!(req["expires_at"], "24h");
    }

    #[test]
    fn create_database_request_omits_expires_at_when_none() {
        let req = create_database_request(None, "public", &[], None);
        assert!(req.get("expires_at").is_none());
    }

    fn full_detail(id: &str, desc: &str, conn_id: &str) -> String {
        format!(
            r#"{{"id":"{id}","description":"{desc}","default_connection_id":"{conn_id}","attachments":[]}}"#
        )
    }

    #[test]
    fn resolve_database_by_id_and_description() {
        let mut server = mockito::Server::new();
        // by-id path: direct GET /databases/db_abc succeeds
        let by_id_mock = server
            .mock("GET", "/databases/db_abc")
            .with_status(200)
            .with_body(full_detail("db_abc", "sales", "conn_1"))
            .create();
        // by-description path: GET /databases/warehouse → 404, then list, then detail
        let not_id = server
            .mock("GET", "/databases/warehouse")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        let list = server
            .mock("GET", "/databases")
            .with_status(200)
            .with_body(
                r#"{"databases":[{"id":"db_abc","description":"sales"},{"id":"db_xyz","description":"warehouse"}]}"#,
            )
            .create();
        let detail = server
            .mock("GET", "/databases/db_xyz")
            .with_status(200)
            .with_body(full_detail("db_xyz", "warehouse", "conn_2"))
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws"));
        let by_id = resolve_database(&api, "db_abc");
        assert_eq!(by_id.default_connection_id, "conn_1");
        let by_desc = resolve_database(&api, "warehouse");
        assert_eq!(by_desc.id, "db_xyz");
        by_id_mock.assert();
        not_id.assert();
        list.assert();
        detail.assert();
    }

    #[test]
    fn try_resolve_database_not_found() {
        let mut server = mockito::Server::new();
        // Direct id lookup returns 404
        server
            .mock("GET", "/databases/missing")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        // List also returns nothing
        server
            .mock("GET", "/databases")
            .with_status(200)
            .with_body(r#"{"databases":[]}"#)
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let err = try_resolve_database(&api, "missing").unwrap_err();
        assert!(err.contains("no database with id or description"));
    }

    #[test]
    fn try_resolve_database_rejects_ambiguous_description() {
        let mut server = mockito::Server::new();
        // Direct id lookup returns 404 (description isn't a valid id)
        server
            .mock("GET", "/databases/sales")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        // List returns two entries with the same description
        server
            .mock("GET", "/databases")
            .with_status(200)
            .with_body(
                r#"{"databases":[{"id":"db_1","description":"sales"},{"id":"db_2","description":"sales"}]}"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let err = try_resolve_database(&api, "sales").unwrap_err();
        assert!(err.contains("multiple databases"));
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
    fn table_rows_uses_default_prefix() {
        let rows = table_rows(vec![InfoTable {
            connection: "ignored".into(),
            schema: "public".into(),
            table: "orders".into(),
            synced: true,
            last_sync: Some("2026-05-19T00:00:00Z".into()),
        }]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].full_name, "default.public.orders");
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
                r#"{"tables":[{"connection":"default","schema":"public","table":"b","synced":true,"last_sync":null}],"has_more":false,"next_cursor":null}"#,
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
                r#"{"tables":[{"connection":"default","schema":"public","table":"a","synced":false,"last_sync":null}],"has_more":true,"next_cursor":"cur2"}"#,
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
    fn create_posts_to_databases_endpoint() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/databases")
            .match_header("X-Workspace-Id", "ws-test")
            .with_status(201)
            .with_body(
                r#"{"id":"db_new","description":"mydb","default_connection_id":"conn_abc"}"#,
            )
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&create_database_request(
                    Some("mydb"),
                    "public",
                    &["gdp".to_string()],
                    None,
                ))
                .unwrap(),
            ))
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws-test"));
        let body = create_database_request(Some("mydb"), "public", &["gdp".to_string()], None);
        let (status, resp_body) = api.post_raw("/databases", &body);
        assert_eq!(status.as_u16(), 201);
        let parsed: CreateDatabaseResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(parsed.description.as_deref(), Some("mydb"));
        assert_eq!(parsed.default_connection_id, "conn_abc");
        mock.assert();
    }

    #[test]
    fn tables_load_uses_default_connection_id() {
        let mut server = mockito::Server::new();
        // resolve_database resolves by id directly
        let resolve = server
            .mock("GET", "/databases/db_1")
            .with_status(200)
            .with_body(full_detail("db_1", "sales", "conn_default"))
            .create();
        let load = server
            .mock(
                "POST",
                "/connections/conn_default/schemas/public/tables/orders/loads",
            )
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&load_table_request("upl_123")).unwrap(),
            ))
            .with_status(200)
            .with_body(
                r#"{
                "connection_id":"conn_default",
                "schema_name":"public",
                "table_name":"orders",
                "row_count":42,
                "arrow_schema_json":"{}"
            }"#,
            )
            .create();

        let api = ApiClient::test_new(&server.url(), "k", Some("ws1"));
        let db = resolve_database(&api, "db_1");
        let path = managed_table_load_path(&db.default_connection_id, "public", "orders");
        let body = load_table_request("upl_123");
        let (status, resp_body) = api.post_raw(&path, &body);
        assert!(status.is_success());
        let parsed: LoadManagedTableResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(parsed.row_count, 42);
        assert_eq!(parsed.table_name, "orders");
        resolve.assert();
        load.assert();
    }

    #[test]
    fn tables_delete_uses_default_connection_id() {
        let mut server = mockito::Server::new();
        let resolve = server
            .mock("GET", "/databases/db_1")
            .with_status(200)
            .with_body(full_detail("db_1", "sales", "conn_default"))
            .create();
        let delete = server
            .mock(
                "DELETE",
                "/connections/conn_default/schemas/public/tables/orders",
            )
            .with_status(204)
            .with_body("")
            .create();

        let api = ApiClient::test_new(&server.url(), "k", None);
        let db = resolve_database(&api, "db_1");
        let path = managed_table_delete_path(&db.default_connection_id, "public", "orders");
        let (status, _) = api.delete_raw(&path);
        assert_eq!(status.as_u16(), 204);
        resolve.assert();
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
