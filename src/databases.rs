use crate::sdk::{Api, ApiError, block, block_with_wakeup, none_if_404};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::Path;

const DEFAULT_SCHEMA: &str = "public";

/// CLI output shape for `databases list` rows. A curated, stably-ordered view
/// mapped from the SDK's `DatabaseSummary` (see the `From` impl) so the
/// `-o json`/`-o yaml` contract stays decoupled from generated-model field
/// order and nullability.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct DatabaseSummary {
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    default_catalog: Option<String>,
}

/// CLI output shape for `databases get`. A curated, stably-ordered view mapped
/// from the SDK's `DatabaseDetailResponse` (see the `From` impl), keeping the
/// `-o json`/`-o yaml` contract independent of the generated model's field order
/// and `Option<Option<_>>` nullability.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Database {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub default_catalog: Option<String>,
    pub default_connection_id: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    attachments: Vec<DatabaseAttachment>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
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
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    default_catalog: Option<String>,
    default_connection_id: String,
    #[serde(default)]
    expires_at: Option<String>,
}

/// Response shape of `POST /v1/auth/database`.
#[derive(Deserialize)]
struct DatabaseTokenResponse {
    token: String,
    refresh_token: String,
    database_id: String,
    expires_in: u64,
    refresh_expires_in: u64,
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

impl From<hotdata::models::DatabaseDetailResponse> for Database {
    /// Map the SDK's typed detail response into the CLI output shape, flattening
    /// the SDK's `Option<Option<_>>` nullable fields and wrapping the
    /// SDK-required `default_catalog` as `Some`.
    fn from(d: hotdata::models::DatabaseDetailResponse) -> Self {
        Database {
            id: d.id,
            name: d.name.flatten(),
            default_catalog: Some(d.default_catalog),
            default_connection_id: d.default_connection_id,
            expires_at: d.expires_at.flatten(),
            attachments: d
                .attachments
                .into_iter()
                .map(|a| DatabaseAttachment {
                    connection_id: a.connection_id,
                    alias: a.alias.flatten(),
                })
                .collect(),
        }
    }
}

impl From<hotdata::models::DatabaseSummary> for DatabaseSummary {
    /// Map the SDK's typed summary into the CLI's list-row output shape.
    fn from(s: hotdata::models::DatabaseSummary) -> Self {
        DatabaseSummary {
            id: s.id,
            name: s.name.flatten(),
            default_catalog: Some(s.default_catalog),
        }
    }
}

/// Fetch a database by id through the SDK's typed `databases().get` handle.
///
/// The handle owns auth, scope headers, URL construction, and percent-encoding
/// of the id segment, so callers no longer hand-roll the path. The result is
/// mapped into the CLI's `Database`.
pub(crate) fn get_database(api: &Api, id: &str) -> Result<Database, ApiError> {
    block(api.client().databases().get(id)).map(Database::from)
}

/// List databases through the SDK's typed `databases().list` handle, mapped
/// into the CLI's summary rows.
///
/// Routed through [`block_with_wakeup`] so a cold KEDA start surfaces a "waking
/// up worker" hint instead of an unexplained pause — `databases list` is a
/// common first command against an idle workspace.
fn list_database_summaries(api: &Api) -> Result<Vec<DatabaseSummary>, ApiError> {
    block_with_wakeup(api, "Loading databases…", api.client().databases().list())
        .map(|r| r.databases.into_iter().map(DatabaseSummary::from).collect())
}

/// List the ids of every managed database in the workspace.
///
/// Exposed for the whole-workspace `indexes list` scan (#168): a managed
/// database's connection is hidden from `connections list` and its tables are
/// absent from the unscoped `information_schema` enumeration, so that scan
/// rediscovers managed databases here and resolves each one's
/// `default_connection_id` via [`get_database`]. The list summary omits the
/// connection id, hence ids only.
pub(crate) fn list_database_ids(api: &Api) -> Result<Vec<String>, ApiError> {
    list_database_summaries(api).map(|dbs| dbs.into_iter().map(|d| d.id).collect())
}

fn fetch_database(api: &Api, id: &str) -> Database {
    get_database(api, id).unwrap_or_else(|e| e.exit())
}

pub fn try_resolve_database(api: &Api, id_or_name: &str) -> Result<Database, String> {
    // Try a direct id lookup first — avoids the list round-trip for the common
    // case. The typed handle percent-encodes the id segment, so names with
    // spaces or other URL-unsafe characters resolve (or 404 cleanly) without
    // manual encoding before the list fallback runs.
    if let Some(db) = none_if_404(get_database(api, id_or_name)).unwrap_or_else(|e| e.exit()) {
        return Ok(db);
    }

    // Fall back to listing — prefer catalog alias match, then name.
    let databases = list_database_summaries(api).unwrap_or_else(|e| e.exit());

    let catalog_matches: Vec<&DatabaseSummary> = databases
        .iter()
        .filter(|d| d.default_catalog.as_deref() == Some(id_or_name))
        .collect();

    if !catalog_matches.is_empty() {
        return match catalog_matches.len() {
            1 => Ok(fetch_database(api, &catalog_matches[0].id)),
            _ => Err(format!(
                "multiple databases have catalog '{}' — use the database id instead",
                id_or_name
            )),
        };
    }

    let name_matches: Vec<&DatabaseSummary> = databases
        .iter()
        .filter(|d| d.name.as_deref() == Some(id_or_name))
        .collect();

    match name_matches.len() {
        0 => Err(format!(
            "no database with id, catalog, or name '{id_or_name}'"
        )),
        1 => Ok(fetch_database(api, &name_matches[0].id)),
        _ => Err(format!(
            "multiple databases have name '{}' — use the database id instead",
            id_or_name
        )),
    }
}

pub fn resolve_database(api: &Api, id_or_name: &str) -> Database {
    match try_resolve_database(api, id_or_name) {
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
    name: Option<&str>,
    catalog: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
) -> serde_json::Value {
    let mut req = serde_json::Map::new();

    if let Some(n) = name {
        req.insert("name".to_string(), serde_json::Value::String(n.to_string()));
    }

    if let Some(c) = catalog {
        req.insert(
            "default_catalog".to_string(),
            serde_json::Value::String(c.to_string()),
        );
    }

    if !tables.is_empty() {
        // Group tables by schema, preserving insertion order.
        // Dot-notation entries (e.g. "raw.raw_orders") use the named schema;
        // bare names fall back to the `schema` argument.
        let mut schema_tables: Vec<(String, Vec<String>)> = Vec::new();
        for t in tables {
            let (s, table_name) = match t.split_once('.') {
                Some((s, tbl)) => (s.to_string(), tbl.to_string()),
                None => (schema.to_string(), t.to_string()),
            };
            if let Some(entry) = schema_tables.iter_mut().find(|(n, _)| n == &s) {
                entry.1.push(table_name);
            } else {
                schema_tables.push((s, vec![table_name]));
            }
        }
        let schemas_json: Vec<serde_json::Value> = schema_tables
            .into_iter()
            .map(|(s, tbls)| {
                serde_json::json!({
                    "name": s,
                    "tables": tbls.iter().map(|t| serde_json::json!({ "name": t })).collect::<Vec<_>>()
                })
            })
            .collect();
        req.insert(
            "schemas".to_string(),
            serde_json::Value::Array(schemas_json),
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

/// Build the typed `CreateDatabaseRequest` for the SDK's `databases().create`
/// handle, reusing [`create_database_request`] as the single source of truth for
/// the body shape. The delete+recreate path still consumes the raw JSON form
/// (it inspects raw error bodies), so the JSON builder stays; this just adapts
/// it for the typed call sites.
fn create_database_typed_request(
    name: Option<&str>,
    catalog: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
) -> hotdata::models::CreateDatabaseRequest {
    serde_json::from_value(create_database_request(
        name, catalog, schema, tables, expires_at,
    ))
    .expect("create_database_request always emits a valid CreateDatabaseRequest body")
}

pub fn managed_table_load_path(connection_id: &str, schema: &str, table: &str) -> String {
    format!("/connections/{connection_id}/schemas/{schema}/tables/{table}/loads")
}

pub fn managed_table_delete_path(connection_id: &str, schema: &str, table: &str) -> String {
    format!("/connections/{connection_id}/schemas/{schema}/tables/{table}")
}

/// Database-scoped managed-table endpoints (addressed by database id, not
/// connection id). These are the paths a database API token is allowed to use —
/// the connection-scoped variants above are denied for it.
pub fn database_table_load_path(database_id: &str, schema: &str, table: &str) -> String {
    format!("/databases/{database_id}/schemas/{schema}/tables/{table}/loads")
}

pub fn database_schemas_path(database_id: &str) -> String {
    format!("/databases/{database_id}/schemas")
}

pub fn database_schema_tables_path(database_id: &str, schema: &str) -> String {
    format!("/databases/{database_id}/schemas/{schema}/tables")
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

fn table_rows(catalog: &str, tables: Vec<InfoTable>) -> Vec<TableRow> {
    tables
        .into_iter()
        .map(|t| TableRow {
            full_name: format!("{catalog}.{}.{}", t.schema, t.table),
            schema: t.schema,
            table: t.table,
            synced: t.synced,
            last_sync: t.last_sync,
        })
        .collect()
}

/// The shared indicatif progress-bar template for an upload: a spinner, a
/// byte-granular bar, the bytes-done / total, and an ETA.
fn upload_progress_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
    )
    .unwrap()
    .progress_chars("=>-")
}

/// Upload an already-on-disk parquet file via the SDK's presigned direct-to-
/// storage flow, driving a single aggregate progress bar from the SDK's
/// byte-granular progress callback. Returns the finalized upload id, or the
/// seam's error (a `501 PRESIGN_UNSUPPORTED` surfaces an actionable message,
/// not a fallback). The caller decides how to surface failure — `--url` must
/// clean up its temp file before exiting, so this returns rather than exits.
fn upload_parquet_path(api: &Api, path: &Path, size: u64) -> Result<String, ApiError> {
    let pb = ProgressBar::new(size);
    pb.set_style(upload_progress_style());

    // The SDK reports cumulative `(done, total)`; mirror it onto the bar. We
    // `set_length(total)` defensively so the bar tracks the SDK's own notion of
    // total even though it equals `size` here.
    let cb_pb = pb.clone();
    let progress: hotdata::UploadProgress = std::sync::Arc::new(move |done, total| {
        cb_pb.set_length(total);
        cb_pb.set_position(done);
    });

    let result = api.upload(path, progress);
    pb.finish_and_clear();
    result
}

fn upload_parquet_file(api: &Api, path: &str) -> String {
    if !is_parquet_path(path) {
        eprintln!(
            "error: managed table loads require a parquet file (got '{}'). \
             Convert your data to parquet first.",
            path
        );
        std::process::exit(1);
    }

    let file_size = match std::fs::metadata(path) {
        Ok(m) => m.len(),
        Err(e) => {
            eprintln!("error opening file '{path}': {e}");
            std::process::exit(1);
        }
    };

    upload_parquet_path(api, Path::new(path), file_size).unwrap_or_else(|e| e.exit())
}

fn upload_parquet_url(api: &Api, url: &str) -> String {
    if !is_parquet_path(url) {
        eprintln!(
            "error: managed table loads require a parquet URL ending in .parquet (got '{url}')."
        );
        std::process::exit(1);
    }

    // The presigned upload needs a seekable, size-known source (the SDK opens
    // the path, declares its byte count, and PUTs it directly to storage), so
    // download the URL to a temp file first, then upload that file on the same
    // path as `--file`. The temp file is removed before this returns on both
    // success and failure (see `upload_temp_file`).
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
    // Download progress: a byte bar when the length is known, else a spinner.
    let dl_pb = match content_length {
        Some(len) => {
            let pb = ProgressBar::new(len);
            pb.set_style(upload_progress_style());
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

    let temp = match download_to_temp(resp, &dl_pb) {
        Ok(t) => t,
        Err(e) => {
            dl_pb.finish_and_clear();
            eprintln!("error downloading '{url}': {e}");
            std::process::exit(1);
        }
    };
    dl_pb.finish_and_clear();

    let size = std::fs::metadata(temp.path()).map(|m| m.len()).unwrap_or(0);
    upload_temp_file(temp, |path| upload_parquet_path(api, path, size)).unwrap_or_else(|e| e.exit())
}

/// Upload an already-downloaded temp file, guaranteeing the file is deleted
/// before returning — on both success and failure.
///
/// The temp file MUST be cleaned up here, on return, rather than left to a
/// guard in the caller's scope: the caller exits the process via
/// [`ApiError::exit`] (`std::process::exit`) on the `Err` arm, and
/// `process::exit` runs no destructors. Owning `temp` in this function means it
/// drops (deleting a potentially multi-GB download) before the caller can exit.
fn upload_temp_file<F>(temp: tempfile::NamedTempFile, upload: F) -> Result<String, ApiError>
where
    F: FnOnce(&Path) -> Result<String, ApiError>,
{
    let result = upload(temp.path());
    // Delete now, while still inside this function, so cleanup precedes any
    // `process::exit` the caller performs on `Err`.
    drop(temp);
    result
}

/// Stream a blocking HTTP response body to a freshly created temp file,
/// advancing `pb` as bytes land. Returns the open [`NamedTempFile`], which
/// deletes the file on drop. Created atomically with `O_EXCL` + 0600 perms via
/// `tempfile`, so it can't be redirected by a pre-planted symlink.
fn download_to_temp(
    resp: reqwest::blocking::Response,
    pb: &ProgressBar,
) -> std::io::Result<tempfile::NamedTempFile> {
    use std::io::Write;

    let mut temp = tempfile::Builder::new()
        .prefix("hotdata-upload-")
        .suffix(".parquet")
        .tempfile()?;

    let mut reader = pb.wrap_read(resp);
    std::io::copy(&mut reader, temp.as_file_mut())?;
    temp.as_file_mut().flush()?;
    Ok(temp)
}

fn collect_tables(api: &Api, connection_id: &str, schema: Option<&str>) -> Vec<InfoTable> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let resp = crate::sdk::block(api.client().information_schema().get(
            Some(connection_id),
            schema,
            None,
            None,
            None,
            cursor.as_deref(),
        ))
        .unwrap_or_else(|e| e.exit());
        out.extend(resp.tables.into_iter().map(|t| InfoTable {
            connection: t.connection,
            schema: t.schema,
            table: t.table,
            synced: t.synced,
            last_sync: t.last_sync.flatten(),
        }));
        if !resp.has_more {
            break;
        }
        let Some(c) = resp.next_cursor.flatten() else {
            break;
        };
        cursor = Some(c);
    }
    out.sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.table.cmp(&b.table)));
    out
}

pub fn list(workspace_id: &str, format: &str) {
    let api = Api::new(Some(workspace_id));
    let databases = list_database_summaries(&api).unwrap_or_else(|e| e.exit());

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&databases).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&databases).unwrap()),
        "table" => {
            if databases.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No databases found.".dark_grey());
                eprintln!(
                    "{}",
                    "Create one with: hotdata databases create --catalog <alias>".dark_grey()
                );
            } else {
                let current = crate::config::load_current_database("default", workspace_id);
                let rows: Vec<Vec<String>> = databases
                    .iter()
                    .map(|d| {
                        let marker = if current.as_deref() == Some(d.id.as_str()) {
                            "*"
                        } else {
                            ""
                        };
                        vec![
                            marker.to_string(),
                            d.id.clone(),
                            d.name.as_deref().unwrap_or("-").to_string(),
                        ]
                    })
                    .collect();
                crate::table::print(&["", "ID", "NAME"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(workspace_id: &str, id_or_name: &str, format: &str) {
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, id_or_name);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&db).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&db).unwrap()),
        "table" => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<24}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), db.id.clone().dark_cyan());
            if let Some(n) = &db.name {
                println!("{}{}", label("name:"), n.clone().cyan());
            }
            if let Some(c) = &db.default_catalog {
                println!("{}{}", label("catalog:"), c.clone().cyan());
            }
            println!(
                "{}{}",
                label("default_connection_id:"),
                db.default_connection_id.clone().dark_cyan()
            );
            let catalog = db
                .default_catalog
                .as_deref()
                .or(db.name.as_deref())
                .unwrap_or("default");
            println!(
                "{}{}",
                label("sql_prefix:"),
                format!(
                    "{catalog}.{{schema}}.{{table}}  (pass X-Database-Id header when querying)"
                )
                .green()
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

/// Attach a connection as a queryable catalog on a managed database, so its
/// live tables are visible inside that database's query scope (cross-source
/// joins without exporting data). Defaults to the current database.
pub fn attach(workspace_id: &str, connection: &str, database: Option<&str>, alias: Option<&str>) {
    use crossterm::style::Stylize;

    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let where_ = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or(&db.id);

    // Resolve + attach via the exiting paths (mirroring `detach`): a bad name
    // exits with the resolver's message, and an API failure goes through
    // `ApiError::exit`, which upgrades a masked 401/403 into the re-auth hint.
    // (The non-fatal `create --attach` loop uses `attach_connection` instead.)
    let connection_id = crate::connections::resolve_connection_id(&api, connection);
    send_attach(&api, &db.id, connection_id, alias).unwrap_or_else(|e| e.exit());

    match alias {
        Some(a) => println!(
            "{}",
            format!(
                "Attached '{connection}' to database '{where_}' as catalog '{a}'.\n\
                 Query: hotdata query \"SELECT * FROM {a}.<schema>.<table> LIMIT 10\" -d {where_}"
            )
            .green()
        ),
        None => println!(
            "{}",
            format!(
                "Attached '{connection}' to database '{where_}'. It is reachable by the \
                 connection's name; run `hotdata databases {where_}` to see attached catalogs."
            )
            .green()
        ),
    }
}

/// Detach a previously attached connection catalog from a managed database.
/// Defaults to the current database.
pub fn detach(workspace_id: &str, connection: &str, database: Option<&str>) {
    use crossterm::style::Stylize;

    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let where_ = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or(&db.id)
        .to_string();
    // Detach is keyed by the underlying connection id, but a user who attached
    // `github=gh` will naturally type `detach gh`. Resolve an attachment alias to
    // its connection id first; otherwise treat the argument as a connection
    // name/id like everywhere else.
    let connection_id = db
        .attachments
        .iter()
        .find(|a| a.alias.as_deref() == Some(connection))
        .map(|a| a.connection_id.clone())
        .unwrap_or_else(|| crate::connections::resolve_connection_id(&api, connection));

    block(
        api.client()
            .databases()
            .detach_catalog(&db.id, &connection_id),
    )
    .unwrap_or_else(|e| e.exit());

    println!(
        "{}",
        format!("Detached '{connection}' from database '{where_}'.").green()
    );
}

/// Create a database and return its id. Used by `run` when no
/// `--database` is given. Mirrors `create`'s request path but returns
/// the id instead of printing.
fn create_and_return_id(
    api: &Api,
    name: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
) -> String {
    let request = create_database_typed_request(name, None, schema, tables, expires_at);
    block(api.client().databases().create(request))
        .unwrap_or_else(|e| e.exit())
        .id
}

/// Mint a database-scoped JWT for an existing database id via
/// `POST /v1/auth/database` (grant_type=existing_database). The call
/// doubles as an existence + access check (the server 404s an unknown
/// or unreachable database).
fn mint_database_token(api: &Api, database_id: &str) -> DatabaseTokenResponse {
    let body = serde_json::json!({
        "grant_type": "existing_database",
        "database_id": database_id,
    });
    let (status, resp_body) = api
        .post_raw("/auth/database", &body)
        .unwrap_or_else(|e| e.exit());
    if !status.is_success() {
        // The old typed `api.post` routed non-success through `fail_response`,
        // which upgrades a masked 401/403/404 into the re-auth hint. Reproduce
        // that via the seam's auth-aware exit.
        crate::sdk::ApiError::Status {
            status,
            body: resp_body,
        }
        .exit();
    }
    match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    }
}

