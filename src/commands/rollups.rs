//! `hotdata rollups` — rollups of a database's tables.
//!
//! A rollup keeps a table pre-aggregated by time; aggregate queries it can
//! answer read it instead of the table. The verbs map one to one onto the
//! rollups API under `/v1/databases/{database_id}/rollups`.
//!
//! Responses are read as untyped JSON rather than through the SDK's generated
//! models. `-o json|yaml` prints the API object as the server sent it, and a
//! status or measure kind a newer server adds is displayed instead of failing
//! a closed enum — which would take `list` and the build wait down with it.

use crate::client::sdk::{Api, ApiError};
use crate::commands::databases;
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Subcommands for `hotdata rollups`.
#[derive(clap::Subcommand)]
pub enum RollupsCommands {
    /// Create a rollup on a table and wait for it to build
    ///
    /// The table must hold data, and its time column must be a UTC timestamp.
    /// A rollup cannot be changed afterwards: remove it and create a new one.
    ///
    /// Creating a rollup restricts writes to the table. Rows older than the
    /// window can no longer be changed or deleted, SQL DELETE and UPDATE are
    /// refused, and a keyed load (`--mode update|delete|upsert`) must include
    /// the time column in its key. Appends are unaffected.
    Create {
        /// Table to roll up (`schema.table` in --database or the current
        /// database, or `catalog.schema.table`)
        table: String,

        /// Timestamp column rows are bucketed by
        #[arg(long = "time-column")]
        time_column: String,

        /// Width of a time bucket: the finest time grouping the rollup answers
        /// (e.g. `1m`, `1h`, `1d`, or a number of seconds)
        #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
        bucket: u64,

        /// Aggregate to keep, as `kind[:column]`; repeat for more. Kinds:
        /// `count` (rows, or `count:col` for non-null values), `sum:col`,
        /// `min:col`, `max:col`, `numeric:col` (answers count, sum, avg, min
        /// and max of the column), and `approx_percentile:col[:centroids]`
        /// (answers approx_percentile_cont and approx_median)
        #[arg(long = "measure", value_name = "KIND[:COLUMN]", required = true, value_parser = parse_measure)]
        measures: Vec<Measure>,

        /// Columns queries group or filter by, comma-separated
        #[arg(long, visible_alias = "by", value_delimiter = ',')]
        dimensions: Vec<String>,

        /// Keep only rows matching this SQL condition (e.g. "status <> 'error'").
        /// A query uses the rollup only when its WHERE clause contains the
        /// same condition, written the same way
        #[arg(long)]
        filter: Option<String>,

        /// How long after a row's time it can still be changed or deleted
        /// (default `1h`). Shared by every rollup on the table: omit it on
        /// later rollups, and change it with `hotdata rollups window`
        #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
        window: Option<u64>,

        /// Database the table is in (id, catalog, or name; defaults to the
        /// current database). Use with a `schema.table` table
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Create as a background job: return once the build has started
        #[arg(long)]
        r#async: bool,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// List the rollups in a database
    List {
        /// Database to list rollups in (id, catalog, or name; defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Only rollups on tables of this schema
        #[arg(long)]
        schema: Option<String>,

        /// Only rollups on this table (`schema.table`, or a bare table name in
        /// --schema, which defaults to `public`)
        #[arg(long)]
        table: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Show one rollup: what it keeps, its status, and how far its build has come
    Show {
        /// Rollup id (from `hotdata rollups list`)
        id: String,

        /// Database the rollup is in (id, catalog, or name; defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Build a rollup again and wait for it
    ///
    /// For a rollup that needs a rebuild (its table was replaced) or whose
    /// build failed; a failed build resumes where it stopped. A rollup that is
    /// already ready is left alone.
    Rebuild {
        /// Rollup id (from `hotdata rollups list`)
        id: String,

        /// Database the rollup is in (id, catalog, or name; defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Rebuild as a background job: return once the build has started
        #[arg(long)]
        r#async: bool,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Set how long a table's rows stay editable after their time
    ///
    /// The window applies to every rollup on the table. Nothing is rebuilt: a
    /// wider window applies as time passes (rows already past the old window
    /// stay closed), and a narrower one at the next refresh.
    Window {
        /// Table whose window to set (`schema.table` in --database or the
        /// current database, or `catalog.schema.table`)
        table: String,

        /// The new window (e.g. `30m`, `6h`, `1d`, or a number of seconds)
        #[arg(value_parser = parse_duration)]
        duration: u64,

        /// Database the table is in (id, catalog, or name; defaults to the
        /// current database). Use with a `schema.table` table
        #[arg(long, short = 'd')]
        database: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Remove a rollup; the table and its data do not change
    Remove {
        /// Rollup id (from `hotdata rollups list`)
        id: String,

        /// Database the rollup is in (id, catalog, or name; defaults to the current database)
        #[arg(long, short = 'd')]
        database: Option<String>,
    },
}

pub fn dispatch(workspace_id: &str, command: RollupsCommands) {
    match command {
        RollupsCommands::Create {
            table,
            time_column,
            bucket,
            measures,
            dimensions,
            filter,
            window,
            database,
            r#async,
            output,
        } => create(
            workspace_id,
            &table,
            database.as_deref(),
            &Declaration {
                time_column: &time_column,
                bucket_secs: bucket,
                dimensions: &dimensions,
                measures: &measures,
                filter: filter.as_deref(),
                window_secs: window,
            },
            r#async,
            &output,
        ),
        RollupsCommands::List {
            database,
            schema,
            table,
            output,
        } => list(
            workspace_id,
            database.as_deref(),
            schema.as_deref(),
            table.as_deref(),
            &output,
        ),
        RollupsCommands::Show {
            id,
            database,
            output,
        } => show(workspace_id, database.as_deref(), &id, &output),
        RollupsCommands::Rebuild {
            id,
            database,
            r#async,
            output,
        } => rebuild(workspace_id, database.as_deref(), &id, r#async, &output),
        RollupsCommands::Window {
            table,
            duration,
            database,
            output,
        } => window(workspace_id, &table, database.as_deref(), duration, &output),
        RollupsCommands::Remove { id, database } => remove(workspace_id, database.as_deref(), &id),
    }
}

// --- durations ---------------------------------------------------------------

/// Parse a duration flag into whole seconds: a bare number of seconds, or a
/// number with an `s`, `m`, `h` or `d` suffix.
fn parse_duration(input: &str) -> Result<u64, String> {
    let s = input.trim();
    let (digits, unit) = match s.char_indices().last() {
        Some((i, c)) if c.is_ascii_alphabetic() => (&s[..i], c.to_ascii_lowercase()),
        _ => (s, 's'),
    };
    let per_unit: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86_400,
        _ => {
            return Err(format!(
                "unknown unit in '{input}': use s, m, h or d (e.g. 30s, 5m, 1h, 1d)"
            ));
        }
    };
    let n: u64 = digits.parse().map_err(|_| {
        format!("'{input}' is not a duration: use a number with s, m, h or d (e.g. 5m, 1h, 1d)")
    })?;
    match n.checked_mul(per_unit) {
        Some(0) => Err("a duration must be longer than zero".to_string()),
        // The API carries seconds as a signed 64-bit number.
        Some(secs) if i64::try_from(secs).is_ok() => Ok(secs),
        _ => Err(format!("'{input}' is too long a duration")),
    }
}

/// Seconds as the largest unit that divides them exactly: `3600` is `1h`,
/// `5400` is `90m`. The inverse of [`parse_duration`].
fn format_duration(secs: u64) -> String {
    for (per_unit, suffix) in [(86_400, 'd'), (3600, 'h'), (60, 'm')] {
        if secs != 0 && secs.is_multiple_of(per_unit) {
            return format!("{}{suffix}", secs / per_unit);
        }
    }
    format!("{secs}s")
}

// --- measures ----------------------------------------------------------------

/// The measure kinds the API accepts.
const MEASURE_KINDS: [&str; 6] = ["count", "sum", "min", "max", "numeric", "approx_percentile"];

/// The digest size an `approx_percentile` measure keeps when none is named.
const DEFAULT_CENTROIDS: u64 = 100;

/// One measure of a rollup, as declared and as the API reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Measure {
    kind: String,
    column: Option<String>,
    centroids: Option<u64>,
}

/// Parse a `--measure` value: `kind`, `kind:column`, or for
/// `approx_percentile`, `kind:column:centroids`.
///
/// The centroids are read from the right, and only for `approx_percentile`
/// and only when the last segment is a number, so a column whose name
/// contains a colon still works.
fn parse_measure(spec: &str) -> Result<Measure, String> {
    let (kind, rest) = match spec.split_once(':') {
        Some((kind, rest)) => (kind, Some(rest).filter(|r| !r.is_empty())),
        None => (spec, None),
    };
    if kind == "avg" {
        let column = rest.unwrap_or("<column>");
        return Err(format!(
            "'avg' is not a measure kind: use 'numeric:{column}', which answers count, sum, \
             avg, min and max of the column"
        ));
    }
    if !MEASURE_KINDS.contains(&kind) {
        return Err(format!(
            "unknown measure kind '{kind}': use one of {}",
            MEASURE_KINDS.join(", ")
        ));
    }
    let (column, centroids) = match (kind, rest) {
        (_, None) => (None, None),
        ("approx_percentile", Some(rest)) => match rest.rsplit_once(':') {
            Some((column, n)) if !column.is_empty() && n.parse::<u64>().is_ok() => {
                (Some(column), n.parse::<u64>().ok())
            }
            _ => (Some(rest), None),
        },
        (_, Some(rest)) => (Some(rest), None),
    };
    if column.is_none() && kind != "count" {
        return Err(format!(
            "measure '{kind}' needs a column: '{kind}:<column>'"
        ));
    }
    Ok(Measure {
        kind: kind.to_string(),
        column: column.map(str::to_string),
        centroids,
    })
}

/// A measure in the grammar `--measure` takes, so what `show` prints can be
/// passed back to `create`. The default digest size is left out.
fn format_measure(m: &Measure) -> String {
    match (&m.column, m.centroids) {
        (None, _) => m.kind.clone(),
        (Some(column), Some(n)) if n != DEFAULT_CENTROIDS => format!("{}:{column}:{n}", m.kind),
        (Some(column), _) => format!("{}:{column}", m.kind),
    }
}

fn format_measures(measures: &[Measure]) -> String {
    measures
        .iter()
        .map(format_measure)
        .collect::<Vec<_>>()
        .join(", ")
}

// --- naming the table and the database ---------------------------------------

/// Split a table argument into (catalog, schema, table). The catalog is `None`
/// for a `schema.table`, which names a table in `--database` or the current
/// database.
fn split_table(table: &str) -> Result<(Option<&str>, &str, &str), String> {
    let parts: Vec<&str> = table.splitn(3, '.').collect();
    let shape = match parts.as_slice() {
        [catalog, schema, tbl] => Some((Some(*catalog), *schema, *tbl)),
        [schema, tbl] => Some((None, *schema, *tbl)),
        _ => None,
    };
    shape
        .filter(|(catalog, schema, tbl)| {
            catalog.is_none_or(|c| !c.is_empty()) && !schema.is_empty() && !tbl.is_empty()
        })
        .ok_or_else(|| {
            format!("'{table}' is not a table: use 'schema.table' or 'catalog.schema.table'")
        })
}

/// The `schema_name` / `table_name` filter `list` sends. The API refuses a
/// table without a schema, so a bare `--table` takes `--schema`, or `public`.
fn list_filter(
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<(Option<String>, Option<String>), String> {
    let Some(table) = table else {
        return Ok((schema.map(str::to_string), None));
    };
    match table.split('.').collect::<Vec<_>>().as_slice() {
        [tbl] if !tbl.is_empty() => Ok((
            Some(schema.unwrap_or("public").to_string()),
            Some(tbl.to_string()),
        )),
        [s, tbl] if !s.is_empty() && !tbl.is_empty() => match schema {
            Some(flag) if flag != *s => Err(format!(
                "--schema {flag} and --table {table} name different schemas"
            )),
            _ => Ok((Some(s.to_string()), Some(tbl.to_string()))),
        },
        _ => Err(format!(
            "--table '{table}' must be a table name or 'schema.table'"
        )),
    }
}

fn exit_with(message: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!("{}", format!("error: {message}").red());
    std::process::exit(1);
}

/// The database a command addresses: `--database`, else the current one
/// (`HOTDATA_DATABASE`, or the one set with `databases use`).
fn database_id(api: &Api, flag: Option<&str>) -> String {
    match flag.or(api.database_id()) {
        Some(key) => databases::resolve_database_flag(api, key),
        None => exit_with(
            "no database: pass --database <id>, or set a current database with \
             `hotdata databases use <id>`",
        ),
    }
}

/// Resolve a table argument to (database id, schema, table).
fn resolve_table(api: &Api, table: &str, database: Option<&str>) -> (String, String, String) {
    let (catalog, schema, tbl) = split_table(table).unwrap_or_else(|e| exit_with(&e));
    let db = match (catalog, database) {
        (Some(_), Some(_)) => exit_with(
            "--database takes a 'schema.table' table; drop the catalog from the table, or \
             drop --database",
        ),
        (Some(catalog), None) => {
            databases::try_resolve_database_preferring_active(api, catalog)
                .unwrap_or_else(|e| exit_with(&e))
                .id
        }
        (None, flag) => database_id(api, flag),
    };
    (db, schema.to_string(), tbl.to_string())
}

/// A rollup id as a path segment. Ids are opaque tokens from `rollups list`;
/// anything else (a table name, say) is refused here rather than sent.
fn checked_id(id: &str) -> &str {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        exit_with(&format!(
            "'{id}' is not a rollup id; list ids with `hotdata rollups list`"
        ));
    }
    id
}

// --- the API -----------------------------------------------------------------

/// How long a request waits for the table when another write holds it.
const LOCK_RETRY_TIMEOUT: Duration = Duration::from_secs(60);

fn rollups_path(database_id: &str) -> String {
    format!("/databases/{database_id}/rollups")
}

fn rollup_path(database_id: &str, id: &str) -> String {
    format!("/databases/{database_id}/rollups/{id}")
}

fn parse_json(body: &str) -> Result<Value, ApiError> {
    serde_json::from_str(body)
        .map_err(|e| ApiError::Transport(format!("error parsing response: {e}")))
}

/// Send one request; a non-success status is an error carrying the server's
/// body.
fn send(
    api: &Api,
    method: Method,
    path: &str,
    query: &[(&str, String)],
    body: Option<&Value>,
) -> Result<String, ApiError> {
    let (status, _, text) = api.send_raw(method, path, query, body, None)?;
    if status.is_success() {
        Ok(text)
    } else {
        Err(ApiError::Status { status, body: text })
    }
}

/// [`send`] for the calls that write to the table's rollups: declaring one,
/// rebuilding one and setting the window all wait their turn behind a load
/// into the same table, which the server reports as a `RESOURCE_LOCKED` 409
/// with a `Retry-After`. That one is retried until [`LOCK_RETRY_TIMEOUT`]; any
/// other 409 is the caller's to read.
fn send_when_unlocked(
    api: &Api,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<String, ApiError> {
    let deadline = Instant::now() + LOCK_RETRY_TIMEOUT;
    loop {
        let (status, headers, text) = api.send_raw(method.clone(), path, &[], body, None)?;
        if status.is_success() {
            return Ok(text);
        }
        match databases::lock_retry_after(status, &headers, &text) {
            Some(wait) if Instant::now() + wait <= deadline => std::thread::sleep(wait),
            _ => return Err(ApiError::Status { status, body: text }),
        }
    }
}

fn get_rollup(api: &Api, database_id: &str, id: &str) -> Result<Value, ApiError> {
    parse_json(&send(
        api,
        Method::GET,
        &rollup_path(database_id, id),
        &[],
        None,
    )?)
}

fn list_rollups(
    api: &Api,
    database_id: &str,
    schema: Option<&str>,
    table: Option<&str>,
) -> Result<Vec<Value>, ApiError> {
    let mut query: Vec<(&str, String)> = Vec::new();
    if let Some(schema) = schema {
        query.push(("schema_name", schema.to_string()));
    }
    if let Some(table) = table {
        query.push(("table_name", table.to_string()));
    }
    let mut body = parse_json(&send(
        api,
        Method::GET,
        &rollups_path(database_id),
        &query,
        None,
    )?)?;
    match body["rollups"].take() {
        Value::Array(rollups) => Ok(rollups),
        _ => Err(ApiError::Transport(
            "error parsing response: no `rollups` list".to_string(),
        )),
    }
}

/// What `create` declares, besides the table.
struct Declaration<'a> {
    time_column: &'a str,
    bucket_secs: u64,
    dimensions: &'a [String],
    measures: &'a [Measure],
    filter: Option<&'a str>,
    window_secs: Option<u64>,
}

/// The body of a declare request. Optional fields are left out rather than
/// sent as `null`.
fn declare_body(schema: &str, table: &str, d: &Declaration<'_>) -> Value {
    let measures: Vec<Value> = d
        .measures
        .iter()
        .map(|m| {
            let mut measure = json!({ "kind": m.kind });
            if let Some(column) = &m.column {
                measure["column"] = json!(column);
            }
            if let Some(centroids) = m.centroids {
                measure["centroids"] = json!(centroids);
            }
            measure
        })
        .collect();
    let mut body = json!({
        "schema_name": schema,
        "table_name": table,
        "time_column": d.time_column,
        "bucket_width_secs": d.bucket_secs,
        "dimensions": d.dimensions,
        "measures": measures,
    });
    if let Some(filter) = d.filter {
        body["filter"] = json!(filter);
    }
    if let Some(window) = d.window_secs {
        body["window_secs"] = json!(window);
    }
    body
}

/// A rollup and the job building it, if the server named one.
#[derive(Debug)]
struct Build {
    rollup: Value,
    job_id: Option<String>,
}

impl Build {
    fn from_response(mut body: Value) -> Self {
        Build {
            job_id: body["job_id"].as_str().map(str::to_string),
            rollup: body["rollup"].take(),
        }
    }

    fn id(&self) -> String {
        self.rollup["id"].as_str().unwrap_or_default().to_string()
    }

    fn to_value(&self) -> Value {
        json!({ "rollup": self.rollup, "job_id": self.job_id })
    }
}

/// What asking for a rebuild came to.
#[derive(Debug)]
enum BuildStart {
    /// A build was started.
    Started(Build),
    /// The rollup is `ready`: there is nothing to build.
    AlreadyReady(Value),
    /// A build of the rollup is already running.
    AlreadyBuilding(Value),
}

/// Ask for a rebuild, and read a refusal by the rollup's state.
///
/// The server refuses with the same 409 a rollup that is ready, one already
/// being built, and one being dropped. The first two are not failures for
/// someone who wants the rollup built, so the rollup is read to tell them
/// apart; anything else stays the server's error.
fn start_build(api: &Api, database_id: &str, id: &str) -> Result<BuildStart, ApiError> {
    let path = format!("{}/rebuild", rollup_path(database_id, id));
    let refusal = match send_when_unlocked(api, Method::POST, &path, None) {
        Ok(body) => {
            return Ok(BuildStart::Started(Build::from_response(parse_json(
                &body,
            )?)));
        }
        Err(e) => e,
    };
    if !matches!(&refusal, ApiError::Status { status, .. } if *status == StatusCode::CONFLICT) {
        return Err(refusal);
    }
    let rollup = get_rollup(api, database_id, id)?;
    match rollup["status"].as_str() {
        Some("ready") => Ok(BuildStart::AlreadyReady(rollup)),
        Some("building") => Ok(BuildStart::AlreadyBuilding(rollup)),
        _ => Err(refusal),
    }
}

/// Declare a rollup and make sure its build is under way.
///
/// The server can answer a declaration without a job: the rollup exists, but
/// its build could not be started. Declaring it again would be refused as a
/// duplicate, so the build is started here with one rebuild request. The
/// `Err` carries the declared rollup when only that second step failed.
fn declare_and_start(
    api: &Api,
    database_id: &str,
    body: &Value,
) -> Result<Build, (ApiError, Option<Build>)> {
    let declared = send_when_unlocked(api, Method::POST, &rollups_path(database_id), Some(body))
        .and_then(|text| parse_json(&text))
        .map(Build::from_response)
        .map_err(|e| (e, None))?;
    if declared.job_id.is_some() {
        return Ok(declared);
    }
    match start_build(api, database_id, &declared.id()) {
        Ok(BuildStart::Started(build)) => Ok(build),
        Ok(BuildStart::AlreadyReady(rollup) | BuildStart::AlreadyBuilding(rollup)) => Ok(Build {
            rollup,
            job_id: None,
        }),
        Err(e) => Err((e, Some(declared))),
    }
}

/// The fields of a rollup the CLI displays or branches on. Everything is
/// optional on the wire as far as this is concerned: a field this build does
/// not know is ignored, and one it expects but does not get shows as empty.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Rollup {
    id: String,
    schema_name: String,
    table_name: String,
    time_column: String,
    bucket_width_secs: u64,
    dimensions: Vec<String>,
    measures: Vec<Measure>,
    filter: Option<String>,
    window_secs: Option<u64>,
    status: String,
    error: Option<String>,
    failed_at: Option<String>,
    covered_until: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

impl Rollup {
    fn from_value(value: &Value) -> Result<Self, ApiError> {
        Rollup::deserialize(value)
            .map_err(|e| ApiError::Transport(format!("error parsing response: {e}")))
    }

    fn table(&self) -> String {
        format!("{}.{}", self.schema_name, self.table_name)
    }

    fn window(&self) -> String {
        self.window_secs
            .map(format_duration)
            .unwrap_or_else(|| "-".to_string())
    }
}

// --- waiting for a build -----------------------------------------------------

/// How a build that was waited for ended.
#[derive(Debug)]
enum Built {
    Ready(Value),
    /// The build failed for good; the rollup says why.
    Failed(Value),
    /// The table was replaced while the rollup was building.
    NeedsRebuild,
    /// The rollup was dropped while it was building.
    Dropped,
}

/// Pause between two reads of a building rollup: short at first, since small
/// tables build in seconds, then long, since large ones take hours.
const POLL_FAST: Duration = Duration::from_secs(2);
const POLL_SLOW: Duration = Duration::from_secs(10);
const POLL_FAST_FOR: Duration = Duration::from_secs(60);
/// One read's own time limit, well under the client's, so a stalled read
/// costs one attempt rather than minutes.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// Consecutive reads that may fail (no response, a 5xx, a 429 or a 408) before the wait
/// gives up. The build itself is unaffected either way.
const MAX_FAILED_POLLS: u32 = 5;

/// Read a rollup until its build ends.
///
/// The rollup's own status is the authority, not its job's: a build is
/// retried across several attempts, and only the rollup says when it has
/// failed for good. There is no deadline, because a first build of a large
/// table can run for hours. `pause` waits between reads and `building` is
/// told each state seen on the way.
fn wait_until_built(
    api: &Api,
    database_id: &str,
    id: &str,
    mut pause: impl FnMut(Duration),
    mut building: impl FnMut(&Rollup),
) -> Result<Built, ApiError> {
    let path = rollup_path(database_id, id);
    let mut failed_polls = 0;
    let mut waited = Duration::ZERO;
    loop {
        let failure = match api.send_raw(Method::GET, &path, &[], None, Some(POLL_TIMEOUT)) {
            Ok((status, _, body)) if status.is_success() => {
                failed_polls = 0;
                let value = parse_json(&body)?;
                let rollup = Rollup::from_value(&value)?;
                match rollup.status.as_str() {
                    "ready" => return Ok(Built::Ready(value)),
                    "build_failed" => return Ok(Built::Failed(value)),
                    "needs_rebuild" => return Ok(Built::NeedsRebuild),
                    "dropping" => return Ok(Built::Dropped),
                    _ => building(&rollup),
                }
                None
            }
            Ok((StatusCode::NOT_FOUND, _, _)) => return Ok(Built::Dropped),
            // A 5xx, a rate limit or a request timeout says nothing about the
            // build: over a wait of hours one of them is likely, and none is a
            // reason to stop.
            Ok((status, _, body))
                if status.is_server_error()
                    || status == StatusCode::TOO_MANY_REQUESTS
                    || status == StatusCode::REQUEST_TIMEOUT =>
            {
                Some(ApiError::Status { status, body })
            }
            // Any other status says something is wrong with the request
            // itself (the credential, say), which reading again cannot fix.
            Ok((status, _, body)) => return Err(ApiError::Status { status, body }),
            Err(e) => Some(e),
        };
        if let Some(failure) = failure {
            failed_polls += 1;
            if failed_polls > MAX_FAILED_POLLS {
                return Err(failure);
            }
        }
        let step = if waited < POLL_FAST_FOR {
            POLL_FAST
        } else {
            POLL_SLOW
        };
        pause(step);
        waited += step;
    }
}

/// Wait for a build with a spinner, and exit non-zero unless it ends `ready`.
fn wait_or_exit(api: &Api, database_id: &str, build: &Build) -> Value {
    use crossterm::style::Stylize;
    let id = build.id();
    eprintln!("{}", format!("rollup: {id}").dark_grey());
    if let Some(job_id) = &build.job_id {
        eprintln!("{}", format!("job_id: {job_id}").dark_grey());
    }
    eprintln!(
        "{}",
        format!(
            "Waiting for the build. Ctrl-C stops the wait, not the build; check it later \
             with `hotdata rollups show {id}`."
        )
        .dark_grey()
    );

    let spinner = crate::util::spinner("building rollup...");
    let outcome = wait_until_built(api, database_id, &id, std::thread::sleep, |rollup| {
        if let Some(until) = &rollup.covered_until {
            spinner.set_message(format!(
                "building rollup... built until {}",
                crate::util::format_date(until)
            ));
        }
    });
    spinner.finish_and_clear();

    let rebuild_hint = format!("hotdata rollups rebuild {id}");
    match outcome {
        Ok(Built::Ready(rollup)) => rollup,
        Ok(Built::Failed(rollup)) => {
            let error = rollup["error"].as_str().unwrap_or("no reason was recorded");
            eprintln!("{}", format!("error: the build failed: {error}").red());
            eprintln!(
                "{}",
                format!("Its progress is kept. Resume it with `{rebuild_hint}`.").dark_grey()
            );
            std::process::exit(1);
        }
        Ok(Built::NeedsRebuild) => {
            eprintln!(
                "{}",
                "error: the table was replaced while the rollup was building".red()
            );
            eprintln!(
                "{}",
                format!("Build it from the new data with `{rebuild_hint}`.").dark_grey()
            );
            std::process::exit(1);
        }
        Ok(Built::Dropped) => exit_with(&format!("rollup {id} was removed while it was building")),
        Err(e) => {
            e.print();
            eprintln!(
                "{}",
                format!(
                    "Stopped waiting; the build is not affected. Check it with \
                     `hotdata rollups show {id}`."
                )
                .dark_grey()
            );
            std::process::exit(1);
        }
    }
}

// --- setting the window ------------------------------------------------------

#[derive(Debug)]
enum WindowError {
    /// The table has no rollups, so it has no window.
    NoRollups,
    /// Every rollup on the table is waiting for a rebuild or being dropped,
    /// and the server sets the window through neither.
    NoneSettable,
    Api(ApiError),
}

/// Set the window of a table through one of its rollups.
///
/// The window belongs to the table, but the API sets it through a rollup, and
/// refuses one that is being dropped or whose table was replaced. So the
/// rollups that can carry it are tried in turn, and the first that takes it
/// settles it for all of them.
fn set_table_window(
    api: &Api,
    database_id: &str,
    rollups: &[Value],
    window_secs: u64,
) -> Result<Value, WindowError> {
    if rollups.is_empty() {
        return Err(WindowError::NoRollups);
    }
    let body = json!({ "window_secs": window_secs });
    let mut refusal = None;
    for rollup in rollups {
        let (Some(id), status) = (rollup["id"].as_str(), rollup["status"].as_str()) else {
            continue;
        };
        if matches!(status, Some("dropping" | "needs_rebuild")) {
            continue;
        }
        let path = format!("{}/window", rollup_path(database_id, id));
        match send_when_unlocked(api, Method::PUT, &path, Some(&body)) {
            Ok(text) => return parse_json(&text).map_err(WindowError::Api),
            // This rollup changed state since it was listed; another may not have.
            Err(e @ ApiError::Status { status, .. }) if status == StatusCode::CONFLICT => {
                refusal = Some(e)
            }
            Err(e) => return Err(WindowError::Api(e)),
        }
    }
    Err(refusal.map_or(WindowError::NoneSettable, WindowError::Api))
}

// --- output ------------------------------------------------------------------

fn print_value(value: &Value, output: &str) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(value).unwrap()),
        _ => print!("{}", serde_yaml::to_string(value).unwrap()),
    }
}

/// `rollups show` in table mode: one `key: value` line per field, then what
/// to do about a rollup that is not serving queries.
fn show_lines(r: &Rollup) -> Vec<String> {
    let line = |label: &str, value: &str| format!("{:<14}{value}", format!("{label}:"));
    let or_dash = |v: &Option<String>| v.clone().unwrap_or_else(|| "-".to_string());
    let mut lines = vec![
        line("id", &r.id),
        line("table", &r.table()),
        line("time column", &r.time_column),
        line("bucket", &format_duration(r.bucket_width_secs)),
        line(
            "dimensions",
            &if r.dimensions.is_empty() {
                "-".to_string()
            } else {
                r.dimensions.join(", ")
            },
        ),
        line("measures", &format_measures(&r.measures)),
    ];
    if let Some(filter) = &r.filter {
        lines.push(line("filter", filter));
    }
    lines.push(line("window", &r.window()));
    lines.push(line("status", &r.status));
    // After its table is replaced a rollup still reports how far it had built
    // over the old data, which says nothing about the new.
    if r.status != "needs_rebuild" {
        lines.push(line("built until", &or_dash(&r.covered_until)));
    }
    if r.status == "build_failed" {
        lines.push(line("error", &or_dash(&r.error)));
        lines.push(line("failed at", &or_dash(&r.failed_at)));
    }
    lines.push(line("created", &or_dash(&r.created_at)));
    lines.push(line("updated", &or_dash(&r.updated_at)));
    match r.status.as_str() {
        "build_failed" => lines.push(format!(
            "\nThe build failed and its progress is kept. Resume it with \
             `hotdata rollups rebuild {}`.",
            r.id
        )),
        "needs_rebuild" => lines.push(format!(
            "\nThe table was replaced, so queries read the table until the rollup is rebuilt: \
             `hotdata rollups rebuild {}`.",
            r.id
        )),
        _ => {}
    }
    lines
}

const LIST_HEADERS: [&str; 7] = [
    "ID",
    "TABLE",
    "STATUS",
    "BUCKET",
    "DIMENSIONS",
    "MEASURES",
    "BUILT UNTIL",
];

fn list_row(r: &Rollup) -> Vec<String> {
    vec![
        r.id.clone(),
        r.table(),
        r.status.clone(),
        format_duration(r.bucket_width_secs),
        r.dimensions.join(", "),
        format_measures(&r.measures),
        match (&r.covered_until, r.status.as_str()) {
            (Some(until), status) if status != "needs_rebuild" => crate::util::format_date(until),
            _ => "-".to_string(),
        },
    ]
}

/// What creating a rollup changes about writing to its table. Printed on
/// every create, because nothing else about the command suggests it.
fn write_note(r: &Rollup) -> String {
    format!(
        "Writes to {table} are now restricted: rows older than the window ({window}) cannot be \
         changed or deleted, SQL DELETE and UPDATE are refused, and a keyed load \
         (--mode update|delete|upsert) must include `{time}` in its key. Appends are unaffected.",
        table = r.table(),
        window = r.window(),
        time = r.time_column,
    )
}

/// Print a build that was started and not waited for.
fn print_started(build: &Build, output: &str) {
    use crossterm::style::Stylize;
    if output != "table" {
        return print_value(&build.to_value(), output);
    }
    let id = build.id();
    println!("rollup: {id}");
    if let Some(job_id) = &build.job_id {
        println!("job_id: {job_id}");
    }
    eprintln!(
        "{}",
        format!(
            "The rollup is building. Queries use it once `hotdata rollups show {id}` reports \
             ready."
        )
        .dark_grey()
    );
}

/// Print a rollup whose build was waited for and ended `ready`.
fn print_ready(rollup: &Value, job_id: Option<&str>, output: &str) -> Rollup {
    use crossterm::style::Stylize;
    let view = Rollup::from_value(rollup).unwrap_or_else(|e| e.exit());
    if output == "table" {
        println!("{}", format!("Rollup {} is ready.", view.id).green());
        for line in show_lines(&view) {
            println!("{line}");
        }
    } else {
        print_value(&json!({ "rollup": rollup, "job_id": job_id }), output);
    }
    view
}

// --- the verbs ---------------------------------------------------------------

fn create(
    workspace_id: &str,
    table: &str,
    database: Option<&str>,
    declaration: &Declaration<'_>,
    async_mode: bool,
    output: &str,
) {
    use crossterm::style::Stylize;
    let api = Api::new(Some(workspace_id));
    let (database_id, schema, tbl) = resolve_table(&api, table, database);
    let body = declare_body(&schema, &tbl, declaration);

    let build = match declare_and_start(&api, &database_id, &body) {
        Ok(build) => build,
        Err((e, None)) => e.exit(),
        Err((e, Some(declared))) => {
            e.print();
            eprintln!(
                "{}",
                format!(
                    "Rollup {id} was created, but its build did not start. Start it with \
                     `hotdata rollups rebuild {id}`.",
                    id = declared.id()
                )
                .dark_grey()
            );
            std::process::exit(1);
        }
    };

    let view = if async_mode {
        print_started(&build, output);
        Rollup::from_value(&build.rollup).unwrap_or_else(|e| e.exit())
    } else {
        let rollup = wait_or_exit(&api, &database_id, &build);
        let view = print_ready(&rollup, build.job_id.as_deref(), output);
        eprintln!(
            "{}",
            format!(
                "Aggregate queries on {} that fit the rollup now read it; `hotdata query` \
                 reports `rollup: used`.",
                view.table()
            )
            .dark_grey()
        );
        view
    };
    eprintln!("{}", write_note(&view).dark_grey());
}

fn list(
    workspace_id: &str,
    database: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    output: &str,
) {
    let (schema, table) = list_filter(schema, table).unwrap_or_else(|e| exit_with(&e));
    let api = Api::new(Some(workspace_id));
    let database_id = database_id(&api, database);
    let rollups = list_rollups(&api, &database_id, schema.as_deref(), table.as_deref())
        .unwrap_or_else(|e| e.exit());
    if output != "table" {
        return print_value(&Value::Array(rollups), output);
    }
    if rollups.is_empty() {
        use crossterm::style::Stylize;
        eprintln!("{}", "No rollups found.".dark_grey());
        return;
    }
    let rows: Vec<Vec<String>> = rollups
        .iter()
        .map(|v| list_row(&Rollup::from_value(v).unwrap_or_else(|e| e.exit())))
        .collect();
    crate::output::table::print(&LIST_HEADERS, &rows);
}

fn show(workspace_id: &str, database: Option<&str>, id: &str, output: &str) {
    let id = checked_id(id);
    let api = Api::new(Some(workspace_id));
    let database_id = database_id(&api, database);
    let rollup = get_rollup(&api, &database_id, id).unwrap_or_else(|e| e.exit());
    if output != "table" {
        return print_value(&rollup, output);
    }
    for line in show_lines(&Rollup::from_value(&rollup).unwrap_or_else(|e| e.exit())) {
        println!("{line}");
    }
}

fn rebuild(workspace_id: &str, database: Option<&str>, id: &str, async_mode: bool, output: &str) {
    let id = checked_id(id);
    let api = Api::new(Some(workspace_id));
    let database_id = database_id(&api, database);
    let build = match start_build(&api, &database_id, id).unwrap_or_else(|e| e.exit()) {
        BuildStart::Started(build) => build,
        BuildStart::AlreadyBuilding(rollup) => Build {
            rollup,
            job_id: None,
        },
        BuildStart::AlreadyReady(rollup) => {
            if output == "table" {
                println!("Rollup {id} is already ready; nothing to rebuild.");
            } else {
                print_value(&json!({ "rollup": rollup, "job_id": null }), output);
            }
            return;
        }
    };
    if async_mode {
        return print_started(&build, output);
    }
    let rollup = wait_or_exit(&api, &database_id, &build);
    print_ready(&rollup, build.job_id.as_deref(), output);
}

fn window(workspace_id: &str, table: &str, database: Option<&str>, window_secs: u64, output: &str) {
    use crossterm::style::Stylize;
    let api = Api::new(Some(workspace_id));
    let (database_id, schema, tbl) = resolve_table(&api, table, database);
    let rollups =
        list_rollups(&api, &database_id, Some(&schema), Some(&tbl)).unwrap_or_else(|e| e.exit());
    let rollup = match set_table_window(&api, &database_id, &rollups, window_secs) {
        Ok(rollup) => rollup,
        Err(WindowError::NoRollups) => exit_with(&format!(
            "{schema}.{tbl} has no rollups, so it has no window to set"
        )),
        Err(WindowError::NoneSettable) => exit_with(&format!(
            "every rollup on {schema}.{tbl} needs a rebuild or is being removed; rebuild one \
             first (`hotdata rollups list --table {schema}.{tbl}`, then `hotdata rollups \
             rebuild <id>`)"
        )),
        Err(WindowError::Api(e)) => e.exit(),
    };
    if output != "table" {
        return print_value(&rollup, output);
    }
    println!(
        "{}",
        format!(
            "Window for {schema}.{tbl} set to {}.",
            format_duration(window_secs)
        )
        .green()
    );
    eprintln!(
        "{}",
        "It applies to every rollup on the table. Rows already past the old window stay closed."
            .dark_grey()
    );
}

fn remove(workspace_id: &str, database: Option<&str>, id: &str) {
    use crossterm::style::Stylize;
    let id = checked_id(id);
    let api = Api::new(Some(workspace_id));
    let database_id = database_id(&api, database);

    // The server answers once the rollup is gone, which waits for a build
    // step that is running on it.
    let spinner = crate::util::spinner("removing rollup (this can take up to two minutes)...");
    let result = send(
        &api,
        Method::DELETE,
        &rollup_path(&database_id, id),
        &[],
        None,
    );
    spinner.finish_and_clear();

    let Err(e) = result else {
        println!("{}", format!("Rollup {id} removed.").green());
        return;
    };
    // A removal that ran out of time behind a build step or a load leaves the
    // rollup marked as being dropped; asking again finishes it.
    let still_dropping = matches!(&e, ApiError::Status { status, .. } if *status == StatusCode::CONFLICT)
        && get_rollup(&api, &database_id, id)
            .is_ok_and(|rollup| rollup["status"].as_str() == Some("dropping"));
    e.print();
    if still_dropping {
        eprintln!(
            "{}",
            format!(
                "Rollup {id} is marked for removal but its table was busy. Run \
                 `hotdata rollups remove {id}` again to finish."
            )
            .dark_grey()
        );
    }
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &str = "dbid1";

    fn api(server: &mockito::Server) -> Api {
        Api::test_new(&server.url(), "k", Some("ws"))
    }

    fn rollup_json(id: &str, status: &str) -> Value {
        json!({
            "id": id,
            "database_id": DB,
            "schema_name": "public",
            "table_name": "requests",
            "time_column": "ts",
            "bucket_width_secs": 3600,
            "dimensions": ["region", "status"],
            "measures": [
                {"kind": "count"},
                {"kind": "numeric", "column": "bytes"},
                {"kind": "approx_percentile", "column": "latency_ms", "centroids": 100}
            ],
            "window_secs": 3600,
            "status": status,
            "error": null,
            "failed_at": null,
            "covered_until": "2026-11-18T16:00:00Z",
            "created_at": "2026-11-18T10:00:00Z",
            "updated_at": "2026-11-18T10:05:00Z"
        })
    }

    fn mock_get(
        server: &mut mockito::Server,
        id: &str,
        status: &str,
        times: usize,
    ) -> mockito::Mock {
        server
            .mock("GET", format!("/v1/databases/{DB}/rollups/{id}").as_str())
            .with_status(200)
            .with_body(rollup_json(id, status).to_string())
            .expect(times)
            .create()
    }

    fn conflict(message: &str) -> String {
        json!({"error": {"code": "CONFLICT", "message": message}}).to_string()
    }

    // --- durations ---

    #[test]
    fn durations_parse_with_and_without_a_unit() {
        assert_eq!(parse_duration("90"), Ok(90));
        assert_eq!(parse_duration("30s"), Ok(30));
        assert_eq!(parse_duration("5m"), Ok(300));
        assert_eq!(parse_duration("1h"), Ok(3600));
        assert_eq!(parse_duration("1H"), Ok(3600));
        assert_eq!(parse_duration("2d"), Ok(172_800));
    }

    #[test]
    fn durations_refuse_zero_junk_and_overflow() {
        for bad in ["0", "0h", "", "h", "1w", "1.5h", "-5m", "one hour"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be refused");
        }
        // Past what the API's signed seconds can carry, with and without the
        // multiplication itself overflowing.
        assert!(parse_duration("213503982334602d").is_err());
        assert!(parse_duration("18446744073709551615").is_err());
    }

    #[test]
    fn durations_print_in_the_largest_exact_unit_and_parse_back() {
        for (secs, text) in [
            (30, "30s"),
            (60, "1m"),
            (5400, "90m"),
            (3600, "1h"),
            (86_400, "1d"),
            (90_000, "25h"),
            (61, "61s"),
        ] {
            assert_eq!(format_duration(secs), text);
            assert_eq!(parse_duration(text), Ok(secs));
        }
    }

    // --- measures ---

    fn measure(kind: &str, column: Option<&str>, centroids: Option<u64>) -> Measure {
        Measure {
            kind: kind.to_string(),
            column: column.map(str::to_string),
            centroids,
        }
    }

    #[test]
    fn measures_parse_in_each_form() {
        assert_eq!(parse_measure("count"), Ok(measure("count", None, None)));
        assert_eq!(
            parse_measure("count:user_id"),
            Ok(measure("count", Some("user_id"), None))
        );
        assert_eq!(
            parse_measure("numeric:bytes"),
            Ok(measure("numeric", Some("bytes"), None))
        );
        assert_eq!(
            parse_measure("approx_percentile:latency_ms"),
            Ok(measure("approx_percentile", Some("latency_ms"), None))
        );
        assert_eq!(
            parse_measure("approx_percentile:latency_ms:200"),
            Ok(measure("approx_percentile", Some("latency_ms"), Some(200)))
        );
    }

    #[test]
    fn a_colon_in_a_column_name_is_kept() {
        // Only `approx_percentile` reads a trailing number as centroids.
        assert_eq!(
            parse_measure("sum:http:200"),
            Ok(measure("sum", Some("http:200"), None))
        );
        assert_eq!(
            parse_measure("approx_percentile:http:latency"),
            Ok(measure("approx_percentile", Some("http:latency"), None))
        );
    }

    #[test]
    fn avg_is_refused_with_the_kind_to_use_instead() {
        let err = parse_measure("avg:bytes").unwrap_err();
        assert!(err.contains("numeric:bytes"), "{err}");
    }

    #[test]
    fn measures_refuse_unknown_kinds_and_missing_columns() {
        let err = parse_measure("median:x").unwrap_err();
        assert!(err.contains("approx_percentile"), "{err}");
        for kind in ["sum", "min", "max", "numeric", "approx_percentile"] {
            for spec in [kind.to_string(), format!("{kind}:")] {
                let err = parse_measure(&spec).unwrap_err();
                assert!(err.contains("needs a column"), "{spec}: {err}");
            }
        }
    }

    #[test]
    fn measures_print_in_the_grammar_create_takes() {
        // The server reports the default digest size explicitly; printing it
        // would add noise, and leaving it out still parses to the same rollup.
        for (m, text) in [
            (measure("count", None, None), "count"),
            (measure("numeric", Some("bytes"), None), "numeric:bytes"),
            (
                measure("approx_percentile", Some("ms"), Some(100)),
                "approx_percentile:ms",
            ),
            (
                measure("approx_percentile", Some("ms"), Some(200)),
                "approx_percentile:ms:200",
            ),
        ] {
            assert_eq!(format_measure(&m), text);
            let parsed = parse_measure(text).unwrap();
            assert_eq!(format_measure(&parsed), text);
        }
    }

    // --- tables ---

    #[test]
    fn tables_split_into_catalog_schema_and_table() {
        assert_eq!(split_table("public.t"), Ok((None, "public", "t")));
        assert_eq!(
            split_table("demo.public.t"),
            Ok((Some("demo"), "public", "t"))
        );
        for bad in ["t", "", ".t", "public.", "demo..t", ".public.t"] {
            assert!(split_table(bad).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn list_never_filters_by_a_table_without_a_schema() {
        let owned = |s: &str| Some(s.to_string());
        assert_eq!(list_filter(None, None), Ok((None, None)));
        assert_eq!(list_filter(Some("s"), None), Ok((owned("s"), None)));
        assert_eq!(
            list_filter(None, Some("t")),
            Ok((owned("public"), owned("t")))
        );
        assert_eq!(
            list_filter(Some("s"), Some("t")),
            Ok((owned("s"), owned("t")))
        );
        assert_eq!(list_filter(None, Some("s.t")), Ok((owned("s"), owned("t"))));
        assert_eq!(
            list_filter(Some("s"), Some("s.t")),
            Ok((owned("s"), owned("t")))
        );
        assert!(list_filter(Some("other"), Some("s.t")).is_err());
        assert!(list_filter(None, Some("c.s.t")).is_err());
    }

    // --- requests ---

    #[test]
    fn declare_sends_the_api_body_and_omits_unset_options() {
        let measures = vec![
            measure("count", None, None),
            measure("numeric", Some("bytes"), None),
            measure("approx_percentile", Some("latency_ms"), Some(200)),
        ];
        let dimensions = vec!["region".to_string(), "status".to_string()];
        let body = declare_body(
            "public",
            "requests",
            &Declaration {
                time_column: "ts",
                bucket_secs: 3600,
                dimensions: &dimensions,
                measures: &measures,
                filter: None,
                window_secs: None,
            },
        );

        let mut server = mockito::Server::new();
        let declare = server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .match_header("Authorization", "Bearer k")
            .match_body(mockito::Matcher::Json(json!({
                "schema_name": "public",
                "table_name": "requests",
                "time_column": "ts",
                "bucket_width_secs": 3600,
                "dimensions": ["region", "status"],
                "measures": [
                    {"kind": "count"},
                    {"kind": "numeric", "column": "bytes"},
                    {"kind": "approx_percentile", "column": "latency_ms", "centroids": 200}
                ]
            })))
            .with_status(201)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": "jobs1"}).to_string(),
            )
            .create();

        let build = declare_and_start(&api(&server), DB, &body).unwrap();
        declare.assert();
        assert_eq!(build.id(), "rlup1");
        assert_eq!(build.job_id.as_deref(), Some("jobs1"));
    }

    #[test]
    fn declare_carries_the_filter_and_window_when_set() {
        let measures = vec![measure("count", None, None)];
        let body = declare_body(
            "public",
            "requests",
            &Declaration {
                time_column: "ts",
                bucket_secs: 60,
                dimensions: &[],
                measures: &measures,
                filter: Some("status <> 'error'"),
                window_secs: Some(21_600),
            },
        );
        assert_eq!(body["filter"], "status <> 'error'");
        assert_eq!(body["window_secs"], 21_600);
        assert_eq!(body["dimensions"], json!([]));
    }

    #[test]
    fn a_declaration_without_a_job_starts_the_build_once() {
        let mut server = mockito::Server::new();
        let declare = server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .with_status(201)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": null}).to_string(),
            )
            .create();
        let rebuild = server
            .mock(
                "POST",
                format!("/v1/databases/{DB}/rollups/rlup1/rebuild").as_str(),
            )
            .with_status(202)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": "jobs2"}).to_string(),
            )
            .expect(1)
            .create();

        let build = declare_and_start(&api(&server), DB, &json!({})).unwrap();
        declare.assert();
        rebuild.assert();
        assert_eq!(build.job_id.as_deref(), Some("jobs2"));
    }

    #[test]
    fn a_build_that_cannot_start_reports_the_rollup_it_left_behind() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .with_status(201)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": null}).to_string(),
            )
            .create();
        server
            .mock(
                "POST",
                format!("/v1/databases/{DB}/rollups/rlup1/rebuild").as_str(),
            )
            .with_status(500)
            .with_body("boom")
            .create();

