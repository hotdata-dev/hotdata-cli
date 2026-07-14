//! `hotdata ingest` — pull data from external sources into a managed database.
//!
//! Two nouns, named explicitly in every command: **datasources** (added,
//! credentialed sources — schema discovered, no data loaded) and **imports**
//! (managed DBs materialized from a datasource). Commands: `new-datasource`,
//! `list-datasources`, `datasources` (the catalog of available types),
//! `new-import` (--all or
//! SQL), `list-imports`, `trigger-import` (re-run: refresh an import's DB
//! from source), `status` (one-shot or `--wait` attach). The 0.13
//! `*-connection` verbs survive as hidden aliases (the noun collided with
//! the federated `hotdata connections` store).
//!
//! **Imports don't block.** `new-import` and `trigger-import` submit, print
//! the import id, and return; progress is tracked with `status <id>
//! [--wait]` or `list-imports`. `--wait` opts into the old blocking poll.
//! `new-datasource` is the deliberate exception — it blocks by default
//! because its output IS the feedback (credentials validated, schema
//! discovered); `--no-wait` opts out.
//!
//! `new-datasource` **adds a datasource and discovers its schema — it loads
//! no data**. It runs the guided wizard when no `--service` is given on a
//! terminal, and is flag-driven otherwise (`--service` given, a non-TTY, or
//! `--no-input`) — one command, two front doors. Pulling rows is the
//! separate, explicit `new-import` step, read back through the core
//! `query`/`databases`/`results` commands.
//!
//! The wizard is catalog-driven: the `/ingest/connectors` catalog returns a
//! `template` per REST service with everything but the `<PLACEHOLDER>`
//! secrets filled in, so the user only supplies those. Submitting needs a
//! durable `hd_` API key (`--api-key`/`HOTDATA_API_KEY`); results land in
//! the authenticated workspace — there is no destination flag by design.
//!
//! **Presentation contract:** user-facing output speaks datasource / import
//! / database, never the service's job vocabulary. `status` is a closed set
//! (pending | running | done | failed); the service's finer progress states
//! ride in `stage`. JSON output projects the same user-facing fields (`id`,
//! `name`, `type`, `status`, `stage`, …), not the wire rows.

use crate::client::ingest::{
    ConnectorEntry, IngestAck, IngestClient, IngestRequest, JobStatus, QueryToIngest,
};
use crate::commands::prompt::{
    ask_secret, ask_text, optional, optional_default, prompt_list, select_optional,
};
use crate::util;
use inquire::{Select, Text};
use std::time::{Duration, Instant};

/// Status-poll cadence (what the hotdlt UI uses). Each poll is a service-side
/// read, so this is deliberately not sub-second.
const POLL_INTERVAL: Duration = Duration::from_millis(2_500);

/// A run kicked immediately after submitting can miss the new row (the
/// service's reads lag writes briefly). If a job sits pending this long,
/// kick processing again.
const DRAIN_REKICK_AFTER: Duration = Duration::from_secs(30);

/// Render a value for `-o json|yaml`, or fall through to the human branch.
/// One definition so the json-println / yaml-print convention cannot drift
/// between the (many) commands that support all three formats.
fn render<T: serde::Serialize>(output: &str, value: &T, human: impl FnOnce()) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(value).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(value).unwrap()),
        _ => human(),
    }
}

/// Run `f` under a spinner, clearing it before either returning the value or
/// printing the error — the clear-before-exit invariant lives here instead of
/// being copy-discipline at every call site.
fn with_spinner<T>(
    msg: &str,
    f: impl FnOnce() -> Result<T, crate::client::ingest::IngestError>,
) -> T {
    let spinner = util::spinner(msg);
    match f() {
        Ok(v) => {
            spinner.finish_and_clear();
            v
        }
        Err(e) => {
            spinner.finish_and_clear();
            e.exit()
        }
    }
}

#[derive(clap::Subcommand)]
pub enum IngestCommands {
    /// Add a datasource and discover its schema (loads no data)
    ///
    /// Interactive by default; pass `--service` with config flags to add
    /// non-interactively. Pull rows separately with `hotdata ingest
    /// new-import`; browse available types with `hotdata ingest datasources`.
    #[command(alias = "new-connection")]
    NewDatasource {
        #[command(flatten)]
        create: CreateArgs,

        #[command(flatten)]
        wait: WaitArgs,
    },

    /// Show a datasource's details: status and discovered tables + columns
    #[command(alias = "show-connection")]
    ShowDatasource {
        /// Datasource id (from `list-datasources`)
        id: String,
    },

    /// Delete a datasource: its stored credentials and discovered schema
    ///
    /// Imports you've already run keep working — each carries its own
    /// credential copy.
    #[command(alias = "delete-connection")]
    DeleteDatasource {
        /// Datasource id (from `list-datasources`)
        id: String,

        /// Keep the datasource's schema-preview database
        #[arg(long = "keep-database", hide = true)]
        keep_database: bool,
    },

    /// List the datasources you've added (each has its own datasource id)
    #[command(alias = "list-connections")]
    ListDatasources {
        /// Include datasources replaced by a newer one of the same name
        #[arg(long)]
        all: bool,
    },

    /// Browse the catalog of available datasource types
    #[command(alias = "connectors")]
    Datasources {
        /// Filter to entries whose name contains this text
        name: Option<String>,
    },

    /// Import data from a datasource into a managed database (--all or SQL)
    NewImport {
        /// SELECT <cols|*> FROM <datasource>[.<table>] [WHERE …] [LIMIT n]
        sql: Option<String>,

        /// Import everything from --source (SELECT * with no LIMIT)
        #[arg(long, conflicts_with = "sql")]
        all: bool,

        /// Datasource to import from: a name (used in FROM) or a datasource
        /// id from `list-datasources` (pins resolution)
        #[arg(long)]
        source: Option<String>,

        /// Reuse an existing managed database (by id) instead of minting one
        #[arg(long = "database-id", hide = true)]
        database_id: Option<String>,

        #[command(flatten)]
        poll: PollArgs,
    },

    /// List your imports: the SQL behind each and the database it landed in
    ListImports,

    /// Re-run an import: refresh its database from the source
    ///
    /// The same managed database is refreshed using the stored credentials —
    /// nothing is re-entered. Also accepts a datasource id: re-checks the
    /// credentials and refreshes the discovered schema.
    TriggerImport {
        /// Import id from `list-imports`, or a datasource id from
        /// `list-datasources`
        id: String,

        #[command(flatten)]
        poll: PollArgs,
    },

    /// Show an import's or datasource's status
    ///
    /// One-shot by default with script-friendly exit codes: 0 done,
    /// 1 failed, 2 still in flight. `--wait` attaches and polls to a
    /// terminal state instead.
    Status {
        /// Import or datasource id (from `new-import`, `list-imports`, or
        /// `list-datasources`)
        id: String,

        #[command(flatten)]
        poll: PollArgs,
    },

    /// Cancel a running or pending import
    ///
    /// Marks the import cancelled immediately. The dltHub drain may still be
    /// running and will finish on its own — use `status <id> --wait` to see
    /// when it settles. To retry after cancellation, use `trigger-import <id>`
    /// once the status reaches `failed`.
    Cancel {
        /// Import or datasource id to cancel
        id: String,
    },
}

/// Wait flags shared by the non-blocking commands (`new-import`,
/// `trigger-import`, `status`): one definition so the default timeout and
/// help text cannot drift between them.
#[derive(clap::Args)]
pub struct PollArgs {
    /// Block until completion (default: return immediately; track with
    /// `hotdata ingest status <id> --wait`)
    #[arg(long)]
    wait: bool,

    /// Seconds to wait for completion with --wait (default 300)
    #[arg(long = "wait-timeout", default_value = "300")]
    wait_timeout: u64,
}

/// Wait flags for `new-datasource` — the one submit command that still
/// blocks by default, because its output IS the feedback: did the
/// credentials work, and what schema was discovered. The import commands
/// return immediately and are tracked with `ingest status`.
#[derive(clap::Args)]
pub struct WaitArgs {
    /// Submit only; print the datasource id and return without waiting
    #[arg(long = "no-wait")]
    no_wait: bool,