/// Run a command with a database-scoped token. Creates a new database
/// first when `database` is None, then mints a JWT and execs the
/// command with it injected as HOTDATA_DATABASE_TOKEN.
pub fn run(
    database: Option<&str>,
    workspace_id: &str,
    name: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
    cmd: &[String],
) {
    use crossterm::style::Stylize;
    use std::time::{SystemTime, UNIX_EPOCH};

    let api = Api::new(Some(workspace_id));

    // Unlike `create`, we don't persist the auto-created database as the
    // workspace's "current" database: a `run` database is scratch/ephemeral
    // for the child process, addressed only by the token we mint below.
    let database_id = match database {
        Some(id) => id.to_string(),
        None => create_and_return_id(&api, name, schema, tables, expires_at),
    };

    let resp = mint_database_token(&api, &database_id);
    let db_id = resp.database_id.clone();
    let db_jwt = resp.token.clone();
    let db_refresh = resp.refresh_token.clone();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let session = crate::database_session::DatabaseSession {
        access_token: db_jwt.clone(),
        refresh_token: db_refresh.clone(),
        database_id: db_id.clone(),
        workspace_id: workspace_id.to_string(),
        access_expires_at: now + resp.expires_in,
        refresh_expires_at: now + resp.refresh_expires_in,
    };
    if let Err(e) = crate::database_session::save(&session) {
        eprintln!("warning: could not persist database session: {e}");
    }

    eprintln!("{} {}", "database:".dark_grey(), db_id);
    eprintln!("{} {}", "workspace:".dark_grey(), workspace_id);

    let status = std::process::Command::new(&cmd[0])
        .args(&cmd[1..])
        .env("HOTDATA_DATABASE", &db_id)
        .env("HOTDATA_WORKSPACE", workspace_id)
        .env("HOTDATA_API_URL", &api.api_url)
        .env("HOTDATA_DATABASE_TOKEN", &db_jwt)
        .env("HOTDATA_DATABASE_REFRESH_TOKEN", &db_refresh)
        .status();

    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("error: failed to execute '{}': {e}", cmd[0]);
            std::process::exit(1);
        }
    }
}