        let (_, declared) = declare_and_start(&api(&server), DB, &json!({})).unwrap_err();
        assert_eq!(declared.unwrap().id(), "rlup1");
    }

    #[test]
    fn a_refused_declaration_is_the_servers_error() {
        let mut server = mockito::Server::new();
        server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .with_status(409)
            .with_body(conflict("an identical rollup already exists"))
            .create();
        let (e, declared) = declare_and_start(&api(&server), DB, &json!({})).unwrap_err();
        assert!(declared.is_none());
        assert!(e.message().contains("identical rollup"), "{}", e.message());
    }

    #[test]
    fn a_locked_table_is_waited_for() {
        let mut server = mockito::Server::new();
        let locked = server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .with_status(409)
            .with_header("retry-after", "0")
            .with_body(
                json!({"error": {"code": "RESOURCE_LOCKED", "message": "busy; retry shortly"}})
                    .to_string(),
            )
            .expect(2)
            .create();
        let declared = server
            .mock("POST", format!("/v1/databases/{DB}/rollups").as_str())
            .with_status(201)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": "jobs1"}).to_string(),
            )
            .create();

        let build = declare_and_start(&api(&server), DB, &json!({})).unwrap();
        locked.assert();
        declared.assert();
        assert_eq!(build.id(), "rlup1");
    }

    // --- rebuild ---

    fn mock_rebuild_conflict(server: &mut mockito::Server) -> mockito::Mock {
        server
            .mock(
                "POST",
                format!("/v1/databases/{DB}/rollups/rlup1/rebuild").as_str(),
            )
            .with_status(409)
            .with_body(conflict("the rollup cannot be rebuilt now"))
            .create()
    }

    #[test]
    fn rebuilding_a_ready_rollup_is_not_an_error() {
        let mut server = mockito::Server::new();
        mock_rebuild_conflict(&mut server);
        mock_get(&mut server, "rlup1", "ready", 1);
        let start = start_build(&api(&server), DB, "rlup1").unwrap();
        assert!(matches!(start, BuildStart::AlreadyReady(_)), "{start:?}");
    }

    #[test]
    fn rebuilding_a_rollup_that_is_building_joins_that_build() {
        let mut server = mockito::Server::new();
        mock_rebuild_conflict(&mut server);
        mock_get(&mut server, "rlup1", "building", 1);
        let start = start_build(&api(&server), DB, "rlup1").unwrap();
        assert!(matches!(start, BuildStart::AlreadyBuilding(_)), "{start:?}");
    }

    #[test]
    fn rebuilding_a_rollup_being_dropped_stays_the_servers_refusal() {
        let mut server = mockito::Server::new();
        mock_rebuild_conflict(&mut server);
        mock_get(&mut server, "rlup1", "dropping", 1);
        let e = start_build(&api(&server), DB, "rlup1").unwrap_err();
        assert!(e.message().contains("cannot be rebuilt"), "{}", e.message());
    }

    #[test]
    fn a_started_rebuild_carries_its_job() {
        let mut server = mockito::Server::new();
        server
            .mock(
                "POST",
                format!("/v1/databases/{DB}/rollups/rlup1/rebuild").as_str(),
            )
            .with_status(202)
            .with_body(
                json!({"rollup": rollup_json("rlup1", "building"), "job_id": "jobs9"}).to_string(),
            )
            .create();
        match start_build(&api(&server), DB, "rlup1").unwrap() {
            BuildStart::Started(build) => assert_eq!(build.job_id.as_deref(), Some("jobs9")),
            other => panic!("expected a started build, got {other:?}"),
        }
    }

    // --- the wait ---

    /// Run the wait without sleeping, returning its outcome and its pauses.
    fn wait(server: &mockito::Server) -> (Result<Built, ApiError>, Vec<Duration>) {
        let mut pauses = Vec::new();
        let outcome = wait_until_built(&api(server), DB, "rlup1", |d| pauses.push(d), |_| {});
        (outcome, pauses)
    }

    #[test]
    fn the_wait_ends_when_the_rollup_is_ready() {
        let mut server = mockito::Server::new();
        let building = mock_get(&mut server, "rlup1", "building", 2);
        let ready = mock_get(&mut server, "rlup1", "ready", 1);
        let (outcome, pauses) = wait(&server);
        building.assert();
        ready.assert();
        assert!(matches!(outcome, Ok(Built::Ready(_))), "{outcome:?}");
        assert_eq!(pauses, vec![POLL_FAST, POLL_FAST]);
    }

    #[test]
    fn the_wait_reports_a_failed_build_with_its_reason() {
        let mut server = mockito::Server::new();
        mock_get(&mut server, "rlup1", "building", 1);
        let mut failed = rollup_json("rlup1", "build_failed");
        failed["error"] = json!("a single time bucket needs more memory than the build can use");
        server
            .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
            .with_status(200)
            .with_body(failed.to_string())
            .create();
        match wait(&server).0 {
            Ok(Built::Failed(rollup)) => {
                assert!(rollup["error"].as_str().unwrap().contains("more memory"))
            }
            other => panic!("expected a failed build, got {other:?}"),
        }
    }

    #[test]
    fn the_wait_ends_when_the_table_is_replaced_or_the_rollup_goes() {
        for (status, want_dropped) in [("needs_rebuild", false), ("dropping", true)] {
            let mut server = mockito::Server::new();
            mock_get(&mut server, "rlup1", status, 1);
            let outcome = wait(&server).0;
            match (outcome, want_dropped) {
                (Ok(Built::NeedsRebuild), false) | (Ok(Built::Dropped), true) => {}
                (other, _) => panic!("{status}: unexpected {other:?}"),
            }
        }

        let mut server = mockito::Server::new();
        server
            .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
            .with_status(404)
            .with_body(r#"{"error":{"code":"NOT_FOUND","message":"no such rollup"}}"#)
            .create();
        assert!(matches!(wait(&server).0, Ok(Built::Dropped)));
    }

    #[test]
    fn the_wait_rides_out_a_few_failed_reads() {
        let mut server = mockito::Server::new();
        let unavailable = server
            .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
            .with_status(503)
            .with_body("unavailable")
            .expect(MAX_FAILED_POLLS as usize)
            .create();
        let ready = mock_get(&mut server, "rlup1", "ready", 1);
        let (outcome, _) = wait(&server);
        unavailable.assert();
        ready.assert();
        assert!(matches!(outcome, Ok(Built::Ready(_))), "{outcome:?}");
    }

    #[test]
    fn the_wait_rides_out_a_rate_limit_and_a_request_timeout() {
        for status in [429, 408] {
            let mut server = mockito::Server::new();
            let limited = server
                .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
                .with_status(status)
                .with_body("slow down")
                .expect(2)
                .create();
            let ready = mock_get(&mut server, "rlup1", "ready", 1);
            let (outcome, pauses) = wait(&server);
            limited.assert();
            ready.assert();
            assert!(
                matches!(outcome, Ok(Built::Ready(_))),
                "{status}: {outcome:?}"
            );
            assert_eq!(pauses.len(), 2);
        }
    }

    #[test]
    fn the_wait_gives_up_after_too_many_failed_reads_in_a_row() {
        let mut server = mockito::Server::new();
        let unavailable = server
            .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
            .with_status(503)
            .with_body("unavailable")
            .expect(MAX_FAILED_POLLS as usize + 1)
            .create();
        let (outcome, _) = wait(&server);
        unavailable.assert();
        assert!(
            matches!(&outcome, Err(ApiError::Status { status, .. }) if status.as_u16() == 503),
            "{outcome:?}"
        );
    }

    #[test]
    fn the_wait_stops_at_once_on_a_refused_credential() {
        let mut server = mockito::Server::new();
        let forbidden = server
            .mock("GET", format!("/v1/databases/{DB}/rollups/rlup1").as_str())
            .with_status(403)
            .with_body(r#"{"error":{"code":"ACCESS_DENIED","message":"no"}}"#)
            .expect(1)
            .create();
        let (outcome, pauses) = wait(&server);
        forbidden.assert();
        assert!(outcome.is_err());
        assert!(pauses.is_empty());
    }

    #[test]
    fn the_wait_keeps_going_through_a_status_it_does_not_know() {
        let mut server = mockito::Server::new();
        let unknown = mock_get(&mut server, "rlup1", "compacting", 1);
        mock_get(&mut server, "rlup1", "ready", 1);
        let (outcome, _) = wait(&server);
        unknown.assert();
        assert!(matches!(outcome, Ok(Built::Ready(_))), "{outcome:?}");
    }

    #[test]
    fn the_wait_slows_down_after_the_first_minute() {
        let mut server = mockito::Server::new();
        let fast_reads = (POLL_FAST_FOR.as_secs() / POLL_FAST.as_secs()) as usize;
        mock_get(&mut server, "rlup1", "building", fast_reads + 2);
        mock_get(&mut server, "rlup1", "ready", 1);
        let (_, pauses) = wait(&server);
        assert_eq!(pauses.len(), fast_reads + 2);
        assert!(pauses[..fast_reads].iter().all(|p| *p == POLL_FAST));
        assert!(pauses[fast_reads..].iter().all(|p| *p == POLL_SLOW));
    }

    // --- the window ---

    fn mock_window(server: &mut mockito::Server, id: &str, status: usize) -> mockito::Mock {
        let body = if status == 200 {
            rollup_json(id, "ready").to_string()
        } else {
            conflict("the rollup's table was replaced")
        };
        server
            .mock(
                "PUT",
                format!("/v1/databases/{DB}/rollups/{id}/window").as_str(),
            )
            .match_body(mockito::Matcher::Json(json!({"window_secs": 21600})))
            .with_status(status)
            .with_body(body)
            .create()
    }

    #[test]
    fn the_window_is_set_through_a_rollup_that_can_carry_it() {
        let mut server = mockito::Server::new();
        let set = mock_window(&mut server, "rlup3", 200);
        let rollups = vec![
            rollup_json("rlup1", "dropping"),
            rollup_json("rlup2", "needs_rebuild"),
            rollup_json("rlup3", "building"),
        ];
        let rollup = set_table_window(&api(&server), DB, &rollups, 21_600).unwrap();
        set.assert();
        assert_eq!(rollup["id"], "rlup3");
    }

    #[test]
    fn the_window_moves_on_from_a_rollup_that_refuses_it() {
        let mut server = mockito::Server::new();
        let refused = mock_window(&mut server, "rlup1", 409);
        let set = mock_window(&mut server, "rlup2", 200);
        let rollups = vec![rollup_json("rlup1", "ready"), rollup_json("rlup2", "ready")];
        set_table_window(&api(&server), DB, &rollups, 21_600).unwrap();
        refused.assert();
        set.assert();
    }

    #[test]
    fn the_window_says_why_it_could_not_be_set() {
        let server = mockito::Server::new();
        assert!(matches!(
            set_table_window(&api(&server), DB, &[], 60),
            Err(WindowError::NoRollups)
        ));
        let stuck = vec![rollup_json("rlup1", "needs_rebuild")];
        assert!(matches!(
            set_table_window(&api(&server), DB, &stuck, 60),
            Err(WindowError::NoneSettable)
        ));

        let mut server = mockito::Server::new();
        mock_window(&mut server, "rlup1", 409);
        let rollups = vec![rollup_json("rlup1", "ready")];
        match set_table_window(&api(&server), DB, &rollups, 21_600) {
            Err(WindowError::Api(e)) => {
                assert!(e.message().contains("was replaced"), "{}", e.message())
            }
            other => panic!("expected the server's refusal, got {other:?}"),
        }
    }

    // --- list and show ---

    #[test]
    fn list_sends_its_filters_and_returns_the_rollups_as_sent() {
        let mut server = mockito::Server::new();
        let mut rollup = rollup_json("rlup1", "ready");
        rollup["added_later"] = json!("kept");
        let listed = server
            .mock("GET", format!("/v1/databases/{DB}/rollups").as_str())
            .match_query(mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("schema_name".into(), "public".into()),
                mockito::Matcher::UrlEncoded("table_name".into(), "requests".into()),
            ]))
            .with_status(200)
            .with_body(json!({"rollups": [rollup]}).to_string())
            .create();
        let rollups = list_rollups(&api(&server), DB, Some("public"), Some("requests")).unwrap();
        listed.assert();
        // A field this build has never heard of still reaches `-o json`.
        assert_eq!(rollups[0]["added_later"], "kept");
    }

    #[test]
    fn a_rollup_reads_through_statuses_and_kinds_added_later() {
        let mut value = rollup_json("rlup1", "compacting");
        value["measures"] = json!([{"kind": "approx_distinct", "column": "user_id"}]);
        let rollup = Rollup::from_value(&value).unwrap();
        let row = list_row(&rollup);
        assert_eq!(row[5], "approx_distinct:user_id");
        assert_eq!(row[2], "compacting");
    }

    #[test]
    fn a_list_row_shows_durations_and_measures_readably() {
        let rollup = Rollup::from_value(&rollup_json("rlup1", "ready")).unwrap();
        assert_eq!(
            list_row(&rollup),
            vec![
                "rlup1",
                "public.requests",
                "ready",
                "1h",
                "region, status",
                "count, numeric:bytes, approx_percentile:latency_ms",
                "2026-11-18 16:00",
            ]
        );
    }

    #[test]
    fn show_explains_a_failed_build_and_how_to_resume_it() {
        let mut value = rollup_json("rlup1", "build_failed");
        value["error"] = json!("out of memory");
        value["failed_at"] = json!("2026-11-18T11:00:00Z");
        let lines = show_lines(&Rollup::from_value(&value).unwrap()).join("\n");
        assert!(lines.contains("error:        out of memory"), "{lines}");
        assert!(
            lines.contains("failed at:    2026-11-18T11:00:00Z"),
            "{lines}"
        );
        assert!(lines.contains("hotdata rollups rebuild rlup1"), "{lines}");
    }

    #[test]
    fn show_hides_stale_progress_once_the_table_is_replaced() {
        let mut value = rollup_json("rlup1", "needs_rebuild");
        value["window_secs"] = Value::Null;
        let rollup = Rollup::from_value(&value).unwrap();
        let lines = show_lines(&rollup).join("\n");
        assert!(!lines.contains("built until"), "{lines}");
        assert!(lines.contains("window:       -"), "{lines}");
        assert!(lines.contains("hotdata rollups rebuild rlup1"), "{lines}");
        assert_eq!(list_row(&rollup)[6], "-");
    }

    #[test]
    fn show_prints_the_filter_only_when_there_is_one() {
        let mut value = rollup_json("rlup1", "ready");
        let without = show_lines(&Rollup::from_value(&value).unwrap()).join("\n");
        assert!(!without.contains("filter:"), "{without}");
        value["filter"] = json!("status <> 'error'");
        let with = show_lines(&Rollup::from_value(&value).unwrap()).join("\n");
        assert!(with.contains("filter:       status <> 'error'"), "{with}");
    }

    #[test]
    fn the_write_note_names_the_table_the_window_and_the_time_column() {
        let note = write_note(&Rollup::from_value(&rollup_json("rlup1", "ready")).unwrap());
        assert!(note.contains("public.requests"), "{note}");
        assert!(note.contains("(1h)"), "{note}");
        assert!(note.contains("`ts`"), "{note}");
    }
}