    /// Seconds to wait for completion before giving up (default 300)
    #[arg(long = "wait-timeout", default_value = "300")]
    wait_timeout: u64,
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: IngestCommands) {
    match command {
        IngestCommands::NewDatasource { create, wait } => {
            add_datasource(workspace_id, output, create, &wait)
        }
        IngestCommands::ShowDatasource { id } => show_datasource(workspace_id, output, &id),
        IngestCommands::DeleteDatasource { id, keep_database } => {
            delete_datasource(workspace_id, output, &id, keep_database)
        }
        IngestCommands::ListDatasources { all } => list_datasources(workspace_id, output, all),
        IngestCommands::Datasources { name } => catalog_list(workspace_id, name.as_deref(), output),
        IngestCommands::NewImport {
            sql,
            all,
            source,
            database_id,
            poll,
        } => new_import(workspace_id, output, sql, all, source, database_id, &poll),
        IngestCommands::ListImports => list_imports(workspace_id, output),
        IngestCommands::TriggerImport { id, poll } => {
            trigger_import(workspace_id, output, &id, &poll)
        }
        IngestCommands::Status { id, poll } => status(workspace_id, output, &id, &poll),
        IngestCommands::Cancel { id } => cancel_import(workspace_id, output, &id),
    }
}

// --- presentation helpers --------------------------------------------------

/// `status` as shown to users is a CLOSED set: pending | running | done |
/// failed. Anything else the service reports is a finer in-flight stage —
/// presented as `running` with the raw value demoted to the stage slot.
/// (New servers already split status/stage; this also covers older ones.)
fn normalize_status(raw: &str) -> (&'static str, Option<&str>) {
    match raw {
        "pending" => ("pending", None),
        "running" => ("running", None),
        "done" => ("done", None),
        "failed" => ("failed", None),
        "cancelled" => ("cancelled", None),
        stage => ("running", Some(stage)),
    }
}

/// The (status, stage) pair for a job row: the server's `stage` field wins,
/// a stage-shaped `status` from an older server is the fallback.
fn presented_status(status: &str, stage: Option<&str>) -> (String, Option<String>) {
    let (normalized, fallback) = normalize_status(status);
    (
        normalized.to_string(),
        stage.map(str::to_string).or(fallback.map(str::to_string)),
    )
}

/// STATUS cell for the human tables/details: the normalized status, with the
/// in-flight stage in parentheses when there is one.
fn status_cell(status: &str, stage: Option<&str>) -> String {
    let (normalized, stage) = presented_status(status, stage);
    let colored = util::color_status(&normalized);
    match stage {
        Some(s) => format!("{colored} ({s})"),
        None => colored,
    }
}

/// Display label for a wire family value. Datasource types read as product
/// nouns (SQL, buckets, API), not protocol jargon.
fn family_label(family: &str) -> &str {
    match family {
        "sql" => "SQL",
        "filesystem" => "buckets",
        "rest" => "API",
        other => other,
    }
}

/// Machine value for a wire family (`type` in json/yaml output): the same
/// product nouns, lowercased for scripting.
fn family_type(family: &str) -> &str {
    match family {
        "filesystem" => "buckets",
        "rest" => "api",
        other => other,
    }
}

/// A datasource name is the FROM target for imports, so it must be a bare
/// SQL-identifier-shaped word (the server enforces the same rule).
fn valid_datasource_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 64
        && matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn checked_name(name: Option<String>) -> Option<String> {
    if let Some(n) = name.as_deref()
        && !valid_datasource_name(n)
    {
        fail(
            "datasource names are letters, digits, and underscores (not starting \
             with a digit) — they are the FROM target for imports",
        );
    }
    name
}

/// The user-facing name of a job row: the datasource name, falling back to
/// the connector for rows created before names existed.
fn display_name(name: Option<&str>, connector_type: Option<&str>) -> Option<String> {
    name.or(connector_type).map(str::to_string)
}

/// The user-facing json/yaml view of a job status. One projection shared by
/// `status`, `show-datasource`, and the poll terminal states, so machine
/// output never reverts to wire rows.
fn status_json(st: &JobStatus) -> serde_json::Value {
    let (status, stage) = presented_status(&st.status, st.stage.as_deref());
    serde_json::json!({
        "id": st.ingest_id,
        "name": display_name(st.name.as_deref(), st.connector_type.as_deref()),
        "connector": st.connector_type,
        "type": st.family.as_deref().map(family_type),
        "status": status,
        "stage": stage,
        "detail": st.detail,
        "database_id": st.database_id,
        "created_at": st.created_at,
        "updated_at": st.updated_at,
    })
}

fn family_rank(family: &str) -> u8 {
    match family {
        "sql" => 0,
        "filesystem" => 1,
        "iceberg" => 2,
        _ => 3, // rest services
    }
}

/// Sort the catalog for display: generic families (sql, filesystem, iceberg)
/// first, then the REST services, each group alphabetical. Redundant SQL
/// dialect aliases are collapsed at the source (the catalog), not here.
fn sorted_for_display(entries: &[ConnectorEntry]) -> Vec<ConnectorEntry> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| {
        family_rank(&a.family)
            .cmp(&family_rank(&b.family))
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
}

// --- new-datasource: wizard or flag-driven --------------------------------

/// `ingest new-datasource`. The guided wizard runs when no connector was
/// named and we're on a terminal; otherwise (a `--service` was given, or a
/// non-TTY / `--no-input` run) it's flag-driven and needs `--service`.
fn add_datasource(workspace_id: &str, output: &str, args: CreateArgs, wait: &WaitArgs) {
    if args.service.is_none() && util::is_interactive() {
        if args.any_given() {
            // The wizard prompts from scratch and would silently ignore
            // every provided flag — refuse instead of discarding input.
            fail(
                "--service is required when other flags are given (e.g. --service postgres); \
                 run plain 'hotdata ingest new-datasource' for the guided wizard",
            );
        }
        wizard(workspace_id, output, checked_name(args.name), wait);
    } else {
        add_from_flags(workspace_id, output, args, wait);
    }
}

fn wizard(workspace_id: &str, output: &str, name: Option<String>, wait: &WaitArgs) {
    let client = IngestClient::new(workspace_id);
    let entries = fetch_catalog(&client);
    let entry = select_connector(&entries);

    let mut req = match entry.family.as_str() {
        "sql" => build_sql_interactive(&entry),
        "filesystem" => build_filesystem_interactive(&entry),
        "iceberg" => build_iceberg_interactive(&entry),
        _ => build_rest_interactive(&entry),
    };
    // A --name given up front is honored; otherwise offer one (blank keeps
    // the connector name, which is what most people want).
    req.name =
        checked_name(name.or_else(|| optional(None, "Name (blank to use the connector name):")));
    // Adding a datasource discovers the schema only — never loads data.
    req.validate_only = true;
    run_source(&client, output, wait.no_wait, wait.wait_timeout, req);
}

/// Present the catalog as a filterable menu and return the chosen entry.
/// Families are grouped (generic sql/filesystem/iceberg first, then the REST
/// services) and inquire's typeahead narrows the ~150 entries as the user types.
fn select_connector(entries: &[ConnectorEntry]) -> ConnectorEntry {
    let sorted = sorted_for_display(entries);
    let labels: Vec<String> = sorted
        .iter()
        .map(|c| {
            if c.description.is_empty() {
                format!("{}  ({})", c.name, family_label(&c.family))
            } else {
                format!(
                    "{}  ({}) — {}",
                    c.name,
                    family_label(&c.family),
                    c.description
                )
            }
        })
        .collect();
    let selected = Select::new("Source:", labels.clone())
        .with_page_size(15)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    let idx = labels.iter().position(|l| l == &selected).unwrap();
    sorted[idx].clone()
}

fn build_sql_interactive(entry: &ConnectorEntry) -> IngestRequest {
    // `entry.name` is the dialect (postgres, mysql, …) — we know the shape, so
    // prompt only the connection fields, defaulting the port from the dialect.
    let mut m = serde_json::Map::new();
    m.insert("host".into(), ask_text("Host:").into());
    let default_port = default_port_for(&entry.name);
    let mut port_prompt = Text::new("Port:");
    if let Some(dp) = default_port {
        port_prompt = port_prompt.with_default(dp);
    }
    let port = port_prompt
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    if let Ok(n) = port.trim().parse::<u16>() {
        m.insert("port".into(), n.into());
    }
    m.insert("username".into(), ask_text("User:").into());
    let pw = ask_secret("Password (blank if none):");
    if !pw.is_empty() {
        m.insert("password".into(), pw.into());
    }
    m.insert("database".into(), ask_text("Database:").into());

    let schema = optional(None, "Schema (blank for all):");
    let tables = prompt_list("Tables (comma-separated, blank for all):");

    IngestRequest {
        family: "sql".into(),
        connector_type: Some(entry.name.clone()),
        credentials: serde_json::Value::Object(m),
        schema,
        table_names: tables,
        ..Default::default()
    }
}