/// Parse one `--attach` entry into `(connection, alias)`. `conn=alias` sets an
/// explicit SQL alias; a bare `conn` leaves the alias to default to the
/// connection's name. The connection part is a name or id resolved later.
fn parse_attach_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once('=') {
        Some((conn, alias)) => (conn.trim(), Some(alias.trim())),
        None => (spec.trim(), None),
    }
}

/// POST the attach request for an already-resolved `connection_id`. The single
/// place the `AttachDatabaseCatalogRequest` (incl. the double-Option alias) is
/// built, so the standalone `attach` command and the `create --attach` loop
/// share one request shape while handling the error differently (exit vs warn).
fn send_attach(
    api: &Api,
    database_id: &str,
    connection_id: String,
    alias: Option<&str>,
) -> Result<(), ApiError> {
    let mut request = hotdata::models::AttachDatabaseCatalogRequest::new(connection_id);
    request.alias = alias.map(|a| Some(a.to_string()));
    block(
        api.client()
            .databases()
            .attach_catalog(database_id, request),
    )
    .map(|_| ())
}

/// Attach a connection (by name or id) as a catalog on `database_id`, returning
/// the resolved connection id or `Err(message)` on a bad connection name/id or a
/// failed attach.
///
/// Returns the error (rather than exiting) so `create --attach` can warn on one
/// bad spec and still process the rest. Uses [`try_resolve_connection_id`] for
/// the same reason — `resolve_connection_id` would `process::exit` on an unknown
/// name and abort the whole `create` mid-loop. The standalone `attach` command
/// does NOT use this — it wants the auth-aware [`ApiError::exit`], so it calls
/// `resolve_connection_id` + [`send_attach`] directly.
fn attach_connection(
    api: &Api,
    database_id: &str,
    connection: &str,
    alias: Option<&str>,
) -> Result<String, String> {
    let connection_id = crate::connections::try_resolve_connection_id(api, connection)?;
    send_attach(api, database_id, connection_id.clone(), alias).map_err(|e| e.message())?;
    Ok(connection_id)
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    workspace_id: &str,
    name: Option<&str>,
    catalog: Option<&str>,
    schema: &str,
    tables: &[String],
    expires_at: Option<&str>,
    attach: &[String],
    format: &str,
) {
    use crossterm::style::Stylize;

    let request = create_database_typed_request(name, catalog, schema, tables, expires_at);

    let api = Api::new(Some(workspace_id));
    let resp = if format == "table" {
        block_with_wakeup(
            &api,
            "Creating database...",
            api.client().databases().create(request),
        )
    } else {
        block(api.client().databases().create(request))
    }
    .unwrap_or_else(|e| e.exit());

    // Attach requested connection catalogs onto the fresh database so a single
    // `databases create --attach …` lands a cross-source-ready context. A failed
    // attach is surfaced but non-fatal: the database exists and is usable.
    let attached: Vec<String> = attach
        .iter()
        .filter_map(|spec| {
            let (connection, alias) = parse_attach_spec(spec);
            match attach_connection(&api, &resp.id, connection, alias) {
                Ok(_) => Some(match alias {
                    Some(a) => format!("{connection} (as {a})"),
                    None => connection.to_string(),
                }),
                Err(e) => {
                    eprintln!(
                        "{}",
                        format!("warning: could not attach '{connection}': {e}").yellow()
                    );
                    None
                }
            }
        })
        .collect();

    let result = CreateDatabaseResponse {
        id: resp.id,
        name: resp.name.flatten(),
        default_catalog: Some(resp.default_catalog),
        default_connection_id: resp.default_connection_id,
        expires_at: resp.expires_at.flatten(),
    };

    if let Err(e) = crate::config::save_current_database("default", workspace_id, &result.id) {
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!("warning: database created but could not set as current: {e}").yellow()
        );
    }

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&result).unwrap()),
        "table" => {
            println!("{}", "Database created".green());
            if let Some(n) = &result.name {
                println!("name:        {}", n.clone().cyan());
            }
            if let Some(c) = &result.default_catalog {
                println!("catalog:     {}", c.clone().cyan());
            }
            println!("id:          {}", result.id);
            if let Some(exp) = &result.expires_at {
                println!("expires_at:  {exp}");
            }
            if !attached.is_empty() {
                println!("attached:    {}", attached.join(", ").cyan());
            }
            println!();
            let catalog = result
                .default_catalog
                .as_deref()
                .or(result.name.as_deref())
                .unwrap_or("default");
            println!(
                "{}",
                format!(
                    concat!(
                        "Load a table:\n",
                        "  hotdata databases load --catalog {0} --table <table> --file <path.parquet>\n",
                        "  hotdata databases load --catalog {0} --table <table> --url <url>\n",
                        "\nQuery with:\n",
                        "  hotdata query \"SELECT * FROM {0}.public.<table> LIMIT 10\"\n",
                        "\n  Tip: column names are case-sensitive — wrap uppercase names in double quotes",
                    ),
                    catalog
                )
                .dark_grey()
            );
        }
        _ => unreachable!(),
    }
}

