use crate::client::sdk::{Api, ApiError, block, block_with_wakeup, none_if_404};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Subcommands for `hotdata databases`.
#[derive(clap::Subcommand)]
pub enum DatabasesCommands {
    /// List managed databases in the workspace
    List {
        /// Maximum number of databases to return
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination cursor from a previous response
        #[arg(long)]
        cursor: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Show details for a specific managed database
    Show {
        /// Database name or ID
        name_or_id: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Create a new managed database
    Create {
        /// Human-readable display name for the database (e.g. "Sales reporting").
        #[arg(long)]
        name: Option<String>,

        /// SQL catalog alias used in queries: SELECT … FROM <catalog>.schema.table.
        /// Must be [a-z_][a-z0-9_]*, globally unique.
        #[arg(long)]
        catalog: Option<String>,

        /// Default schema for bare `--table` entries (default: public).
        /// Use dot notation in `--table` to target a different schema directly,
        /// e.g. `--table raw.raw_orders` always goes into the "raw" schema.
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table to declare up front (repeatable). Accepts bare names or
        /// `schema.table` dot notation to span multiple schemas in one command:
        ///   --table orders --table raw.raw_orders --table raw.raw_customers
        #[arg(long = "table")]
        tables: Vec<String>,

        /// When the database expires. Accepts a relative duration (e.g. 24h, 7d, 90m)
        /// or an RFC 3339 timestamp. Omitting means no expiry.
        #[arg(long)]
        expires_at: Option<String>,

        /// Attach a connection as a queryable catalog on the new database (repeatable).
        /// Accepts a connection name or id, optionally `connection=alias` to set the
        /// SQL alias it answers to: `--attach github --attach salesdb=sales`.
        #[arg(long = "attach")]
        attach: Vec<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Fork a managed database into a new, independent database.
    ///
    /// The fork contains the same schemas, tables, and data as the source, and
    /// answers to the same SQL catalog alias inside its own query scope; the
    /// two databases diverge freely afterwards (writes to one never affect the
    /// other). Connection catalogs attached to the source are re-attached to
    /// the fork; indexes are not carried over.
    Fork {
        /// Source database id, catalog, or name (defaults to the current database)
        database: Option<String>,

        /// Display name for the fork. Defaults to "<source-name>-fork" so the
        /// two databases stay distinguishable in `databases list`.
        #[arg(long)]
        name: Option<String>,

        /// When the fork expires. Accepts a relative duration (e.g. 24h, 7d, 90m)
        /// or an RFC 3339 timestamp. When omitted, a still-future expiry on the
        /// source is carried over; otherwise the fork never expires.
        #[arg(long)]
        expires_at: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Attach a connection as a queryable catalog on a managed database.
    ///
    /// A `query` runs inside one managed database; attaching a connection makes
    /// its live tables visible in that database's scope, so you can join across
    /// sources in a single query without exporting data. Reachable in SQL as
    /// `<alias>.<schema>.<table>`, or `<connection-name>.<schema>.<table>` when
    /// `--alias` is omitted.
    Attach {
        /// Connection name or id to attach (e.g. `github`)
        connection: String,

        /// Database id, catalog, or name to attach into (defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Alias the catalog answers to in SQL. Defaults to the connection's name.
        #[arg(long)]
        alias: Option<String>,
    },

    /// Detach a previously attached connection catalog from a managed database.
    Detach {
        /// Connection name or id to detach
        connection: String,

        /// Database id, catalog, or name to detach from (defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,
    },

    /// Set the current database (used by default when no database is specified)
    Set {
        /// Database id
        id: String,
    },

    /// Clear the current database
    Unset,

    /// Delete a managed database and its tables
    Delete {
        /// Database name or connection ID
        name_or_id: String,
    },

    /// Load a parquet file or a saved query result into a managed database table
    Load {
        /// SQL catalog alias of the target database (e.g. `--catalog airbnb`)
        #[arg(long)]
        catalog: String,

        /// Schema to load into (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table name to load into
        #[arg(long)]
        table: String,

        /// Path to a local parquet file to upload and load
        #[arg(long, conflicts_with_all = ["upload_id", "url", "result_id"])]
        file: Option<String>,

        /// URL of a remote parquet file to download and load
        #[arg(long, conflicts_with_all = ["file", "upload_id", "result_id"])]
        url: Option<String>,

        /// Use a previously staged upload ID from `POST /v1/uploads` instead of uploading
        #[arg(long, conflicts_with_all = ["file", "url", "result_id"])]
        upload_id: Option<String>,

        /// Load a saved query result by id (e.g. `--result-id rslt…`, from
        /// `hotdata results` or a query's `[result-id: …]` footer) instead of a
        /// file. The result must belong to the target database — the one it was
        /// queried in.
        #[arg(long, conflicts_with_all = ["file", "url", "upload_id"])]
        result_id: Option<String>,
    },

    /// Manage tables inside a managed database
    Tables {
        /// Database id or name — shorthand for `tables list` when no subcommand is given
        database: Option<String>,

        #[command(subcommand)]
        command: Option<DatabaseTablesCommands>,
    },

    /// Run a command with a database-scoped token. Creates a new database unless --database is given.
    Run {
        /// Existing database id to scope the token to (omit to auto-create a database)
        #[arg(long)]
        database: Option<String>,

        /// Name for the auto-created database (only used when --database is omitted)
        #[arg(long)]
        name: Option<String>,

        /// Schema for tables declared in the auto-created database (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Table to declare in the auto-created database (repeatable)
        #[arg(long = "table")]
        tables: Vec<String>,

        /// When the auto-created database expires. Accepts a relative duration
        /// (e.g. 24h, 7d, 90m) or an RFC 3339 timestamp. Defaults to 24h when omitted.
        #[arg(long)]
        expires_at: Option<String>,

        /// Command to execute (everything after `--`)
        #[arg(trailing_var_arg = true, required = true)]
        cmd: Vec<String>,
    },
}

/// Subcommands for `hotdata databases tables`.
#[derive(clap::Subcommand)]
pub enum DatabaseTablesCommands {
    /// List tables in a managed database
    List {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Filter by schema name
        #[arg(long)]
        schema: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Load a parquet file or a saved query result into a table (creates or replaces the table)
    Load {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Table name
        table: String,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,

        /// Path to a local parquet file to upload and load
        #[arg(long, conflicts_with_all = ["upload_id", "url", "result_id"])]
        file: Option<String>,

        /// URL of a remote parquet file to download and load
        #[arg(long, conflicts_with_all = ["file", "upload_id", "result_id"])]
        url: Option<String>,

        /// Use a previously staged upload ID from `POST /v1/uploads` instead of uploading
        #[arg(long, conflicts_with_all = ["file", "url", "result_id"])]
        upload_id: Option<String>,

        /// Load a saved query result by id (e.g. `--result-id rslt…`, from
        /// `hotdata results` or a query's `[result-id: …]` footer) instead of a
        /// file. The result must belong to the target database — the one it was
        /// queried in.
        #[arg(long, conflicts_with_all = ["file", "url", "upload_id"])]
        result_id: Option<String>,
    },

    /// Delete a table from a managed database
    Delete {
        /// Database id or name (defaults to current database)
        #[arg(long)]
        database: Option<String>,

        /// Table name
        table: String,

        /// Schema name (default: public)
        #[arg(long, default_value = "public")]
        schema: String,
    },
}

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
    #[serde(default)]
    created_at: Option<String>,
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
    pub created_at: Option<String>,
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
            created_at: d.created_at.flatten(),
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
    fn from(s: hotdata::models::DatabaseSummary) -> Self {
        DatabaseSummary {
            id: s.id,
            name: s.name.flatten(),
            default_catalog: Some(s.default_catalog),
            created_at: s.created_at.flatten(),
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

/// Drain every page of the paginated databases list into the full set of
/// summary rows.
///
/// `GET /v1/databases` returns one capped page plus a `next_cursor`; the CLI
/// shows and resolves against the whole workspace, so follow the cursor to the
/// end. When `spinner` is `Some`, the first page is fetched via
/// [`block_with_wakeup`] so a cold KEDA start surfaces a "waking up worker"
/// hint; the remaining pages (and the spinner-less id-only caller) use
/// [`block`].
fn list_all_databases(
    api: &Api,
    spinner: Option<&str>,
) -> Result<Vec<hotdata::models::DatabaseSummary>, ApiError> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    let mut first = true;
    loop {
        let resp = match (first, spinner) {
            (true, Some(msg)) => block_with_wakeup(
                api,
                msg,
                api.client().databases().list(None, cursor.as_deref()),
            )?,
            _ => block(api.client().databases().list(None, cursor.as_deref()))?,
        };
        first = false;
        // Move the page rows out first (partial move), then read next_cursor —
        // no clone needed.
        all.extend(resp.databases);
        let next = resp.next_cursor.flatten();
        // Stop at the last page, or if the server ever returns a non-advancing
        // cursor (defensive — never loop forever).
        match next {
            Some(c) if !c.is_empty() && Some(&c) != cursor.as_ref() => cursor = Some(c),
            _ => break,
        }
    }
    Ok(all)
}

/// List databases, mapped into the CLI's summary rows. Drains all pages so
/// `databases list` shows the whole workspace and name/catalog resolution sees
/// every database (see [`list_all_databases`]).
fn list_database_summaries(api: &Api) -> Result<Vec<DatabaseSummary>, ApiError> {
    list_all_databases(api, Some("Loading databases..."))
        .map(|dbs| dbs.into_iter().map(DatabaseSummary::from).collect())
}

/// List the ids of every managed database in the workspace.
///
/// Exposed for the whole-workspace `indexes list` scan (#168): a managed
/// database's connection is hidden from `connections list` and its tables are
/// absent from the unscoped `information_schema` enumeration, so that scan
/// rediscovers managed databases here and resolves each one's
/// `default_connection_id` via [`get_database`]. The list summary omits the
/// connection id, hence ids only.
///
/// Deliberately spinner-less (the caller owns its own "Loading indexes…"
/// spinner, and two indicatif bars would fight over the same line).
pub(crate) fn list_database_ids(api: &Api) -> Result<Vec<String>, ApiError> {
    list_all_databases(api, None).map(|dbs| dbs.into_iter().map(|d| d.id).collect())
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

/// Build the request body for `POST /v1/databases/{id}/fork`.
///
/// When `--name` is omitted the CLI defaults it to `<source-label>-fork`
/// rather than letting the server carry the source's name over: an unnamed
/// fork inherits the source's display name AND catalog alias, which would
/// leave two rows in `databases list` identical in every user-facing column.
/// `source_label` is the source's name (or catalog as a fallback).
pub fn fork_database_request(
    name: Option<&str>,
    source_label: Option<&str>,
    expires_at: Option<&str>,
) -> hotdata::models::ForkDatabaseRequest {
    let name = name
        .map(str::to_string)
        .or_else(|| source_label.map(|l| format!("{l}-fork")));

    // The SDK models both fields as double-option: `None` omits the field
    // (server applies its default — inherit the source's name/expiry), while
    // `Some(Some(v))` sends the value. We never send an explicit null.
    hotdata::models::ForkDatabaseRequest {
        name: name.map(Some),
        expires_at: expires_at.map(|e| Some(e.to_string())),
    }
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

pub fn load_table_request_from_result(result_id: &str) -> serde_json::Value {
    serde_json::json!({
        "mode": "replace",
        "result_id": result_id,
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

fn collect_tables(
    api: &Api,
    connection_id: &str,
    schema: Option<&str>,
    table: Option<&str>,
    limit: Option<u32>,
    cursor: Option<&str>,
) -> (Vec<InfoTable>, bool, Option<String>) {
    // When limit/cursor are provided do a single paged fetch; otherwise exhaust all pages.
    if limit.is_some() || cursor.is_some() {
        let resp = crate::client::sdk::block(api.client().information_schema().get(
            Some(connection_id),
            schema,
            table,
            None,
            limit.map(|l| l as i32),
            cursor,
        ))
        .unwrap_or_else(|e| e.exit());
        let mut out: Vec<InfoTable> = resp
            .tables
            .into_iter()
            .map(|t| InfoTable {
                connection: t.connection,
                schema: t.schema,
                table: t.table,
                synced: t.synced,
                last_sync: t.last_sync.flatten(),
            })
            .collect();
        out.sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.table.cmp(&b.table)));
        return (out, resp.has_more, resp.next_cursor.flatten());
    }

    let mut out = Vec::new();
    let mut page_cursor: Option<String> = None;
    loop {
        let resp = crate::client::sdk::block(api.client().information_schema().get(
            Some(connection_id),
            schema,
            table,
            None,
            None,
            page_cursor.as_deref(),
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
        page_cursor = Some(c);
    }
    out.sort_by(|a, b| a.schema.cmp(&b.schema).then_with(|| a.table.cmp(&b.table)));
    (out, false, None)
}

pub fn list(workspace_id: &str, format: &str, limit: Option<u32>, cursor: Option<&str>) {
    let api = Api::new(Some(workspace_id));
    // One page, user-paginated (like `tables list` / `queries list`). Name/
    // catalog resolution and the whole-workspace `indexes list` scan still see
    // every database — they drain all pages internally via list_all_databases;
    // only this display command pages.
    let resp = block_with_wakeup(
        &api,
        "Loading databases...",
        api.client()
            .databases()
            .list(limit.map(|l| l as i32), cursor),
    )
    .unwrap_or_else(|e| e.exit());
    let has_more = resp.has_more.flatten().unwrap_or(false);
    // Take next_cursor before moving the rows out below — no clone needed.
    let next_cursor = resp.next_cursor.flatten();
    // Server returns newest-first; keep that order so the cursor continuation is
    // coherent (no client re-sort across pages).
    let databases: Vec<DatabaseSummary> = resp
        .databases
        .into_iter()
        .map(DatabaseSummary::from)
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
                            d.created_at
                                .as_deref()
                                .map(crate::util::format_date)
                                .unwrap_or_else(|| "-".to_string()),
                        ]
                    })
                    .collect();
                crate::output::table::print(&["DEFAULT", "ID", "NAME", "CREATED"], &rows);
            }
        }
        _ => unreachable!(),
    }

    if has_more {
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!(
                "More results available. Use --cursor {} to fetch the next page.",
                next_cursor.as_deref().unwrap_or("")
            )
            .dark_grey()
        );
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
            if let Some(ts) = &db.created_at {
                println!(
                    "{}{}",
                    label("created_at:"),
                    crate::util::format_date(ts).dark_grey()
                );
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
                format!("{catalog}.{{schema}}.{{table}}").green()
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
    let connection_id = crate::commands::connections::resolve_connection_id(&api, connection);
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
        .unwrap_or_else(|| crate::commands::connections::resolve_connection_id(&api, connection));

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
        crate::client::sdk::ApiError::Status {
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
    let session = crate::client::database_session::DatabaseSession {
        access_token: db_jwt.clone(),
        refresh_token: db_refresh.clone(),
        database_id: db_id.clone(),
        workspace_id: workspace_id.to_string(),
        access_expires_at: now + resp.expires_in,
        refresh_expires_at: now + resp.refresh_expires_in,
    };
    if let Err(e) = crate::client::database_session::save(&session) {
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
    let connection_id = crate::commands::connections::try_resolve_connection_id(api, connection)?;
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

/// Print a fork failure and exit. Preserves the server's message verbatim and,
/// for the parquet-backed-source 400, appends a recovery hint. Non-status
/// (transport) errors fall through to the shared [`ApiError::exit`] path.
fn fork_error_exit(e: ApiError) -> ! {
    use crossterm::style::Stylize;
    if let ApiError::Status { ref body, .. } = e {
        let msg = crate::util::api_error(body.clone());
        eprintln!("{}", msg.clone().red());
        // Pre-DuckLake (parquet-backed) databases can't be forked; recreating
        // the database and reloading its data is the migration path.
        if msg.contains("storage backend") || msg.contains("DuckLake") {
            eprintln!(
                "{}",
                "hint: databases created before DuckLake storage can't be forked — \
                 recreate the database and reload its data to get a forkable one."
                    .dark_grey()
            );
        }
        std::process::exit(1);
    }
    e.exit()
}

pub fn fork(
    workspace_id: &str,
    database: Option<&str>,
    name: Option<&str>,
    expires_at: Option<&str>,
    format: &str,
) {
    use crossterm::style::Stylize;

    let source = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &source);

    let request = fork_database_request(
        name,
        db.name.as_deref().or(db.default_catalog.as_deref()),
        expires_at,
    );

    // The copy is synchronous but bucket-internal (server-side object copy, no
    // bytes through the pod), so it rides the default request timeout. Routed
    // through the same typed `databases()` handle as create/get/list/delete.
    let resp = if format == "table" {
        block_with_wakeup(
            &api,
            "Forking database...",
            api.client().databases().fork(&db.id, request),
        )
    } else {
        block(api.client().databases().fork(&db.id, request))
    }
    .unwrap_or_else(|e| fork_error_exit(e));

    let result = CreateDatabaseResponse {
        id: resp.id,
        name: resp.name.flatten(),
        default_catalog: Some(resp.default_catalog),
        default_connection_id: resp.default_connection_id,
        expires_at: resp.expires_at.flatten(),
    };

    if let Err(e) = crate::config::save_current_database("default", workspace_id, &result.id) {
        eprintln!(
            "{}",
            format!("warning: database forked but could not set as current: {e}").yellow()
        );
    }

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&result).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&result).unwrap()),
        "table" => {
            println!("{}", "Database forked".green());
            if let Some(n) = &result.name {
                println!("name:        {}", n.clone().cyan());
            }
            if let Some(c) = &result.default_catalog {
                println!("catalog:     {}", c.clone().cyan());
            }
            println!("id:          {}", result.id);
            println!("forked_from: {}", db.id);
            // Always printed, "never" included: with --expires-at omitted a
            // still-future expiry on the source is silently inherited.
            println!(
                "expires_at:  {}",
                result.expires_at.as_deref().unwrap_or("never")
            );
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
                        "The fork is now the current database; the source is unchanged.\n",
                        "It answers to the same catalog alias as its source inside its own scope.\n",
                        "Indexes are not carried over — recreate them on the fork if needed.\n",
                        "\nQuery it now:\n",
                        "  hotdata query \"SELECT * FROM {0}.public.<table> LIMIT 10\"",
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
        .and_then(|profile| crate::client::credentials::api_key_jwt_source(&profile))
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

pub fn tables_list(
    workspace_id: &str,
    database: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    limit: Option<u32>,
    cursor: Option<&str>,
    format: &str,
) {
    let database = resolve_current_database(database, workspace_id);
    let api = Api::new(Some(workspace_id));
    let db = resolve_database(&api, &database);
    let catalog = db
        .default_catalog
        .as_deref()
        .or(db.name.as_deref())
        .unwrap_or("default");
    let (tables, has_more, next_cursor) = collect_tables(
        &api,
        &db.default_connection_id,
        schema,
        table,
        limit,
        cursor,
    );

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
                crate::output::table::print(&["TABLE", "SYNCED", "LAST_SYNC"], &table_rows);
            }
        }
        _ => unreachable!(),
    }
    if has_more {
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!(
                "More results available. Use --cursor {} to fetch the next page.",
                next_cursor.as_deref().unwrap_or("")
            )
            .dark_grey()
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub fn tables_load(
    workspace_id: &str,
    database: Option<&str>,
    table: &str,
    schema: Option<&str>,
    file: Option<&str>,
    url: Option<&str>,
    upload_id: Option<&str>,
    result_id: Option<&str>,
) {
    use crossterm::style::Stylize;

    // A database API token can't resolve names/catalogs or use the
    // connection-scoped managed endpoints (all outside its allow-list). Route it
    // through the database-scoped endpoints, addressed by database id.
    if let Ok(profile) = crate::config::load("default")
        && crate::client::credentials::api_key_jwt_source(&profile).as_deref()
            == Some("database_api_token")
    {
        tables_load_database_scoped(
            workspace_id,
            database,
            table,
            schema,
            file,
            url,
            upload_id,
            result_id,
        );
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
    // A result load hits the connection-scoped endpoint, where the server scopes
    // the result by the X-Database-Id header; that must name the resolved target
    // database, not the ambient active one (which may be unset or different). An
    // upload isn't result-scoped, so it keeps the ambient scope.
    let api = api.scoped_to_database_opt(result_id.map(|_| db.id.as_str()));
    let schema = schema_name(schema);

    // clap enforces mutual exclusion; only one source is ever Some. A file/URL is
    // uploaded first and loaded by upload id; a result is loaded by reference with
    // no upload step.
    let body = match (result_id, upload_id, file, url) {
        (Some(rid), None, None, None) => load_table_request_from_result(rid),
        (None, Some(id), None, None) => load_table_request(id),
        (None, None, Some(path), None) => load_table_request(&upload_parquet_file(&api, path)),
        (None, None, None, Some(u)) => load_table_request(&upload_parquet_url(&api, u)),
        (None, None, None, None) => {
            eprintln!(
                "error: one of --file <path>, --url <url>, --upload-id <id>, or --result-id <id> is required"
            );
            std::process::exit(1);
        }
        _ => unreachable!(),
    };

    let path = managed_table_load_path(&db.default_connection_id, schema, table);

    let spinner = crate::util::spinner("Loading table...");
    let (status, resp_body) = api.post_raw(&path, &body).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();

    let (status, resp_body) = if !status.is_success()
        // Upload-only recovery: the delete + recreate below mints a new database
        // id, which would orphan a result (the server scopes it to the original
        // database). A result load into an undeclared table is auto-declared
        // server-side, so this path isn't needed for it — surface the error.
        && result_id.is_none()
        && crate::util::api_error(resp_body.clone()).contains("not declared")
    {
        // The table wasn't declared at create time. Collect existing tables so
        // they are re-declared in the replacement database, then delete and
        // recreate with all tables (including the new one) declared.
        let (existing, _, _) =
            collect_tables(&api, &db.default_connection_id, None, None, None, None);
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
#[allow(clippy::too_many_arguments)]
fn tables_load_database_scoped(
    workspace_id: &str,
    database: Option<&str>,
    table: &str,
    schema: Option<&str>,
    file: Option<&str>,
    url: Option<&str>,
    upload_id: Option<&str>,
    result_id: Option<&str>,
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

    // clap enforces mutual exclusion; only one source is ever Some. A file/URL is
    // uploaded first and loaded by upload id; a result is loaded by reference. The
    // database-scoped endpoint scopes the result by the path database id, so no
    // X-Database-Id header is needed here.
    let body = match (result_id, upload_id, file, url) {
        (Some(rid), None, None, None) => load_table_request_from_result(rid),
        (None, Some(id), None, None) => load_table_request(id),
        (None, None, Some(path), None) => load_table_request(&upload_parquet_file(&api, path)),
        (None, None, None, Some(u)) => load_table_request(&upload_parquet_url(&api, u)),
        (None, None, None, None) => {
            eprintln!(
                "error: one of --file <path>, --url <url>, --upload-id <id>, or --result-id <id> is required"
            );
            std::process::exit(1);
        }
        _ => unreachable!(),
    };

    let load_path = database_table_load_path(&db_id, schema, table);

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
            r#"{{"id":"{id}","name":"{name}","default_catalog":"default","default_schema":"main","default_connection_id":"{conn_id}","attachments":[]}}"#
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
                r#"{"databases":[{"id":"db_abc","name":"sales","default_catalog":"db_abc","default_schema":"main"},{"id":"db_xyz","name":"warehouse","default_catalog":"db_xyz","default_schema":"main"}]}"#,
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
                r#"{"databases":[{"id":"db_1","name":"sales","default_catalog":"db_1","default_schema":"main"},{"id":"db_2","name":"sales","default_catalog":"db_2","default_schema":"main"}]}"#,
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
    fn load_table_request_from_result_is_replace_mode() {
        let body = load_table_request_from_result("rslt_abc");
        assert_eq!(body["mode"], "replace");
        assert_eq!(body["result_id"], "rslt_abc");
        // A result load must not send an upload_id (the server rejects both).
        assert!(body.get("upload_id").is_none());
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
        let (tables, _, _) = collect_tables(&api, "conn1", None, None, None, None);
        page0.assert();
        page1.assert();
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].table, "a");
        assert_eq!(tables[1].table, "b");
    }

    #[test]
    fn list_all_databases_follows_cursor() {
        let mut server = mockito::Server::new();
        // Page 2 (reached via ?cursor=cur2): the last database, no next_cursor.
        let page2 = server
            .mock("GET", "/v1/databases")
            .match_query(mockito::Matcher::UrlEncoded("cursor".into(), "cur2".into()))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"db2","name":"second","default_catalog":"c2","default_schema":"main"}],"count":1,"limit":1,"has_more":false,"next_cursor":null}"#,
            )
            .create();
        // Page 1 (first request, no cursor): first database + a next_cursor.
        let page1 = server
            .mock("GET", "/v1/databases")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"databases":[{"id":"db1","name":"first","default_catalog":"c1","default_schema":"main"}],"count":1,"limit":1,"has_more":true,"next_cursor":"cur2"}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let dbs = list_all_databases(&api, None).unwrap();
        page1.assert();
        page2.assert();
        assert_eq!(dbs.len(), 2, "drain must reassemble both pages");
        assert_eq!(dbs[0].id, "db1");
        assert_eq!(dbs[1].id, "db2");
    }

    #[test]
    fn collect_tables_with_limit_makes_single_request_and_returns_cursor() {
        let mut server = mockito::Server::new();
        // Only one mock — verifies we do NOT follow the cursor when limit is set.
        let mock = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("connection_id".into(), "conn1".into()),
                mockito::Matcher::UrlEncoded("limit".into(), "5".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":5,"tables":[{"connection":"default","schema":"public","table":"a","synced":true,"last_sync":null}],"has_more":true,"next_cursor":"page2"}"#,
            )
            .expect(1)
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let (tables, has_more, next_cursor) =
            collect_tables(&api, "conn1", None, None, Some(5), None);
        mock.assert();
        assert_eq!(tables.len(), 1);
        assert!(has_more);
        assert_eq!(next_cursor.as_deref(), Some("page2"));
    }

    #[test]
    fn collect_tables_with_table_filter_passes_it_to_api() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("GET", "/v1/information_schema")
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("connection_id".into(), "conn1".into()),
                mockito::Matcher::UrlEncoded("table".into(), "orders".into()),
            ]))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"count":1,"limit":1000,"tables":[{"connection":"default","schema":"public","table":"orders","synced":true,"last_sync":null}],"has_more":false,"next_cursor":null}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws"));
        let (tables, _, _) = collect_tables(&api, "conn1", None, Some("orders"), None, None);
        mock.assert();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].table, "orders");
    }

    #[test]
    fn create_posts_to_databases_endpoint() {
        let mut server = mockito::Server::new();
        let mock = server
            .mock("POST", "/v1/databases")
            .match_header("X-Workspace-Id", "ws-test")
            .with_status(201)
            .with_body(r#"{"id":"db_new","name":"mydb","default_catalog":"default","default_schema":"main","default_connection_id":"conn_abc"}"#)
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
    fn fork_database_request_defaults_name_to_source_label_fork() {
        let to_json = |r| serde_json::to_value(&r).unwrap();
        // Explicit --name wins over the derived default.
        assert_eq!(
            to_json(fork_database_request(Some("my-fork"), Some("sales"), None)),
            serde_json::json!({"name": "my-fork"})
        );
        // Omitted --name derives "<source-label>-fork".
        assert_eq!(
            to_json(fork_database_request(None, Some("sales"), Some("24h"))),
            serde_json::json!({"name": "sales-fork", "expires_at": "24h"})
        );
        // No name anywhere: send nothing and let the server default.
        assert_eq!(
            to_json(fork_database_request(None, None, None)),
            serde_json::json!({})
        );
    }

    #[test]
    fn fork_posts_to_fork_endpoint_and_parses_create_response() {
        let mut server = mockito::Server::new();
        // resolve_database resolves by id directly
        let resolve = server
            .mock("GET", "/v1/databases/db_src")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_src", "sales", "conn_src"))
            .create();
        let fork = server
            .mock("POST", "/v1/databases/db_src/fork")
            .match_header("X-Workspace-Id", "ws1")
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&fork_database_request(None, Some("sales"), Some("1h")))
                    .unwrap(),
            ))
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(
                r#"{"id":"db_fork","name":"sales-fork","default_catalog":"default","default_connection_id":"conn_fork","default_schema":"main","expires_at":"2026-07-15T19:00:00Z"}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws1"));
        let db = resolve_database(&api, "db_src");
        let request = fork_database_request(None, db.name.as_deref(), Some("1h"));
        let resp = block(api.client().databases().fork(&db.id, request)).unwrap();
        assert_eq!(resp.id, "db_fork");
        assert_eq!(resp.name.flatten().as_deref(), Some("sales-fork"));
        assert_eq!(resp.default_connection_id, "conn_fork");
        assert_eq!(
            resp.expires_at.flatten().as_deref(),
            Some("2026-07-15T19:00:00Z")
        );
        resolve.assert();
        fork.assert();
    }

    #[test]
    fn fork_unforkable_source_surfaces_api_error_body() {
        let mut server = mockito::Server::new();
        let fork = server
            .mock("POST", "/v1/databases/db_old/fork")
            .with_status(400)
            .with_body(
                r#"{"error":{"code":"BAD_REQUEST","message":"forking a database is only supported for DuckLake-backed catalogs; source database 'db_old' uses storage backend 'parquet'"}}"#,
            )
            .create();

        let api = Api::test_new(&server.url(), "k", Some("ws1"));
        let err = block(
            api.client()
                .databases()
                .fork("db_old", fork_database_request(None, None, None)),
        )
        .expect_err("unforkable source should return an error");
        match err {
            ApiError::Status { status, body } => {
                assert_eq!(status.as_u16(), 400);
                assert!(
                    crate::util::api_error(body).contains("DuckLake-backed"),
                    "expected DuckLake message"
                );
            }
            other => panic!("expected Status error, got {other:?}"),
        }
        fork.assert();
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
    fn tables_load_from_result_posts_result_id_scoped_to_target() {
        let mut server = mockito::Server::new();
        let resolve = server
            .mock("GET", "/v1/databases/db_1")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(full_detail("db_1", "sales", "conn_default"))
            .create();
        // A result load hits the same loads endpoint with a result_id body and no
        // upload step (no POST /v1/uploads mock is registered). The server scopes
        // a result-sourced load by X-Database-Id, so the request must carry the
        // resolved target database id.
        let load = server
            .mock(
                "POST",
                "/v1/connections/conn_default/schemas/public/tables/orders/loads",
            )
            .match_header("X-Database-Id", "db_1")
            .match_body(mockito::Matcher::JsonString(
                serde_json::to_string(&load_table_request_from_result("rslt_123")).unwrap(),
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
        // Mirror tables_load: scope to the resolved target database so the load
        // carries X-Database-Id.
        let api = api.scoped_to_database_opt(Some(db.id.as_str()));
        let path = managed_table_load_path(&db.default_connection_id, "public", "orders");
        let body = load_table_request_from_result("rslt_123");
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
                r#"{"id":"dbid_new","name":"scratch","default_catalog":"default","default_schema":"main","default_connection_id":"conn_1"}"#,
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