fn build_filesystem_interactive(entry: &ConnectorEntry) -> IngestRequest {
    let bucket_url = ask_text("Bucket URL (e.g. s3://bucket/prefix):");
    let format = select_optional("File format:", &["parquet", "csv", "jsonl"]);
    let glob = optional(None, "File glob (e.g. **/*.parquet, blank for all):");
    IngestRequest {
        family: "filesystem".into(),
        // connector_type is what `new-import` resolves FROM when no name is
        // chosen — without it the datasource lands unnamed and un-importable.
        connector_type: Some(entry.name.clone()),
        credentials: filesystem_credentials(),
        bucket_url: Some(bucket_url),
        file_glob: glob,
        file_format: format,
        ..Default::default()
    }
}

fn build_iceberg_interactive(entry: &ConnectorEntry) -> IngestRequest {
    let catalog_type = optional_default("Catalog type:", "rest");
    let tables = prompt_list("Tables (namespace.table, comma-separated):");
    IngestRequest {
        family: "iceberg".into(),
        // connector_type is what `new-import` resolves FROM when no name is
        // chosen — without it the datasource lands unnamed and un-importable.
        connector_type: Some(entry.name.clone()),
        catalog_name: Some(entry.name.clone()),
        catalog_type,
        catalog_config: Some(iceberg_catalog_config()),
        tables,
        ..Default::default()
    }
}

/// REST family. A cataloged service carries a `template` with everything but
/// the secrets filled in — walk it, prompt only the `<PLACEHOLDER>` tokens, and
/// send the result. The bare `api` entry (no template) falls back to building
/// a minimal config interactively.
fn build_rest_interactive(entry: &ConnectorEntry) -> IngestRequest {
    match &entry.template {
        Some(template) => {
            let filled = fill_template(template);
            IngestRequest {
                family: "rest".into(),
                connector_type: Some(entry.name.clone()),
                rest_config: Some(filled),
                ..Default::default()
            }
        }
        None => {
            let name = ask_text("Datasource name (the FROM target for `ingest new-import`):");
            IngestRequest {
                family: "rest".into(),
                connector_type: Some(name),
                rest_config: Some(rest_config()),
                ..Default::default()
            }
        }
    }
}

// --- template placeholder filling ----------------------------------------

/// Collect the distinct `<PLACEHOLDER>` tokens in a template, in first-seen
/// order. A placeholder is a whole string value wrapped in angle brackets
/// (`<CLIENT_ID>`) — the shape the connector catalog uses.
fn collect_placeholders(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::String(s) if is_placeholder(s) => {
            if !out.contains(s) {
                out.push(s.clone());
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|i| collect_placeholders(i, out)),
        serde_json::Value::Object(m) => m.values().for_each(|i| collect_placeholders(i, out)),
        _ => {}
    }
}

fn is_placeholder(s: &str) -> bool {
    s.len() > 2 && s.starts_with('<') && s.ends_with('>')
}

/// A placeholder names a secret when its token looks like one — prompt those
/// with hidden input.
fn is_secret_placeholder(token: &str) -> bool {
    let t = token.to_ascii_uppercase();
    ["SECRET", "TOKEN", "KEY", "PASSWORD", "PASS"]
        .iter()
        .any(|needle| t.contains(needle))
}

/// Prompt for each placeholder in `template` (secrets hidden) and substitute
/// every occurrence, returning the filled config.
fn fill_template(template: &serde_json::Value) -> serde_json::Value {
    let mut tokens = Vec::new();
    collect_placeholders(template, &mut tokens);
    let mut filled = template.clone();
    for token in tokens {
        let label = format!(
            "{}:",
            token
                .trim_matches(|c| c == '<' || c == '>')
                .replace('_', " ")
                .to_lowercase()
        );
        let value = if is_secret_placeholder(&token) {
            ask_secret(&label)
        } else {
            ask_text(&label)
        };
        substitute_placeholder(&mut filled, &token, &value);
    }
    filled
}

fn substitute_placeholder(v: &mut serde_json::Value, token: &str, value: &str) {
    match v {
        serde_json::Value::String(s) if s == token => {
            *v = serde_json::Value::String(value.to_string());
        }
        serde_json::Value::Array(a) => a
            .iter_mut()
            .for_each(|i| substitute_placeholder(i, token, value)),
        serde_json::Value::Object(m) => m
            .values_mut()
            .for_each(|i| substitute_placeholder(i, token, value)),
        _ => {}
    }
}

// --- new-datasource (flag-driven): non-interactive add --------------------

#[derive(clap::Args)]
pub struct CreateArgs {
    /// Connector to add (a catalog name: postgres, bitcoin, buckets, …).
    /// Given → non-interactive; omit on a terminal → guided wizard.
    #[arg(long)]
    service: Option<String>,

    /// Name for the datasource — the FROM target for imports (default: the
    /// connector name). Lets several datasources of one connector coexist.
    #[arg(long)]
    name: Option<String>,

    /// Connector config as JSON (inline, @file.json, or @-). Field reference:
    /// `hotdata ingest datasources -o json` (each entry's config_schema)
    #[arg(long)]
    config: Option<String>,

    /// Restrict discovery to this table (repeatable; SQL/iceberg)
    #[arg(long = "table")]
    tables: Vec<String>,

    /// Schema to discover (SQL)
    #[arg(long)]
    schema: Option<String>,

    /// Bucket URL, e.g. s3://bucket/prefix (buckets)
    #[arg(long = "bucket-url")]
    bucket_url: Option<String>,

    /// File format (buckets)
    #[arg(long, value_parser = ["csv", "jsonl", "parquet"])]
    format: Option<String>,

    /// File glob, e.g. **/*.parquet (buckets)
    #[arg(long)]
    glob: Option<String>,

    /// Catalog type, e.g. rest (iceberg)
    #[arg(long = "catalog-type")]
    catalog_type: Option<String>,

    /// Reuse an existing managed database (by id) instead of minting one
    #[arg(long = "database-id", hide = true)]
    database_id: Option<String>,
}

impl CreateArgs {
    /// Any flag beyond `--service`/`--name` — the wizard never reads these,
    /// so entering it with any of them set would silently discard user input.
    /// (`--name` IS honored by the wizard, so it doesn't count.)
    fn any_given(&self) -> bool {
        self.config.is_some()
            || !self.tables.is_empty()
            || self.schema.is_some()
            || self.bucket_url.is_some()
            || self.format.is_some()
            || self.glob.is_some()
            || self.catalog_type.is_some()
            || self.database_id.is_some()
    }
}

fn add_from_flags(workspace_id: &str, output: &str, args: CreateArgs, wait: &WaitArgs) {
    let Some(service) = args.service.as_deref() else {
        eprintln!(
            "error: --service is required to add a datasource non-interactively. \
             Browse available types with 'hotdata ingest datasources', or run \
             'hotdata ingest new-datasource' in a terminal for the guided wizard."
        );
        std::process::exit(1);
    };
    let client = IngestClient::new(workspace_id);
    let entries = fetch_catalog(&client);
    let entry = entries
        .iter()
        .find(|c| c.name == service)
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("error: unknown connector '{service}'. Run 'hotdata ingest datasources'.");
            std::process::exit(1);
        });

    let config = args.config.as_deref().map(parse_config_arg);
    let req = build_create_request(&entry, args, config).unwrap_or_else(|msg| fail(&msg));
    run_source(&client, output, wait.no_wait, wait.wait_timeout, req);
}