pub fn unset(workspace_id: &str) {
    use crossterm::style::Stylize;
    if let Err(e) = crate::config::clear_current_database("default", workspace_id) {
        eprintln!("{}", format!("error clearing current database: {e}").red());
        std::process::exit(1);
    }
    println!("{}", "Current database cleared.".green());
}

pub fn set(workspace_id: &str, id: &str) {
    use crossterm::style::Stylize;
    // `set` only writes local config; the GET is just a friendly existence-check.
    // A database API token can't call GET /v1/databases/{id} (denied by its
    // allow-list), so skip the check for it and save the id directly.
    let is_database_api_token = crate::config::load("default")
        .ok()
        .and_then(|profile| crate::auth::api_key_jwt_source(&profile))
        .as_deref()
        == Some("database_api_token");
    if !is_database_api_token {
        let api = Api::new(Some(workspace_id));
        if none_if_404(get_database(&api, id))
            .unwrap_or_else(|e| e.exit())
            .is_none()
        {
            eprintln!("{}", format!("error: no database with id '{id}'").red());
            std::process::exit(1);
        }
    }
    if let Err(e) = crate::config::save_current_database("default", workspace_id, id) {
        eprintln!("{}", format!("error saving current database: {e}").red());
        std::process::exit(1);
    }
    println!("{}", format!("Current database set to {id}").green());
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

pub fn delete(workspace_id: &str, id_or_name: &str) {
    use crossterm::style::Stylize;

    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, id_or_name);
    block_with_wakeup(
        &api,
        "Deleting database…",
        api.client().databases().delete(&db.id),
    )
    .unwrap_or_else(|e| e.exit());

    // If the deleted database was the current one, clear it so subsequent
    // commands don't silently send a stale X-Database-Id header.
    if crate::config::load_current_database("default", workspace_id).as_deref() == Some(&db.id) {
        let _ = crate::config::clear_current_database("default", workspace_id);
    }

    println!("{}", "Database deleted.".green());
}