/// Flag-driven request construction for one catalog entry. Pure (config is
/// pre-parsed, errors are returned) so the per-family mapping — the part a
/// server-side 422 would otherwise be the first to catch — is unit-testable.
fn build_create_request(
    entry: &ConnectorEntry,
    args: CreateArgs,
    config: Option<serde_json::Value>,
) -> Result<IngestRequest, String> {
    if let Some(n) = args.name.as_deref()
        && !valid_datasource_name(n)
    {
        return Err(
            "datasource names are letters, digits, and underscores (not starting \
             with a digit) — they are the FROM target for imports"
                .into(),
        );
    }
    let mut req = match entry.family.as_str() {
        "sql" => IngestRequest {
            family: "sql".into(),
            connector_type: Some(entry.name.clone()),
            credentials: config.ok_or(
                "SQL connectors need --config with credentials (a connection_string or host/user/…)",
            )?,
            schema: args.schema,
            table_names: args.tables,
            ..Default::default()
        },
        "filesystem" => IngestRequest {
            family: "filesystem".into(),
            // Same as the wizard path: named, so imports can resolve it.
            connector_type: Some(entry.name.clone()),
            credentials: config.unwrap_or(serde_json::Value::Null),
            bucket_url: Some(args.bucket_url.ok_or("buckets connectors need --bucket-url")?),
            file_glob: args.glob,
            file_format: args.format,
            ..Default::default()
        },
        "iceberg" => IngestRequest {
            family: "iceberg".into(),
            // Same as the wizard path: named, so imports can resolve it.
            connector_type: Some(entry.name.clone()),
            catalog_name: Some(entry.name.clone()),
            catalog_type: args.catalog_type.or_else(|| Some("rest".into())),
            catalog_config: Some(config.ok_or("iceberg needs --config with the catalog fields")?),
            tables: args.tables,
            ..Default::default()
        },
        _ => {
            // REST: an explicit --config wins; otherwise the catalog template,
            // which only works untouched for keyless services.
            let rest_config = config
                .or_else(|| entry.template.clone())
                .ok_or("this API connector needs --config (client + resources)")?;
            let mut leftover = Vec::new();
            collect_placeholders(&rest_config, &mut leftover);
            if !leftover.is_empty() {
                return Err(format!(
                    "connector '{}' needs secrets ({}) — pass a filled --config, or use 'hotdata ingest new-datasource'",
                    entry.name,
                    leftover.join(", ")
                ));
            }
            IngestRequest {
                family: "rest".into(),
                connector_type: Some(entry.name.clone()),
                rest_config: Some(rest_config),
                ..Default::default()
            }
        }
    };
    // Adding a datasource discovers the schema only — never loads data.
    req.validate_only = true;
    req.name = args.name;
    req.database_id = args.database_id;
    Ok(req)
}

fn catalog_list(workspace_id: &str, filter: Option<&str>, output: &str) {
    let client = IngestClient::new(workspace_id);
    let mut entries = fetch_catalog(&client);
    if let Some(f) = filter {
        let f = f.to_lowercase();
        entries.retain(|c| c.name.to_lowercase().contains(&f));
    }
    let entries = sorted_for_display(&entries);
    // "added" = a connector this workspace already has a datasource for.
    // Best-effort: if the read fails the catalog still shows, just without
    // the marks. Login sessions are rejected on every workspace-scoped read
    // (module docs in client/ingest) — don't pay a doomed round-trip.
    let added = if client.has_api_key() {
        added_connector_names(&client)
    } else {
        Default::default()
    };
    let is_added = |name: &str| added.contains(name);

    let projected: Vec<_> = entries
        .iter()
        .map(|c| {
            serde_json::json!({
                "name": c.name,
                "type": family_type(&c.family),
                "added": is_added(&c.name),
                "description": c.description,
                "config_schema": c.config_schema,
            })
        })
        .collect();
    render(output, &projected, || {
        let rows: Vec<Vec<String>> = entries
            .iter()
            .map(|c| {
                let status = if is_added(&c.name) { "added" } else { "" };
                vec![
                    c.name.clone(),
                    family_label(&c.family).to_string(),
                    status.to_string(),
                    c.description.clone(),
                ]
            })
            .collect();
        crate::output::table::print(&["NAME", "TYPE", "STATUS", "DESCRIPTION"], &rows);
    });
}

/// Connector names this workspace has datasources for, used to flag the
/// catalog's "added" marks. Best-effort — an unavailable listing yields an
/// empty set.
fn added_connector_names(client: &IngestClient) -> std::collections::HashSet<String> {
    client
        .list_sources(false)
        .map(|r| {
            r.sources
                .into_iter()
                .filter_map(|s| s.connector_type)
                .collect()
        })
        .unwrap_or_default()
}

// --- new-import: SQL front-door -------------------------------------------

fn new_import(
    workspace_id: &str,
    output: &str,
    sql: Option<String>,
    all: bool,
    source: Option<String>,
    database_id: Option<String>,
    poll: &PollArgs,
) {
    // A 32-char hex --source is a datasource id (pins resolution); anything
    // else is a datasource name (the FROM target).
    let (pin, name) = match source {
        Some(s) if looks_like_ingest_id(&s) => (Some(s), None),
        Some(s) => (None, Some(s)),
        None => (None, None),
    };

    let client = IngestClient::new(workspace_id);
    // `--all` against a pinned id needs the datasource's name for FROM —
    // resolve it, only when actually needed.
    let pinned_name = (all && name.is_none())
        .then(|| pin.as_deref().and_then(|p| pinned_source_name(&client, p)))
        .flatten();
    // A pin that fails to resolve is ITS OWN error — falling through to
    // build_import_query would blame the --source flag the user did pass.
    if all
        && name.is_none()
        && pinned_name.is_none()
        && let Some(p) = pin.as_deref()
    {
        fail(&format!(
            "could not resolve datasource id {p} — check 'hotdata ingest list-datasources'"
        ));
    }
    let query = build_import_query(sql, all, name.as_deref(), pinned_name.as_deref())
        .unwrap_or_else(|msg| fail(msg));
    let req = QueryToIngest {
        query,
        source_ingest_id: pin,
        database_id,
    };
    let spinner = util::spinner("submitting import…");
    // The by-name FROM lookup reads snapshots that lag writes, so an import
    // right after adding a datasource can 404 briefly. Retry ONCE: the lag is
    // usually under one poll interval, and every extra retry makes a typo'd
    // datasource name look like a slow service for 2.5s more.
    let mut ack = client.create_query(&req);
    for _ in 0..1 {
        match &ack {
            Err(crate::client::ingest::IngestError::Http { status: 404, .. }) => {
                spinner.set_message("waiting for the datasource to appear…");
                std::thread::sleep(POLL_INTERVAL);
                ack = client.create_query(&req);
            }
            _ => break,
        }
    }
    let ack = ack.unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();
    if poll.wait {
        let st = poll_ingest(
            &client,
            output,
            &ack.ingest_id,
            poll.wait_timeout,
            "importing",
            true,
        );
        render_done(&st, output);
        return;
    }
    // Non-blocking default: the service processes on demand, so kick the run
    // that actually executes the job — without it the import would sit
    // pending — then hand the terminal back.
    let _ = client.drain();
    render_ack(&ack, "import id:", output);
}

fn looks_like_ingest_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The SQL a `new-import` runs: explicit SQL wins; `--all` becomes a full
/// `SELECT *` against the datasource's name (`pinned_name` when --source was
/// a datasource id). Errors are messages for `fail`.
fn build_import_query(
    sql: Option<String>,
    all: bool,
    name: Option<&str>,
    pinned_name: Option<&str>,
) -> Result<String, &'static str> {
    match (sql, all) {
        (Some(q), _) => Ok(q), // clap rejects sql + --all together
        (None, true) => name
            .or(pinned_name)
            .map(|n| format!("SELECT * FROM {n}"))
            .ok_or("--all needs --source <datasource> (a name or datasource id)"),
        (None, false) => Err(
            "provide SQL (SELECT … FROM <datasource> …), or --all to import \
             everything from --source",
        ),
    }
}

/// Resolve a pinned datasource id to its name (the FROM target).
/// GET /jobs/{id} returns it directly — no need to page the full listing.
fn pinned_source_name(client: &IngestClient, ingest_id: &str) -> Option<String> {
    let st = client.job_status(ingest_id).ok()?;
    display_name(st.name.as_deref(), st.connector_type.as_deref())
}

// --- trigger-import --------------------------------------------------------

fn trigger_import(workspace_id: &str, output: &str, id: &str, poll: &PollArgs) {
    let client = IngestClient::new(workspace_id);
    // The service resets the item to pending and runs it again (409 if a run
    // is already in progress — its message says to retry shortly).
    let ack = with_spinner("requesting re-run…", || client.rerun(id));
    if poll.wait {
        // The re-run has already been kicked; poll only.
        let st = poll_ingest(
            &client,
            output,
            &ack.ingest_id,
            poll.wait_timeout,
            "re-running",
            false,
        );
        render_done(&st, output);
        return;
    }
    render_ack(&ack, "import id:", output);
}

// --- status ------------------------------------------------------------------

/// Exit code for a one-shot status check, mirroring `query status`:
/// 0 done, 1 failed, 2 still in flight (pending/running).
fn status_exit_code(status: &str) -> i32 {
    match normalize_status(status).0 {
        "done" => 0,
        "failed" | "cancelled" => 1,
        _ => 2,
    }
}

fn status(workspace_id: &str, output: &str, id: &str, poll: &PollArgs) {
    let client = IngestClient::new(workspace_id);
    if poll.wait {
        // Attach and poll to a terminal state. No initial kick — the submit
        // already fired one — but the poll loop still re-kicks a job stuck in
        // `pending`, so attaching also rescues an import whose first run
        // missed it.
        let st = poll_ingest(&client, output, id, poll.wait_timeout, "waiting", false);
        render_done(&st, output);
        return;
    }
    let st = client.job_status(id).unwrap_or_else(|e| e.exit());
    render(output, &status_json(&st), || {
        use crossterm::style::Stylize;
        let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
        println!(
            "{}{}",
            label("status:"),
            status_cell(&st.status, st.stage.as_deref())
        );
        if let Some(d) = st.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            println!("{}{}", label("detail:"), d);
        }
        if let Some(db) = st.database_id.as_deref() {
            println!("{}{}", label("database:"), db);
        }
        if let Some(t) = st.updated_at.as_deref() {
            println!("{}{}", label("updated:"), t);
        }
        if status_exit_code(&st.status) == 2 {
            println!(
                "{}",
                format!("Attach with: {}", status_wait_hint(id)).dark_grey()
            );
        }
    });
    std::process::exit(status_exit_code(&st.status));
}

// --- cancel ----------------------------------------------------------------

fn cancel_import(workspace_id: &str, output: &str, id: &str) {
    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("cancelling…", || client.cancel(id));
    let machine = serde_json::json!({
        "ingest_id": ack.ingest_id,
        "status": ack.status,
        "detail": ack.detail,
        "database_id": ack.database_id,
    });
    render(output, &machine, || {
        use crossterm::style::Stylize;
        let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
        println!("{}{}", label("status:"), util::color_status(&ack.status));
        if let Some(db) = ack.database_id.as_deref() {
            println!("{}{}", label("database:"), db);
        }
        println!(
            "{}",
            format!(
                "Drain may still be running — track with: hotdata ingest status {} --wait",
                ack.ingest_id
            )
            .dark_grey()
        );
    });
}

// --- list-datasources ------------------------------------------------------

fn list_datasources(workspace_id: &str, output: &str, all: bool) {
    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("loading datasources…", || client.list_sources(all));

    let projected: Vec<_> = resp
        .sources
        .iter()
        .map(|s| {
            let (status, stage) = presented_status(&s.status, s.stage.as_deref());
            serde_json::json!({
                "id": s.ingest_id,
                "name": display_name(s.name.as_deref(), s.connector_type.as_deref()),
                "connector": s.connector_type,
                "type": s.family.as_deref().map(family_type),
                "status": status,
                "stage": stage,
                "detail": s.detail,
                "database_id": s.database_id,
                "created_at": s.created_at,
                "updated_at": s.updated_at,
                "active": s.active,
            })
        })
        .collect();
    render(output, &projected, || {
        use crossterm::style::Stylize;
        if resp.sources.is_empty() {
            eprintln!(
                "{}",
                "No datasources yet. Add one with 'hotdata ingest new-datasource'.".dark_grey()
            );
            return;
        }
        let mut headers = vec!["NAME", "TYPE", "STATUS", "CREATED", "DATASOURCE ID"];
        if all {
            headers.push("ACTIVE");
        }
        let rows: Vec<Vec<String>> = resp
            .sources
            .iter()
            // Oldest at the top, newest at the bottom — the freshest row
            // lands next to the prompt. (The server returns newest-first;
            // json/yaml keep that order for scripting.)
            .rev()
            .map(|s| {
                let mut row = vec![
                    display_name(s.name.as_deref(), s.connector_type.as_deref())
                        .unwrap_or_else(|| "-".into()),
                    s.family
                        .as_deref()
                        .map(family_label)
                        .unwrap_or_default()
                        .to_string(),
                    status_cell(&s.status, s.stage.as_deref()),
                    created_cell(s.created_at.as_deref()),
                    s.ingest_id.clone(),
                ];
                if all {
                    row.push(if s.active {
                        "yes".into()
                    } else {
                        String::new()
                    });
                }
                row
            })
            .collect();
        crate::output::table::print(&headers, &rows);
    });
}

// --- list-imports ----------------------------------------------------------