pub fn tables_list(workspace_id: &str, database: Option<&str>, schema: Option<&str>, format: &str) {
    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let catalog = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or("default");
    let tables = collect_tables(&api, &db.default_connection_id, schema);

    let rows = table_rows(catalog, tables);

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

    // A database API token can't resolve names/catalogs or use the
    // connection-scoped managed endpoints (all outside its allow-list). Route it
    // through the database-scoped endpoints, addressed by database id.
    if let Ok(profile) = crate::config::load("default")
        && crate::auth::api_key_jwt_source(&profile).as_deref() == Some("database_api_token")
    {
        tables_load_database_scoped(workspace_id, database, table, schema, file, url, upload_id);
        return;
    }

    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    // Prefer the active database when its catalog or name matches the lookup key,
    // avoiding ambiguity when multiple databases share the same catalog name.
    let active_id = crate::config::load_current_database("default", workspace_id);
    let lookup_key = match active_id.as_deref() {
        Some(id) => {
            if let Some(active) = none_if_404(get_database(&api, id)).unwrap_or_else(|e| e.exit()) {
                if active.default_catalog.as_deref() == Some(database.as_str())
                    || active.name.as_deref() == Some(database.as_str())
                {
                    id.to_string()
                } else {
                    database.clone()
                }
            } else {
                database.clone()
            }
        }
        None => database.clone(),
    };
    let db = resolve_database(&api, &lookup_key);
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
    let (status, resp_body) = api.post_raw(&path, &body).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();

    let (status, resp_body) = if !status.is_success()
        && crate::util::api_error(resp_body.clone()).contains("not declared")
    {
        // The table wasn't declared at create time. Collect existing tables so
        // they are re-declared in the replacement database, then delete and
        // recreate with all tables (including the new one) declared.
        let existing = collect_tables(&api, &db.default_connection_id, None);
        let mut all_tables: Vec<String> = existing
            .iter()
            .map(|t| format!("{}.{}", t.schema, t.table))
            .collect();
        let new_table_key = format!("{schema}.{table}");
        if !all_tables.contains(&new_table_key) {
            all_tables.push(new_table_key);
        }

        // Warn if any existing table has synced data — delete+recreate will lose it.
        let synced: Vec<String> = existing
            .iter()
            .filter(|t| t.synced)
            .map(|t| format!("{}.{}", t.schema, t.table))
            .collect();
        if !synced.is_empty() {
            use crossterm::style::Stylize;
            let catalog = db
                .default_catalog
                .as_deref()
                .or(db.name.as_deref())
                .unwrap_or(&db.id);
            eprintln!(
                "{}",
                format!(
                    "warning: declaring '{}' requires recreating the database '{catalog}'. \
                     The following tables have loaded data that will be lost:\n  {}",
                    table,
                    synced.join(", ")
                )
                .yellow()
            );
            if crate::util::is_interactive() {
                use std::io::Write;
                eprint!("Proceed and lose this data? [y/N] ");
                std::io::stderr().flush().unwrap();
                let mut input = String::new();
                std::io::stdin().read_line(&mut input).unwrap();
                if !input.trim().eq_ignore_ascii_case("y") {
                    eprintln!("{}", "Aborted.".red());
                    std::process::exit(1);
                }
            } else {
                eprintln!(
                    "{}",
                    "error: cannot auto-declare table in non-interactive mode — existing data would be lost. \
                     Declare all tables up front with 'databases create --table <name>'."
                        .red()
                );
                std::process::exit(1);
            }
        }

        let (del_status, del_body) = api
            .delete_raw(&format!("/databases/{}", db.id))
            .unwrap_or_else(|e| e.exit());
        if !del_status.is_success() {
            eprintln!("{}", crate::util::api_error(del_body).red());
            std::process::exit(1);
        }
        let create_body = create_database_request(
            db.name.as_deref(),
            db.default_catalog.as_deref(),
            schema,
            &all_tables,
            db.expires_at.as_deref(),
        );
        let (create_status, create_body_resp) = api
            .post_raw("/databases", &create_body)
            .unwrap_or_else(|e| e.exit());
        if !create_status.is_success() {
            eprintln!("{}", crate::util::api_error(create_body_resp).red());
            std::process::exit(1);
        }
        let new_db: CreateDatabaseResponse = match serde_json::from_str(&create_body_resp) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("error parsing create response: {e}");
                std::process::exit(1);
            }
        };
        let _ = crate::config::save_current_database("default", workspace_id, &new_db.id);
        // Managed databases have no add-table endpoint, so declaring a new table
        // is a delete + recreate — which mints a NEW database id. Surface that
        // explicitly: the id printed by `databases create` is now stale, and
        // id-based automation (e.g. `databases delete <create-time-id>`) would
        // otherwise fail with "no database with id". Reference by catalog instead.
        {
            use crossterm::style::Stylize;
            let catalog = db
                .default_catalog
                .as_deref()
                .or(db.name.as_deref())
                .unwrap_or(&db.id);
            eprintln!(
                "{}",
                format!(
                    "note: table '{table}' was not declared — recreated database '{catalog}' to add it \
                     (id {} → {}). Managed databases are recreated when a new table is loaded; \
                     reference them by catalog ('{catalog}'), not the create-time id.",
                    db.id, new_db.id
                )
                .yellow()
            );
        }
        let new_path = managed_table_load_path(&new_db.default_connection_id, schema, table);
        let spinner = crate::util::spinner("Loading table...");
        let result = api.post_raw(&new_path, &body).unwrap_or_else(|e| {
            spinner.finish_and_clear();
            e.exit()
        });
        spinner.finish_and_clear();
        result
    } else {
        (status, resp_body)
    };

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let result: LoadManagedTableResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    let catalog = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or("default");
    let full_name = format!("{catalog}.{}.{}", result.schema_name, result.table_name);
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

/// Load path for a database API token: addresses the database by id and uses
/// the database-scoped endpoints (the connection-scoped managed paths and the
/// name/catalog resolve used by the standard flow are denied for this token).
fn tables_load_database_scoped(
    workspace_id: &str,
    database: Option<&str>,
    table: &str,
    schema: Option<&str>,
    file: Option<&str>,
    url: Option<&str>,
    upload_id: Option<&str>,
) {
    use crossterm::style::Stylize;

    let api = Api::new(Some(workspace_id));
    let schema = schema_name(schema);

    // A database API token can't resolve names/catalog aliases (list/get are
    // denied), so address by id: an explicitly-supplied database id, else the
    // current-database context.
    let db_id = database
        .filter(|d| d.starts_with("dbid"))
        .map(str::to_string)
        .or_else(|| crate::config::load_current_database("default", workspace_id))
        .unwrap_or_else(|| {
            eprintln!(
                "{}",
                "error: a database id is required for a database API token. Pass the database \
                 id, or set one with 'hotdata databases set <dbid…>'."
                    .red()
            );
            std::process::exit(1);
        });

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

    let load_path = database_table_load_path(&db_id, schema, table);
    let body = load_table_request(&upload_id);

    let load = || {
        let spinner = crate::util::spinner("Loading table...");
        let r = api.post_raw(&load_path, &body).unwrap_or_else(|e| {
            spinner.finish_and_clear();
            e.exit()
        });
        spinner.finish_and_clear();
        r
    };

    let (mut status, mut resp_body) = load();

    // Auto-declare the table (database-scoped) when it wasn't declared at create
    // time, then retry — the db-scoped equivalent of the standard flow's declare
    // step, with no delete/recreate (that endpoint is denied for this token).
    if !status.is_success() && crate::util::api_error(resp_body.clone()).contains("not declared") {
        declare_database_table(&api, &db_id, schema, table);
        let (s, b) = load();
        status = s;
        resp_body = b;
    }

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let result: LoadManagedTableResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    // Display catalog: the supplied alias when it wasn't an id, else the default.
    let catalog = database
        .filter(|d| !d.starts_with("dbid"))
        .unwrap_or("default");
    let full_name = format!("{catalog}.{}.{}", result.schema_name, result.table_name);
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

/// Declare a table on a database's default catalog via the database-scoped
/// endpoints (what a database API token may call). Creates the schema with the
/// table when the schema doesn't exist; if it already exists (409), declares
/// just the table. Treats "already exists" as success.
fn declare_database_table(api: &Api, db_id: &str, schema: &str, table: &str) {
    use crossterm::style::Stylize;

    let (status, body) = api
        .post_raw(
            &database_schemas_path(db_id),
            &serde_json::json!({"name": schema, "tables": [{"name": table}]}),
        )
        .unwrap_or_else(|e| e.exit());
    if status.is_success() {
        return;
    }
    if status == reqwest::StatusCode::CONFLICT {
        // Schema already exists — declare just the table on it.
        let (table_status, table_body) = api
            .post_raw(
                &database_schema_tables_path(db_id, schema),
                &serde_json::json!({"name": table}),
            )
            .unwrap_or_else(|e| e.exit());
        if table_status.is_success() || table_status == reqwest::StatusCode::CONFLICT {
            return;
        }
        eprintln!("{}", crate::util::api_error(table_body).red());
        std::process::exit(1);
    }
    eprintln!("{}", crate::util::api_error(body).red());
    std::process::exit(1);
}

pub fn tables_delete(
    workspace_id: &str,
    database: Option<&str>,
    table: &str,
    schema: Option<&str>,
) {
    use crossterm::style::Stylize;

    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let schema = schema_name(schema);

    let path = managed_table_delete_path(&db.default_connection_id, schema, table);
    let (status, resp_body) = api.delete_raw(&path).unwrap_or_else(|e| e.exit());

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let catalog = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or("default");
    println!(
        "{}",
        format!("Table '{catalog}.{schema}.{table}' deleted.").green()
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
    fn parse_attach_spec_bare_connection_has_no_alias() {
        assert_eq!(parse_attach_spec("github"), ("github", None));
    }

    #[test]
    fn parse_attach_spec_splits_alias() {
        assert_eq!(parse_attach_spec("github=gh"), ("github", Some("gh")));
    }

    #[test]
    fn parse_attach_spec_trims_whitespace() {
        assert_eq!(
            parse_attach_spec("  salesdb = sales "),
            ("salesdb", Some("sales"))
        );
    }

    #[test]
    fn database_scoped_table_paths_address_by_database_id() {
        // The database-scoped endpoints a database API token uses — addressed by
        // database id, not connection id.
        assert_eq!(
            database_table_load_path("dbid_1", "public", "t"),
            "/databases/dbid_1/schemas/public/tables/t/loads"
        );
        assert_eq!(database_schemas_path("dbid_1"), "/databases/dbid_1/schemas");
        assert_eq!(
            database_schema_tables_path("dbid_1", "public"),
            "/databases/dbid_1/schemas/public/tables"
        );
    }

    #[test]
    fn create_database_request_empty_without_name_or_tables() {
        let req = create_database_request(None, None, "public", &[], None);
        assert_eq!(req, serde_json::json!({}));
    }

    #[test]
    fn create_database_request_includes_name() {
        let req = create_database_request(Some("jaffle_shop"), None, "public", &[], None);
        assert_eq!(req["name"], "jaffle_shop");
        assert!(req.get("schemas").is_none());
    }

    #[test]
    fn create_database_request_includes_schemas_when_tables_declared() {
        let req = create_database_request(
            None,
            None,
            "public",
            &["orders".to_string(), "customers".to_string()],
            None,
        );
        assert_eq!(req["schemas"][0]["name"], "public");
        assert_eq!(req["schemas"][0]["tables"][0]["name"], "orders");
        assert_eq!(req["schemas"][0]["tables"][1]["name"], "customers");
    }

    #[test]
    fn create_database_request_schemas_without_name() {
        let req = create_database_request(None, None, "analytics", &["events".to_string()], None);
        assert!(req.get("name").is_none());
        assert_eq!(req["schemas"][0]["name"], "analytics");
    }

    #[test]
    fn create_database_request_includes_expires_at_when_provided() {
        let req = create_database_request(None, None, "public", &[], Some("24h"));
        assert_eq!(req["expires_at"], "24h");
    }

    #[test]
    fn create_database_request_omits_expires_at_when_none() {
        let req = create_database_request(None, None, "public", &[], None);
        assert!(req.get("expires_at").is_none());
    }

    #[test]
    fn create_database_request_dot_notation_groups_tables_by_schema() {
        let req = create_database_request(
            None,
            None,
            "public",
            &[
                "orders".to_string(),
                "raw.raw_orders".to_string(),
                "raw.raw_customers".to_string(),
            ],
            None,
        );
        // bare "orders" → default schema "public"
        assert_eq!(req["schemas"][0]["name"], "public");
        assert_eq!(req["schemas"][0]["tables"][0]["name"], "orders");
        // dot-notation entries → "raw" schema, table name is the part after the dot
        assert_eq!(req["schemas"][1]["name"], "raw");
        assert_eq!(req["schemas"][1]["tables"][0]["name"], "raw_orders");
        assert_eq!(req["schemas"][1]["tables"][1]["name"], "raw_customers");
    }

    fn full_detail(id: &str, name: &str, conn_id: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"{name}","default_catalog":"default","default_connection_id":"{conn_id}","attachments":[]}}"#
        )
    }

    #[test]
    fn resolve_database_by_id_and_name() {
        let mut server = mockito::Server::new();
        // by-id path: direct GET /databases/db_abc succeeds
        let by_id_mock = server
            .mock("GET", "/v1/databases/db_abc")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_abc", "sales", "conn_1"))
            .create();
        // by-name path: GET /databases/warehouse → 404, then list, then detail
        let not_id = server
            .mock("GET", "/v1/databases/warehouse")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        let list = server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"db_abc","name":"sales","default_catalog":"db_abc"},{"id":"db_xyz","name":"warehouse","default_catalog":"db_xyz"}]}"#,
            )
            .create();
        let detail = server
            .mock("GET", "/v1/databases/db_xyz")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_xyz", "warehouse", "conn_2"))
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let by_id = resolve_database(&api, "db_abc");
        assert_eq!(by_id.default_connection_id, "conn_1");
        let by_name = resolve_database(&api, "warehouse");
        assert_eq!(by_name.id, "db_xyz");
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
            .mock("GET", "/v1/databases/missing")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        // List also returns nothing
        server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"databases":[]}"#)
            .create();

        let api = Api::test_new(&server.url(), "k", None);
        let err = try_resolve_database(&api, "missing").unwrap_err();
        assert!(err.contains("no database with id"));
    }

    #[test]
    fn try_resolve_database_rejects_ambiguous_name() {
        let mut server = mockito::Server::new();
        // Direct id lookup returns 404 (name isn't a valid id)
        server
            .mock("GET", "/v1/databases/sales")
            .with_status(404)
            .with_body(r#"{"error":"not found"}"#)
            .create();
        // List returns two entries with the same name
        server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"db_1","name":"sales","default_catalog":"db_1"},{"id":"db_2","name":"sales","default_catalog":"db_2"}]}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", None);
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
        let rows = table_rows(
            "default",
            vec![InfoTable {
                connection: "ignored".into(),
                schema: "public".into(),
                table: "orders".into(),
                synced: true,
                last_sync: Some("2026-05-19T00:00:00Z".into()),
            }],
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].full_name, "default.public.orders");
        assert!(rows[0].synced);
    }

    #[test]
    fn collect_tables_follows_cursor() {
        let mut server = mockito::Server::new();
        let page1 = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("connection_id".into(), "conn1".into()),
                mockito::Matcher::UrlEncoded("cursor".into(), "cur2".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":1000,"tables":[{"connection":"default","schema":"public","table":"b","synced":true,"last_sync":null}],"has_more":false,"next_cursor":null}"#,
            )
            .create();
        let page0 = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::UrlEncoded(
                "connection_id".into(),
                "conn1".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":1000,"tables":[{"connection":"default","schema":"public","table":"a","synced":false,"last_sync":null}],"has_more":true,"next_cursor":"cur2"}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
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
            .mock("POST", "/v1/databases")
            .match_header("X-Workspace-Id", "ws-test")
            .with_status(201)
            .with_body(r#"{"id":"db_new","name":"mydb","default_connection_id":"conn_abc"}"#)
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&create_database_request(
                    Some("mydb"),
                    None,
                    "public",
                    &["gdp".to_string()],
                    None,
                ))
                .unwrap(),
            ))
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws-test"));
        let body =
            create_database_request(Some("mydb"), None, "public", &["gdp".to_string()], None);
        let (status, resp_body) = api.post_raw("/databases", &body).unwrap();
        assert_eq!(status.as_u16(), 201);
        let parsed: CreateDatabaseResponse = serde_json::from_str(&resp_body).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("mydb"));
        assert_eq!(parsed.default_connection_id, "conn_abc");
        mock.assert();
    }

    #[test]
    fn tables_load_uses_default_connection_id() {
        let mut server = mockito::Server::new();
        // resolve_database resolves by id directly
        let resolve = server
            .mock("GET", "/v1/databases/db_1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_1", "sales", "conn_default"))
            .create();
        let load = server
            .mock(
                "POST",
                "/v1/connections/conn_default/schemas/public/tables/orders/loads",
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

        let api = Api::test_new(&server.url(), "k", Some("ws1"));
        let db = resolve_database(&api, "db_1");
        let path = managed_table_load_path(&db.default_connection_id, "public", "orders");
        let body = load_table_request("upl_123");
        let (status, resp_body) = api.post_raw(&path, &body).unwrap();
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
            .mock("GET", "/v1/databases/db_1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_1", "sales", "conn_default"))
            .create();
        let delete = server
            .mock(
                "DELETE",
                "/v1/connections/conn_default/schemas/public/tables/orders",
            )
            .with_status(204)
            .with_body("")
            .create();

        let api = Api::test_new(&server.url(), "k", None);
        let db = resolve_database(&api, "db_1");
        let path = managed_table_delete_path(&db.default_connection_id, "public", "orders");
        let (status, _) = api.delete_raw(&path).unwrap();
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

    #[test]
    fn database_token_response_deserializes() {
        let body = r#"{"ok":true,"token":"jwt-x","refresh_token":"rt-x","database_id":"dbid_abc","expires_in":300,"refresh_expires_in":259200}"#;
        let resp: DatabaseTokenResponse = serde_json::from_str(body).unwrap();
        assert_eq!(resp.token, "jwt-x");
        assert_eq!(resp.database_id, "dbid_abc");
        assert_eq!(resp.refresh_token, "rt-x");
        assert_eq!(resp.expires_in, 300);
    }

    #[test]
    fn create_and_return_id_parses_id() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/databases")
            .match_body(mockito::Matcher::Json(create_database_request(
                Some("scratch"),
                None,
                "public",
                &[],
                None,
            )))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"dbid_new","name":"scratch","default_catalog":"default","default_connection_id":"conn_1"}"#,
            )
            .create();
        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let id = create_and_return_id(&api, Some("scratch"), "public", &[], None);
        m.assert();
        assert_eq!(id, "dbid_new");
    }

    #[test]
    fn mint_database_token_posts_existing_database_grant() {
        let mut server = mockito::Server::new();
        let m = server
            .mock("POST", "/v1/auth/database")
            .match_body(mockito::Matcher::JsonString(
                r#"{"grant_type":"existing_database","database_id":"dbid_abc"}"#.to_string(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"ok":true,"token":"jwt-x","refresh_token":"rt-x","database_id":"dbid_abc","expires_in":300,"refresh_expires_in":259200}"#)
            .create();
        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let resp = mint_database_token(&api, "dbid_abc");
        m.assert();
        assert_eq!(resp.token, "jwt-x");
        assert_eq!(resp.database_id, "dbid_abc");
    }

    // The `--url` path downloads to a temp file and then exits via
    // `process::exit` if the upload fails. Because `process::exit` runs no
    // destructors, the temp file must be deleted by `upload_temp_file` before
    // it returns the `Err` the caller exits on — not by a guard in the caller's
    // scope. These tests pin that contract for both arms.
    #[test]
    fn upload_temp_file_removes_temp_on_upload_failure() {
        let temp = tempfile::Builder::new()
            .suffix(".parquet")
            .tempfile()
            .unwrap();
        let path = temp.path().to_path_buf();
        assert!(path.exists());

        let result = upload_temp_file(temp, |p| {
            assert!(p.exists(), "file present while the upload runs");
            Err(ApiError::Transport("upload boom".into()))
        });

        assert!(result.is_err());
        assert!(
            !path.exists(),
            "temp file must be removed before the failure is returned (caller exits without unwinding)"
        );
    }

    #[test]
    fn upload_temp_file_removes_temp_on_success() {
        let temp = tempfile::Builder::new()
            .suffix(".parquet")
            .tempfile()
            .unwrap();
        let path = temp.path().to_path_buf();

        let result = upload_temp_file(temp, |_p| Ok("upid_123".to_string()));

        assert_eq!(result.unwrap(), "upid_123");
        assert!(!path.exists(), "temp file must be removed on success");
    }
}