fn list_imports(workspace_id: &str, output: &str) {
    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("loading imports…", || client.list_queries());

    let projected: Vec<_> = resp
        .queries
        .iter()
        .map(|q| {
            let (status, stage) = presented_status(&q.status, q.stage.as_deref());
            serde_json::json!({
                "id": q.ingest_id,
                "datasource": display_name(q.name.as_deref(), q.connector_type.as_deref()),
                "sql": q.query,
                "status": status,
                "stage": stage,
                "detail": q.detail,
                "database_id": q.database_id,
                "created_at": q.created_at,
                "updated_at": q.updated_at,
            })
        })
        .collect();
    render(output, &projected, || {
        use crossterm::style::Stylize;
        if resp.queries.is_empty() {
            eprintln!(
                "{}",
                "No imports yet. Create one with 'hotdata ingest new-import \
                     --source <datasource> --all' (or pass SQL)."
                    .dark_grey()
            );
            return;
        }
        let rows: Vec<Vec<String>> = resp
            .queries
            .iter()
            // Oldest at the top, newest at the bottom (see list_datasources).
            .rev()
            .map(|q| {
                vec![
                    q.ingest_id.clone(),
                    display_name(q.name.as_deref(), q.connector_type.as_deref())
                        .unwrap_or_else(|| "-".into()),
                    q.query.clone().unwrap_or_default(),
                    status_cell(&q.status, q.stage.as_deref()),
                    created_cell(q.created_at.as_deref()),
                    q.database_id.clone().unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();
        crate::output::table::print(
            &[
                "IMPORT ID",
                "DATASOURCE",
                "SQL",
                "STATUS",
                "CREATED",
                "DATABASE",
            ],
            &rows,
        );
    });
}

/// CREATED cell for the listing tables — util::format_date, aligned with
/// every other table in the CLI ("2026-07-08 10:12").
fn created_cell(ts: Option<&str>) -> String {
    ts.map(util::format_date).unwrap_or_else(|| "-".into())
}

// --- shared run + poll ----------------------------------------------------

fn run_source(
    client: &IngestClient,
    output: &str,
    no_wait: bool,
    wait_timeout: u64,
    req: IngestRequest,
) {
    // The first add in a workspace provisions the runtime (~15-30s); later
    // ones are quick. The HTTP client allows 300s.
    let ack = with_spinner(
        "adding datasource… (the first one in a workspace takes ~30s)",
        || client.create_source(&req),
    );
    if no_wait {
        // The service processes on demand: without this kick the datasource
        // would sit pending until some other command fires one.
        let _ = client.drain();
        render_ack(&ack, "datasource id:", output);
        return;
    }
    let st = poll_ingest(
        client,
        output,
        &ack.ingest_id,
        wait_timeout,
        "discovering schema",
        true,
    );
    render_datasource_added(client, &st, output);
}

/// The canonical "track this" invocation. Every hint goes through here so
/// the next verb rename has ONE string to update, not ten.
fn status_wait_hint(ingest_id: &str) -> String {
    format!("hotdata ingest status {ingest_id} --wait")
}

/// Timeout exit for a wait: code 2 = still in flight, distinct from 1 = the
/// run itself failed. One place so every wait path agrees.
fn poll_timeout_exit(ingest_id: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!("{}", "timed out waiting".red());
    eprintln!(
        "{}",
        format!("Keep tracking it with: {}", status_wait_hint(ingest_id)).dark_grey()
    );
    std::process::exit(2);
}

/// Poll to a terminal state, returning the final (done) status for the
/// caller to render. `kick` fires processing up front (submit paths;
/// `trigger-import` skips it — the re-run already did). Every in-flight
/// stage is shown live in the spinner (stage + detail); exits the process on
/// failure (1) or timeout (2). On failure with `-o json|yaml` the projected
/// status object (with the server's detail) still lands on stdout, matching
/// the one-shot `status` path.
fn poll_ingest(
    client: &IngestClient,
    output: &str,
    ingest_id: &str,
    timeout_secs: u64,
    verb: &str,
    kick: bool,
) -> JobStatus {
    if kick {
        let _ = client.drain();
    }
    let mut last_drain = Instant::now();
    let mut consecutive_errors: u32 = 0;

    let spinner = util::spinner(&format!("{verb}…"));
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        // A status poll is read-only and cheap to retry — one transient
        // gateway error (a 403/5xx blip, a dropped connection) must not kill
        // a wait that is otherwise progressing. Only persistent failure exits.
        let st = match client.job_status(ingest_id) {
            Ok(st) => {
                consecutive_errors = 0;
                st
            }
            Err(e) => {
                consecutive_errors += 1;
                if consecutive_errors >= 3 {
                    spinner.finish_and_clear();
                    e.exit();
                }
                // The deadline outranks the retry budget: a transient blip AT
                // the deadline is a timeout (exit 2 — the job may well still
                // be running), not a failure (exit 1).
                if Instant::now() > deadline {
                    spinner.finish_and_clear();
                    poll_timeout_exit(ingest_id);
                }
                std::thread::sleep(POLL_INTERVAL);
                continue;
            }
        };
        match st.status.as_str() {
            "done" => {
                spinner.finish_and_clear();
                return st;
            }
            "failed" | "cancelled" => {
                spinner.finish_and_clear();
                // Machine formats get the projected status object (with the
                // server's detail) on stdout, matching one-shot `status`.
                render(output, &status_json(&st), || {
                    use crossterm::style::Stylize;
                    let label = st.status.as_str();
                    let detail = st.detail.as_deref().unwrap_or("unknown error");
                    eprintln!("{}", format!("{label}: {detail}").red());
                    if detail.contains("Forbidden") {
                        eprintln!(
                            "{}",
                            "Forbidden at load time usually means a database-scoped API token — \
                                 ingest needs a regular workspace API token."
                                .dark_grey()
                        );
                    }
                });
                std::process::exit(1);
            }
            // Anything else is in flight. Surface the live stage + detail in
            // the spinner rather than treating it as terminal.
            raw => {
                let (_, stage) = presented_status(raw, st.stage.as_deref());
                spinner.set_message(progress_message(
                    verb,
                    stage.as_deref().unwrap_or(raw),
                    st.detail.as_deref(),
                ));
                // Re-kick processing if the job never left `pending` — the
                // submit-time kick can race the new row (a harmless double
                // run; loads replace).
                if raw == "pending" {
                    if last_drain.elapsed() > DRAIN_REKICK_AFTER {
                        let _ = client.drain();
                        last_drain = Instant::now();
                    }
                } else {
                    last_drain = Instant::now(); // progress seen — reset the clock
                }
            }
        }
        if Instant::now() > deadline {
            spinner.finish_and_clear();
            poll_timeout_exit(ingest_id);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Spinner text for an in-progress poll: the verb plus the current stage,
/// and its free-text detail when present (e.g. row counts / table name).
fn progress_message(verb: &str, stage: &str, detail: Option<&str>) -> String {
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{verb}… {stage} — {d}"),
        None => format!("{verb}… {stage}"),
    }
}

// --- rendering -----------------------------------------------------------

fn render_ack(ack: &IngestAck, id_label: &str, output: &str) {
    let machine = serde_json::json!({
        "id": ack.ingest_id,
        "database_id": ack.database_id,
        "status": ack.status,
    });
    render(output, &machine, || {
        use crossterm::style::Stylize;
        let label = |l: &str| format!("{:<15}", l).dark_grey().to_string();
        println!("{}{}", label(id_label), ack.ingest_id);
        println!("{}{}", label("status:"), ack.status.as_str().yellow());
        println!(
            "{}",
            format!(
                "Track it with: hotdata ingest status {} --wait  (or: hotdata ingest list-imports)",
                ack.ingest_id
            )
            .dark_grey()
        );
    });
}

fn render_done(st: &JobStatus, output: &str) {
    render(output, &status_json(st), || {
        use crossterm::style::Stylize;
        let db = st.database_id.as_deref().unwrap_or("-");
        println!("{} → {}", "done".green(), db);
        println!(
            "{}",
            format!("Query it: hotdata query --database {db} \"SELECT * FROM …\"").dark_grey()
        );
    });
}

/// The user-facing json/yaml view of a datasource with its discovered
/// schema: the status projection plus `tables`.
fn datasource_json(st: &JobStatus, tables: Option<&serde_json::Value>) -> serde_json::Value {
    let mut v = status_json(st);
    v["tables"] = tables.cloned().unwrap_or(serde_json::Value::Null);
    v
}

/// Fetch the schema-preview of a datasource: the raw
/// `{"tables": {name: [columns]}}` value, when it exists and answers.
fn discovered_tables(client: &IngestClient, st: &JobStatus) -> Option<serde_json::Value> {
    let db = st.database_id.as_deref().filter(|d| !d.is_empty())?;
    client.schema(db).ok()?.get("tables").cloned()
}

/// Human rendering of the discovered schema: one line per table with its
/// column names. Shared by `new-datasource` and `show-datasource`.
fn print_discovered_tables(tables_value: Option<&serde_json::Value>) {
    use crossterm::style::Stylize;
    match tables_value.and_then(|t| t.as_object()) {
        Some(t) if !t.is_empty() => {
            println!(
                "{}",
                format!("discovered {} table(s):", t.len()).dark_grey()
            );
            for (name, cols) in t {
                let names: Vec<&str> = cols.as_array().map_or(Vec::new(), |a| {
                    a.iter().filter_map(|c| c.as_str()).collect()
                });
                println!(
                    "  {}  {}",
                    name.as_str().cyan(),
                    names.join(", ").dark_grey()
                );
            }
        }
        _ => println!("{}", "no tables discovered".dark_grey()),
    }
}

/// Render the result of adding a datasource: the discovered schema (tables +
/// columns). No data was loaded — the closing hint points at `new-import`
/// for that. The schema-preview database id stays out of the human view
/// (it's plumbing); `-o json` carries it as `database_id`.
fn render_datasource_added(client: &IngestClient, st: &JobStatus, output: &str) {
    let tables_value = discovered_tables(client, st);

    render(output, &datasource_json(st, tables_value.as_ref()), || {
        use crossterm::style::Stylize;
        let source = display_name(st.name.as_deref(), st.connector_type.as_deref())
            .unwrap_or_else(|| "source".into());
        println!(
            "{} {}",
            "datasource added".green(),
            source.as_str().dark_grey()
        );
        print_discovered_tables(tables_value.as_ref());
        println!(
            "{}",
            format!("Import data with: hotdata ingest new-import --source {source} --all (or SQL)")
                .dark_grey()
        );
    });
}

// --- show-datasource / delete-datasource ------------------------------------

fn show_datasource(workspace_id: &str, output: &str, id: &str) {
    let client = IngestClient::new(workspace_id);
    let st = client.job_status(id).unwrap_or_else(|e| e.exit());
    let tables_value = discovered_tables(&client, &st);

    render(output, &datasource_json(&st, tables_value.as_ref()), || {
        use crossterm::style::Stylize;
        let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
        println!(
            "{}{}",
            label("name:"),
            display_name(st.name.as_deref(), st.connector_type.as_deref())
                .unwrap_or_else(|| "-".into())
        );
        if let Some(f) = st.family.as_deref() {
            println!("{}{}", label("type:"), family_label(f));
        }
        println!(
            "{}{}",
            label("status:"),
            status_cell(&st.status, st.stage.as_deref())
        );
        if let Some(d) = st.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            println!("{}{}", label("detail:"), d);
        }
        if let Some(t) = st.created_at.as_deref() {
            println!("{}{}", label("created:"), util::format_date(t));
        }
        if let Some(t) = st.updated_at.as_deref() {
            println!("{}{}", label("updated:"), util::format_date(t));
        }
        print_discovered_tables(tables_value.as_ref());
    });
}

fn delete_datasource(workspace_id: &str, output: &str, id: &str, keep_database: bool) {
    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("deleting datasource…", || client.delete_source(id));

    // Drop the schema-preview database first so the ack (json/yaml) is the
    // last thing on stdout for scripting.
    match ack.database_id.as_deref() {
        Some(db) if !keep_database => crate::commands::databases::delete(workspace_id, db),
        Some(db) => {
            use crossterm::style::Stylize;
            println!(
                "{}",
                format!("kept database {db} (--keep-database)").dark_grey()
            );
        }
        None => {}
    }
    let machine = serde_json::json!({
        "id": ack.ingest_id,
        "name": display_name(ack.name.as_deref(), ack.connector_type.as_deref()),
        "deleted": ack.deleted,
    });
    render(output, &machine, || {
        use crossterm::style::Stylize;
        println!(
            "{} {}",
            "datasource deleted".green(),
            display_name(ack.name.as_deref(), ack.connector_type.as_deref())
                .unwrap_or_else(|| id.to_string())
                .as_str()
                .dark_grey()
        );
    });
}

// --- catalog fetch --------------------------------------------------------

fn fetch_catalog(client: &IngestClient) -> Vec<ConnectorEntry> {
    with_spinner("loading datasource types…", || client.connectors()).connectors
}

/// Bare `api` connector: build a minimal API config interactively
/// (base_url + optional bearer token + resource paths).
fn rest_config() -> serde_json::Value {
    let base_url = ask_text("API base URL:");
    let token = ask_secret("Bearer token (blank if none):");
    let resources: Vec<serde_json::Value> = ask_text("Resource paths (comma-separated):")
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(serde_json::Value::from)
        .collect();
    let mut client = serde_json::Map::new();
    client.insert("base_url".into(), base_url.into());
    if !token.is_empty() {
        client.insert(
            "auth".into(),
            serde_json::json!({ "type": "bearer", "token": token }),
        );
    }
    serde_json::json!({ "client": client, "resources": resources })
}

/// Bucket credentials: prompt for S3-style creds (all optional — blank
/// access key = public bucket, no creds sent).
fn filesystem_credentials() -> serde_json::Value {
    if !util::is_interactive() {
        return serde_json::Value::Null;
    }
    let key = ask_text("Object-store access key id (blank for a public bucket):");
    if key.trim().is_empty() {
        return serde_json::Value::Null;
    }
    let mut m = serde_json::Map::new();
    m.insert("aws_access_key_id".into(), key.into());
    m.insert(
        "aws_secret_access_key".into(),
        ask_secret("Secret access key:").into(),
    );
    if let Some(ep) = optional(None, "Endpoint URL (S3-compatible; blank for AWS):") {
        m.insert("endpoint_url".into(), ep.into());
    }
    if let Some(region) = optional(None, "Region (blank for default):") {
        m.insert("region_name".into(), region.into());
    }
    serde_json::Value::Object(m)
}

/// Iceberg catalog connection config: prompt for catalog URI + warehouse +
/// token + namespace. Field reference: the `iceberg` entry's config_schema
/// in `hotdata ingest datasources -o json`.
fn iceberg_catalog_config() -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("uri".into(), ask_text("Catalog URI:").into());
    if let Some(w) = optional(None, "Warehouse (blank if none):") {
        m.insert("warehouse".into(), w.into());
    }
    let token = ask_secret("Catalog token (blank if none):");
    if !token.is_empty() {
        m.insert("token".into(), token.into());
    }
    if let Some(ns) = optional(None, "Namespace (blank if none):") {
        m.insert("namespace".into(), ns.into());
    }
    serde_json::Value::Object(m)
}

/// Parse a `--config` argument: inline JSON, `@file.json`, or `@-` for stdin.
fn parse_config_arg(arg: &str) -> serde_json::Value {
    use std::io::Read;
    let raw = if arg == "@-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .unwrap_or_else(|e| fail(&format!("--config reading stdin: {e}")));
        s
    } else if let Some(path) = arg.strip_prefix('@') {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| fail(&format!("--config reading {path}: {e}")))
    } else {
        arg.to_string()
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| fail(&format!("--config invalid JSON: {e}")))
}

fn default_port_for(dialect: &str) -> Option<&'static str> {
    match dialect {
        "postgres" | "postgresql" | "redshift" => Some("5432"),
        "mysql" | "mariadb" => Some("3306"),
        "mssql" | "sqlserver" => Some("1433"),
        "oracle" => Some("1521"),
        _ => None,
    }
}

fn fail(msg: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!("{}", format!("error: {msg}").red());
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_collected_in_order_without_dupes() {
        let t = serde_json::json!({
            "client": {
                "base_url": "https://x/api",
                "auth": {"client_id": "<CLIENT_ID>", "client_secret": "<CLIENT_SECRET>"}
            },
            "resources": ["<CLIENT_ID>", "teams"]
        });
        let mut got = Vec::new();
        collect_placeholders(&t, &mut got);
        assert_eq!(got, vec!["<CLIENT_ID>", "<CLIENT_SECRET>"]);
    }

    #[test]
    fn keyless_template_has_no_placeholders() {
        let t = serde_json::json!({
            "client": {"base_url": "https://blockstream.info/api"},
            "resources": [{"name": "blocks", "endpoint": {"path": "blocks"}}]
        });
        let mut got = Vec::new();
        collect_placeholders(&t, &mut got);
        assert!(got.is_empty());
    }

    #[test]
    fn substitute_replaces_every_occurrence_only_for_that_token() {
        let mut t = serde_json::json!({
            "a": "<TOK>", "b": {"c": "<TOK>"}, "d": "<OTHER>", "e": "literal"
        });
        substitute_placeholder(&mut t, "<TOK>", "filled");
        assert_eq!(t["a"], "filled");
        assert_eq!(t["b"]["c"], "filled");
        assert_eq!(t["d"], "<OTHER>"); // untouched
        assert_eq!(t["e"], "literal");
    }

    #[test]
    fn secret_placeholders_detected_by_token_name() {
        assert!(is_secret_placeholder("<CLIENT_SECRET>"));
        assert!(is_secret_placeholder("<API_TOKEN>"));
        assert!(is_secret_placeholder("<ACCESS_KEY>"));
        assert!(is_secret_placeholder("<PASSWORD>"));
        assert!(!is_secret_placeholder("<CLIENT_ID>"));
        assert!(!is_secret_placeholder("<ACCOUNT>"));
    }

    #[test]
    fn ingest_id_recognized_only_for_32_hex() {
        assert!(looks_like_ingest_id("6232a1694a1b4451957c053a56756ff7"));
        assert!(!looks_like_ingest_id("bitcoin"));
        assert!(!looks_like_ingest_id("6232a169")); // too short
        assert!(!looks_like_ingest_id("zzzz1694a1b4451957c053a56756ffff")); // non-hex
    }

    #[test]
    fn import_query_explicit_sql_wins() {
        let q = build_import_query(
            Some("SELECT id FROM pg.orders LIMIT 5".into()),
            false,
            Some("pg"),
            None,
        );
        assert_eq!(q.unwrap(), "SELECT id FROM pg.orders LIMIT 5");
    }

    #[test]
    fn import_query_all_uses_the_datasource_name() {
        // Named source.
        assert_eq!(
            build_import_query(None, true, Some("bitcoin"), None).unwrap(),
            "SELECT * FROM bitcoin"
        );
        // Pinned datasource id — name resolved by the caller.
        assert_eq!(
            build_import_query(None, true, None, Some("postgres")).unwrap(),
            "SELECT * FROM postgres"
        );
    }

    #[test]
    fn import_query_errors_without_sql_or_all() {
        // Neither SQL nor --all: the two modes must be chosen explicitly.
        let err = build_import_query(None, false, Some("bitcoin"), None).unwrap_err();
        assert!(err.contains("--all"), "got: {err}");
        // --all with no resolvable source name.
        let err = build_import_query(None, true, None, None).unwrap_err();
        assert!(err.contains("--source"), "got: {err}");
    }

    fn create_args() -> CreateArgs {
        CreateArgs {
            service: None,
            name: None,
            config: None,
            tables: Vec::new(),
            schema: None,
            bucket_url: None,
            format: None,
            glob: None,
            catalog_type: None,
            database_id: None,
        }
    }

    #[test]
    fn create_request_sql_requires_and_carries_credentials() {
        let e = entry("postgres", "sql");
        assert!(
            build_create_request(&e, create_args(), None)
                .unwrap_err()
                .contains("--config")
        );

        let creds = serde_json::json!({"connection_string": "postgresql://u:p@h/db"});
        let mut args = create_args();
        args.schema = Some("tpch_sf1".into());
        args.tables = vec!["region".into()];
        args.database_id = Some("db_1".into());
        args.name = Some("prod_pg".into());
        let req = build_create_request(&e, args, Some(creds.clone())).unwrap();
        assert_eq!(req.family, "sql");
        assert_eq!(req.connector_type.as_deref(), Some("postgres"));
        assert_eq!(req.credentials, creds);
        assert_eq!(req.schema.as_deref(), Some("tpch_sf1"));
        assert_eq!(req.table_names, vec!["region"]);
        // Never loads data; the name and db-reuse flag ride through.
        assert!(req.validate_only);
        assert_eq!(req.name.as_deref(), Some("prod_pg"));
        assert_eq!(req.database_id.as_deref(), Some("db_1"));
    }

    #[test]
    fn create_request_rejects_invalid_names() {
        let e = entry("postgres", "sql");
        let creds = serde_json::json!({"connection_string": "postgresql://u:p@h/db"});
        for bad in ["prod pg", "1pg", "pg-prod", ""] {
            let mut args = create_args();
            args.name = Some(bad.into());
            let err = build_create_request(&e, args, Some(creds.clone())).unwrap_err();
            assert!(err.contains("datasource names"), "{bad}: {err}");
        }
    }

    #[test]
    fn create_request_filesystem_and_iceberg_required_fields() {
        let fs = entry("buckets", "filesystem");
        assert!(
            build_create_request(&fs, create_args(), None)
                .unwrap_err()
                .contains("--bucket-url")
        );
        let mut fs_args = create_args();
        fs_args.bucket_url = Some("s3://bucket/prefix".into());
        let req = build_create_request(&fs, fs_args, None).unwrap();
        assert_eq!(req.bucket_url.as_deref(), Some("s3://bucket/prefix"));
        // Registered under the connector name, or new-import can't FROM it.
        assert_eq!(req.connector_type.as_deref(), Some("buckets"));

        let ice = entry("iceberg", "iceberg");
        assert!(
            build_create_request(&ice, create_args(), None)
                .unwrap_err()
                .contains("--config")
        );
        let mut args = create_args();
        args.tables = vec!["ns.t".into()];
        let req = build_create_request(&ice, args, Some(serde_json::json!({"uri": "u"}))).unwrap();
        // The catalog type defaults to rest when not specified.
        assert_eq!(req.catalog_type.as_deref(), Some("rest"));
        assert_eq!(req.tables, vec!["ns.t"]);
        // Registered under the connector name, or new-import can't FROM it.
        assert_eq!(req.connector_type.as_deref(), Some("iceberg"));
    }

    #[test]
    fn create_request_rest_template_placeholders_fail_fast() {
        let mut e = entry("aikido", "rest");
        e.template = Some(serde_json::json!({
            "client": {"auth": {"client_id": "<CLIENT_ID>"}}
        }));
        let err = build_create_request(&e, create_args(), None).unwrap_err();
        assert!(err.contains("<CLIENT_ID>"), "got: {err}");

        // Keyless template (no placeholders) passes through verbatim.
        let mut keyless = entry("bitcoin", "rest");
        keyless.template = Some(serde_json::json!({"client": {"base_url": "https://x"}}));
        let req = build_create_request(&keyless, create_args(), None).unwrap();
        assert_eq!(req.family, "rest");
        assert!(req.rest_config.is_some());
    }

    #[test]
    fn status_exit_codes_close_over_stage_states() {
        assert_eq!(status_exit_code("done"), 0);
        assert_eq!(status_exit_code("failed"), 1);
        // Everything else is in flight — including stage states from older
        // servers that report them through `status`.
        assert_eq!(status_exit_code("pending"), 2);
        assert_eq!(status_exit_code("running"), 2);
        assert_eq!(status_exit_code("loading"), 2);
    }

    #[test]
    fn statuses_normalize_to_the_closed_set() {
        assert_eq!(normalize_status("done"), ("done", None));
        assert_eq!(normalize_status("pending"), ("pending", None));
        // Old servers report stages through status — presented as running.
        assert_eq!(
            normalize_status("extracting"),
            ("running", Some("extracting"))
        );
        // A server-provided stage field wins over the fallback.
        assert_eq!(
            presented_status("running", Some("loading")),
            ("running".into(), Some("loading".into()))
        );
        assert_eq!(
            presented_status("normalizing", None),
            ("running".into(), Some("normalizing".into()))
        );
    }

    #[test]
    fn datasource_names_are_identifier_shaped() {
        assert!(valid_datasource_name("prod_pg"));
        assert!(valid_datasource_name("_x2"));
        assert!(!valid_datasource_name("2fast"));
        assert!(!valid_datasource_name("prod pg"));
        assert!(!valid_datasource_name("prod-pg"));
        assert!(!valid_datasource_name(""));
    }

    #[test]
    fn created_cell_matches_repo_date_format() {
        assert_eq!(
            created_cell(Some("2026-07-08T10:12:00+00:00")),
            "2026-07-08 10:12"
        );
        assert_eq!(created_cell(None), "-");
    }

    #[test]
    fn family_rank_orders_generic_before_rest() {
        assert!(family_rank("sql") < family_rank("rest"));
        assert!(family_rank("filesystem") < family_rank("rest"));
        assert!(family_rank("iceberg") < family_rank("rest"));
    }

    #[test]
    fn family_labels_and_types_read_as_product_nouns() {
        assert_eq!(family_label("sql"), "SQL");
        assert_eq!(family_label("filesystem"), "buckets");
        assert_eq!(family_label("rest"), "API");
        assert_eq!(family_label("iceberg"), "iceberg");
        // Machine values: same nouns, scripting-cased.
        assert_eq!(family_type("sql"), "sql");
        assert_eq!(family_type("filesystem"), "buckets");
        assert_eq!(family_type("rest"), "api");
        assert_eq!(family_type("iceberg"), "iceberg");
    }

    #[test]
    fn status_json_projects_user_fields_not_wire_fields() {
        let st = JobStatus {
            ingest_id: "a".repeat(32),
            status: "extracting".into(),
            detail: Some("orders".into()),
            name: None,
            stage: None,
            connector_type: Some("postgres".into()),
            family: Some("sql".into()),
            database_id: Some("db-1".into()),
            created_at: None,
            updated_at: Some("2026-07-09T10:00:00+00:00".into()),
        };
        let v = status_json(&st);
        assert_eq!(v["id"], "a".repeat(32));
        assert_eq!(v["name"], "postgres"); // falls back to the connector
        assert_eq!(v["type"], "sql");
        assert_eq!(v["status"], "running"); // normalized...
        assert_eq!(v["stage"], "extracting"); // ...with the stage split out
        assert!(v.get("ingest_id").is_none());
        assert!(v.get("family").is_none());
    }

    #[test]
    fn parse_config_accepts_inline_json() {
        let v = parse_config_arg(r#"{"connection_string": "postgresql://u:p@h/db"}"#);
        assert_eq!(v["connection_string"], "postgresql://u:p@h/db");
    }

    fn entry(name: &str, family: &str) -> ConnectorEntry {
        ConnectorEntry {
            name: name.into(),
            family: family.into(),
            description: String::new(),
            auth: None,
            template: None,
            config_schema: None,
        }
    }

    #[test]
    fn sorted_for_display_groups_generic_families_before_rest() {
        let entries = vec![
            entry("stripe", "rest"),
            entry("postgres", "sql"),
            entry("buckets", "filesystem"),
            entry("aikido", "rest"),
            entry("iceberg", "iceberg"),
        ];
        let names: Vec<String> = sorted_for_display(&entries)
            .into_iter()
            .map(|c| c.name)
            .collect();
        // Generic families (SQL, buckets, iceberg) first, then API services A→Z.
        assert_eq!(
            names,
            vec!["postgres", "buckets", "iceberg", "aikido", "stripe"]
        );
    }

    #[test]
    fn progress_message_appends_detail_when_present() {
        assert_eq!(
            progress_message("importing", "running", None),
            "importing… running"
        );
        // Empty/whitespace detail is dropped, not shown as a dangling dash.
        assert_eq!(
            progress_message("importing", "pending", Some("  ")),
            "importing… pending"
        );
        assert_eq!(
            progress_message("importing", "loading", Some("region (5 rows)")),
            "importing… loading — region (5 rows)"
        );
    }
}
