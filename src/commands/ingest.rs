//! `hotdata ingest` — pull data from external sources into a managed database.
//!
//! Two nouns, named explicitly in every command: **connections** (onboarded,
//! credentialed sources — schema discovered, no data loaded) and **imports**
//! (managed DBs materialized from a connection). Commands: `new-connection`,
//! `list-connections`, `connectors` (the catalog), `new-import` (--all or
//! SQL), `list-imports`, `trigger-import` (re-run: refresh an ingest's DB
//! from source), `status` (one-shot or `--wait` attach). The pre-rename verbs
//! (`new`/`list`/`import`/`update`) remain as hidden aliases for one release.
//!
//! **Imports don't block.** `new-import` and `trigger-import` enqueue, fire
//! the drain, print the ingest id, and return; progress is tracked with
//! `status <id> [--wait]` or `list-imports`. `--wait` opts into the old
//! blocking poll. `new-connection` is the deliberate exception — it blocks
//! by default because its output IS the feedback (credentials validated,
//! schema discovered); `--no-wait` opts out.
//!
//! `new-connection` **adds a connection and discovers its schema — it loads no
//! data** (the server's `validate_only` mode: check credentials, reflect the
//! schema, cap extraction). It runs the guided wizard when no `--service` is
//! given on a terminal, and is flag-driven otherwise (`--service` given, a
//! non-TTY, or `--no-input`) — one command, two front doors. Pulling rows is
//! the separate, explicit `new-import` step, read back through the core
//! `query`/`databases`/`results` commands.
//!
//! The wizard is catalog-driven: the `/ingest/connectors` catalog returns a
//! `template` per REST service with the `base_url`, auth shape, and resources
//! already filled in, so the user only supplies the `<PLACEHOLDER>` secrets —
//! never a URL the catalog already knows. Enqueueing needs a durable `hd_` API
//! key (`--api-key`/`HOTDATA_API_KEY`); results land in the authenticated
//! workspace — there is no destination flag by design.

use crate::client::ingest::{
    ConnectorEntry, IngestAck, IngestClient, IngestRequest, QueryToIngest,
};
use crate::util;
use inquire::{Password, Select, Text};
use std::time::{Duration, Instant};

/// Status-poll cadence (what the hotdlt UI uses). Each poll is a control-store
/// query on the worker, so this is deliberately not sub-second.
const POLL_INTERVAL: Duration = Duration::from_millis(2_500);

/// A drain fired immediately after enqueue can miss the new row (control-store
/// read lag). If a job sits pending this long, kick the drain again.
const DRAIN_REKICK_AFTER: Duration = Duration::from_secs(30);

#[derive(clap::Subcommand)]
pub enum IngestCommands {
    /// Add a source connection and discover its schema (loads no data)
    ///
    /// Interactive by default; pass `--service` with config flags to add
    /// non-interactively. Pull rows separately with `hotdata ingest
    /// new-import`; browse connectors with `hotdata ingest connectors`.
    #[command(alias = "new")]
    NewConnection {
        /// Connector to add (a catalog name: postgres, bitcoin, filesystem, …).
        /// Given → non-interactive; omit on a terminal → guided wizard.
        #[arg(long)]
        service: Option<String>,

        /// Family payload as JSON (inline, @file.json, or @-): SQL credentials,
        /// a REST rest_config, filesystem creds, or an Iceberg catalog_config
        #[arg(long)]
        config: Option<String>,

        /// Restrict discovery to this table (repeatable; sql/iceberg)
        #[arg(long = "table")]
        tables: Vec<String>,

        /// Schema to discover (sql)
        #[arg(long)]
        schema: Option<String>,

        /// Bucket URL, e.g. s3://bucket/prefix (filesystem)
        #[arg(long = "bucket-url")]
        bucket_url: Option<String>,

        /// File format (filesystem)
        #[arg(long, value_parser = ["csv", "jsonl", "parquet"])]
        format: Option<String>,

        /// File glob, e.g. **/*.parquet (filesystem)
        #[arg(long)]
        glob: Option<String>,

        /// Catalog type, e.g. rest (iceberg)
        #[arg(long = "catalog-type")]
        catalog_type: Option<String>,

        #[command(flatten)]
        wait: WaitArgs,

        /// Reuse an existing managed database (by id) instead of minting one
        #[arg(long = "database-id")]
        database_id: Option<String>,
    },

    /// List the connections you've added (each has its own connection id)
    #[command(alias = "list")]
    ListConnections {
        /// Include superseded onboards (older connections a newer onboard of
        /// the same connector replaced)
        #[arg(long)]
        all: bool,
    },

    /// Browse the connector catalog
    #[command(alias = "supported-connectors")]
    Connectors {
        /// Filter to connectors whose name contains this text
        name: Option<String>,
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// Import data from a connection into a managed database (--all or SQL)
    #[command(alias = "import")]
    NewImport {
        /// SELECT <cols|*> FROM <connection>[.<resource>] [WHERE …] [LIMIT n]
        sql: Option<String>,

        /// Import everything from --source (SELECT * with no LIMIT)
        #[arg(long, conflicts_with = "sql")]
        all: bool,

        /// Connection to import from: a connector name (used in FROM) or a
        /// connection id from `list-connections` (pins resolution)
        #[arg(long)]
        source: Option<String>,

        /// Reuse an existing managed database (by id) instead of minting one
        #[arg(long = "database-id")]
        database_id: Option<String>,

        /// Block until the import completes (default: return immediately;
        /// track with `hotdata ingest status <id> --wait`)
        #[arg(long)]
        wait: bool,

        /// Seconds to wait for completion with --wait (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
    },

    /// List your imports: the SQL behind each and the database it landed in
    ListImports,

    /// Re-run an ingest: refresh its database from the source
    ///
    /// The worker resets the ingest to pending and re-drains it; loads are
    /// replace-mode, so the same managed database is refreshed with the
    /// stored credentials — nothing is re-entered.
    #[command(alias = "update")]
    TriggerImport {
        /// Ingest id: an import from `list-imports`, or a connection from
        /// `list-connections` (re-validates and refreshes its schema)
        id: String,

        /// Block until the re-run completes (default: return immediately;
        /// track with `hotdata ingest status <id> --wait`)
        #[arg(long)]
        wait: bool,

        /// Seconds to wait for completion with --wait (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
    },

    /// Show an ingest's status (an import or a connection onboard)
    ///
    /// One-shot by default with script-friendly exit codes: 0 done,
    /// 1 failed, 2 still in flight. `--wait` attaches and polls to a
    /// terminal state instead.
    Status {
        /// Ingest id (from `new-import`, `list-imports`, or `list-connections`)
        id: String,

        /// Poll until the ingest reaches done/failed instead of returning
        /// the current status
        #[arg(long)]
        wait: bool,

        /// Seconds to wait for completion with --wait (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
    },
}

/// Wait flags for `new-connection` — the one enqueue command that still
/// blocks by default, because its output IS the feedback: did the
/// credentials work, and what schema was discovered. The import commands
/// return immediately and are tracked with `ingest status`.
#[derive(clap::Args)]
pub struct WaitArgs {
    /// Enqueue only; print the ingest id and return without waiting
    #[arg(long = "no-wait")]
    no_wait: bool,

    /// Seconds to wait for completion before giving up (default 300)
    #[arg(long = "wait-timeout", default_value = "300")]
    wait_timeout: u64,
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: IngestCommands) {
    match command {
        IngestCommands::NewConnection {
            service,
            config,
            tables,
            schema,
            bucket_url,
            format,
            glob,
            catalog_type,
            wait,
            database_id,
        } => add_connection(
            workspace_id,
            output,
            CreateArgs {
                service,
                config,
                tables,
                schema,
                bucket_url,
                format,
                glob,
                catalog_type,
                database_id,
            },
            &wait,
        ),
        IngestCommands::ListConnections { all } => list_connections(workspace_id, output, all),
        IngestCommands::Connectors { name, output } => {
            catalog_list(workspace_id, name.as_deref(), &output)
        }
        IngestCommands::NewImport {
            sql,
            all,
            source,
            database_id,
            wait,
            wait_timeout,
        } => new_import(
            workspace_id,
            output,
            sql,
            all,
            source,
            database_id,
            wait,
            wait_timeout,
        ),
        IngestCommands::ListImports => list_imports(workspace_id, output),
        IngestCommands::TriggerImport {
            id,
            wait,
            wait_timeout,
        } => trigger_import(workspace_id, output, &id, wait, wait_timeout),
        IngestCommands::Status {
            id,
            wait,
            wait_timeout,
        } => status(workspace_id, output, &id, wait, wait_timeout),
    }
}

// --- new: add a connection (wizard or flag-driven) -----------------------

/// `ingest new-connection`. The guided wizard runs when no connector was named and we're
/// on a terminal; otherwise (a `--service` was given, or a non-TTY / `--no-input`
/// run) it's flag-driven and needs `--service`.
fn add_connection(workspace_id: &str, output: &str, args: CreateArgs, wait: &WaitArgs) {
    if args.service.is_none() && util::is_interactive() {
        wizard(workspace_id, output, wait);
    } else {
        add_from_flags(workspace_id, output, args, wait);
    }
}

fn wizard(workspace_id: &str, output: &str, wait: &WaitArgs) {
    let client = IngestClient::new(workspace_id);
    let entries = fetch_catalog(&client);
    let entry = select_connector(&entries);

    let mut req = match entry.family.as_str() {
        "sql" => build_sql_interactive(&entry),
        "filesystem" => build_filesystem_interactive(),
        "iceberg" => build_iceberg_interactive(&entry),
        _ => build_rest_interactive(&entry),
    };
    // Adding a connection discovers the schema only — never loads data.
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
                format!("{}  ({})", c.name, c.family)
            } else {
                format!("{}  ({}) — {}", c.name, c.family, c.description)
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
/// dialect aliases are collapsed at the source (the dlthubworker catalog), not
/// here.
fn sorted_for_display(entries: &[ConnectorEntry]) -> Vec<ConnectorEntry> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| {
        family_rank(&a.family)
            .cmp(&family_rank(&b.family))
            .then_with(|| a.name.cmp(&b.name))
    });
    sorted
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

fn build_filesystem_interactive() -> IngestRequest {
    let bucket_url = ask_text("Bucket URL (e.g. s3://bucket/prefix):");
    let format = select_optional("File format:", &["parquet", "csv", "jsonl"]);
    let glob = optional(None, "File glob (e.g. **/*.parquet, blank for all):");
    IngestRequest {
        family: "filesystem".into(),
        credentials: filesystem_credentials(None),
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
        catalog_name: Some(entry.name.clone()),
        catalog_type,
        catalog_config: Some(iceberg_catalog_config(None)),
        tables,
        ..Default::default()
    }
}

/// REST family. A cataloged service carries a `template` with everything but
/// the secrets filled in — walk it, prompt only the `<PLACEHOLDER>` tokens, and
/// send the result. The bare `rest` entry (no template) falls back to building
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
            let name = ask_text("Connection name (names the connection for `ingest new-import`):");
            IngestRequest {
                family: "rest".into(),
                connector_type: Some(name),
                rest_config: Some(rest_config(None)),
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

// --- new (flag-driven): non-interactive add ------------------------------

struct CreateArgs {
    service: Option<String>,
    config: Option<String>,
    tables: Vec<String>,
    schema: Option<String>,
    bucket_url: Option<String>,
    format: Option<String>,
    glob: Option<String>,
    catalog_type: Option<String>,
    database_id: Option<String>,
}

fn add_from_flags(workspace_id: &str, output: &str, args: CreateArgs, wait: &WaitArgs) {
    let Some(service) = args.service.as_deref() else {
        eprintln!(
            "error: --service is required to add a connection non-interactively. \
             Browse connectors with 'hotdata ingest connectors', or run \
             'hotdata ingest new-connection' in a terminal for the guided wizard."
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
            eprintln!("error: unknown connector '{service}'. Run 'hotdata ingest connectors'.");
            std::process::exit(1);
        });

    let config = args.config.as_deref().map(parse_config_arg);
    let mut req = match entry.family.as_str() {
        "sql" => IngestRequest {
            family: "sql".into(),
            connector_type: Some(entry.name.clone()),
            credentials: config.unwrap_or_else(|| {
                fail("sql connectors need --config with credentials (a connection_string or host/user/…)")
            }),
            schema: args.schema,
            table_names: args.tables,
            ..Default::default()
        },
        "filesystem" => {
            let bucket_url = args.bucket_url.unwrap_or_else(|| fail("filesystem needs --bucket-url"));
            IngestRequest {
                family: "filesystem".into(),
                credentials: config.unwrap_or(serde_json::Value::Null),
                bucket_url: Some(bucket_url),
                file_glob: args.glob,
                file_format: args.format,
                ..Default::default()
            }
        }
        "iceberg" => IngestRequest {
            family: "iceberg".into(),
            catalog_name: Some(entry.name.clone()),
            catalog_type: args.catalog_type.or_else(|| Some("rest".into())),
            catalog_config: Some(config.unwrap_or_else(|| fail("iceberg needs --config with the catalog config"))),
            tables: args.tables,
            ..Default::default()
        },
        _ => {
            // REST: an explicit --config wins; otherwise the catalog template,
            // which only works untouched for keyless services.
            let rest_config = config.or_else(|| entry.template.clone()).unwrap_or_else(|| {
                fail("this REST connector needs --config with a rest_config")
            });
            let mut leftover = Vec::new();
            collect_placeholders(&rest_config, &mut leftover);
            if !leftover.is_empty() {
                fail(&format!(
                    "connector '{service}' needs secrets ({}) — pass a filled --config, or use 'hotdata ingest new-connection'",
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
    // Adding a connection discovers the schema only — never loads data.
    req.validate_only = true;
    req.database_id = args.database_id;
    run_source(&client, output, wait.no_wait, wait.wait_timeout, req);
}

fn catalog_list(workspace_id: &str, filter: Option<&str>, output: &str) {
    let client = IngestClient::new(workspace_id);
    let mut entries = fetch_catalog(&client);
    if let Some(f) = filter {
        let f = f.to_lowercase();
        entries.retain(|c| c.name.to_lowercase().contains(&f));
    }
    let entries = sorted_for_display(&entries);
    // "active" = a connector this workspace has already added (it appears in
    // the onboarded-source registry). Best-effort: if the registry read fails
    // the catalog still shows, just without active marks.
    let active = active_source_names(&client);
    let is_active = |name: &str| active.contains(name);

    match output {
        "json" | "yaml" => {
            let v: Vec<_> = entries
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "family": c.family,
                        "active": is_active(&c.name),
                        "description": c.description,
                    })
                })
                .collect();
            if output == "yaml" {
                print!("{}", serde_yaml::to_string(&v).unwrap());
            } else {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
        }
        _ => {
            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|c| {
                    let status = if is_active(&c.name) { "active" } else { "" };
                    vec![
                        c.name.clone(),
                        c.family.clone(),
                        status.to_string(),
                        c.description.clone(),
                    ]
                })
                .collect();
            crate::output::table::print(&["NAME", "FAMILY", "STATUS", "DESCRIPTION"], &rows);
        }
    }
}

/// Connector names this workspace has onboarded, from the sources registry.
/// Used to flag which catalog connectors are active. Best-effort — an
/// unavailable registry yields an empty set.
fn active_source_names(client: &IngestClient) -> std::collections::HashSet<String> {
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

#[allow(clippy::too_many_arguments)] // mirrors the clap surface one-to-one
fn new_import(
    workspace_id: &str,
    output: &str,
    sql: Option<String>,
    all: bool,
    source: Option<String>,
    database_id: Option<String>,
    wait: bool,
    wait_timeout: u64,
) {
    // A 32-char hex --source is a connection id (pins resolution); anything
    // else is a connector name (the FROM target).
    let (pin, name) = match source {
        Some(s) if looks_like_ingest_id(&s) => (Some(s), None),
        Some(s) => (None, Some(s)),
        None => (None, None),
    };

    let client = IngestClient::new(workspace_id);
    // `--all` against a pinned id needs the connector name for FROM — resolve
    // it from the sources registry, only when actually needed.
    let pinned_name = (all && name.is_none())
        .then(|| {
            pin.as_deref()
                .and_then(|p| pinned_connector_name(&client, p))
        })
        .flatten();
    let query = build_import_query(sql, all, name.as_deref(), pinned_name.as_deref())
        .unwrap_or_else(|msg| fail(msg));
    let req = QueryToIngest {
        query,
        source_ingest_id: pin,
        database_id,
    };
    let spinner = util::spinner("submitting import…");
    // The by-name FROM lookup reads control-store snapshots that lag writes, so
    // an import right after onboarding can 404 briefly — retry a few times.
    let mut ack = client.create_query(&req);
    for _ in 0..3 {
        match &ack {
            Err(crate::client::ingest::IngestError::Http { status: 404, .. }) => {
                spinner.set_message("waiting for the source to register…");
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
    if wait {
        let st = poll_ingest(&client, &ack.ingest_id, wait_timeout, "importing", true);
        render_done(&st, output);
        return;
    }
    // Non-blocking default: the worker is on-demand, so fire the drain that
    // actually processes the job — without it the import would sit pending —
    // then hand the terminal back.
    let _ = client.drain();
    render_ack(&ack, output);
}

fn looks_like_ingest_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// The SQL a `new-import` runs: explicit SQL wins; `--all` becomes a full
/// `SELECT *` against the connection's name (`pinned_name` when --source was
/// a connection id). Errors are messages for `fail`.
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
            .ok_or("--all needs --source <connection> (a connector name or connection id)"),
        (None, false) => Err(
            "provide SQL (SELECT … FROM <connection> …), or --all to import \
             everything from --source",
        ),
    }
}

/// Resolve a pinned connection id to its connector name (the FROM target)
/// via the sources registry.
fn pinned_connector_name(client: &IngestClient, ingest_id: &str) -> Option<String> {
    client
        .list_sources(true)
        .ok()?
        .sources
        .into_iter()
        .find(|s| s.ingest_id == ingest_id)
        .and_then(|s| s.connector_type)
}

// --- trigger-import --------------------------------------------------------

fn trigger_import(workspace_id: &str, output: &str, id: &str, wait: bool, wait_timeout: u64) {
    let client = IngestClient::new(workspace_id);
    let spinner = util::spinner("requesting re-run…");
    // The worker resets the ingest to pending and fires the drain (409 if one
    // is already mid-flight — its message says to retry shortly).
    let ack = client.rerun(id).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();
    if wait {
        // The rerun already drained; poll only.
        let st = poll_ingest(&client, &ack.ingest_id, wait_timeout, "re-running", false);
        render_done(&st, output);
        return;
    }
    render_ack(&ack, output);
}

// --- status ------------------------------------------------------------------

/// Exit code for a one-shot status check, mirroring `query status`:
/// 0 done, 1 failed, 2 still in flight (pending/running/stage states).
fn status_exit_code(status: &str) -> i32 {
    match status {
        "done" => 0,
        "failed" => 1,
        _ => 2,
    }
}

fn status(workspace_id: &str, output: &str, id: &str, wait: bool, wait_timeout: u64) {
    let client = IngestClient::new(workspace_id);
    if wait {
        // Attach and poll to a terminal state. No initial drain kick — the
        // enqueue already fired one — but the poll loop still re-kicks a job
        // stuck in `pending` (control-store read lag), so attaching also
        // rescues an import whose first drain missed the row.
        let st = poll_ingest(&client, id, wait_timeout, "waiting", false);
        render_done(&st, output);
        return;
    }
    let st = client.job_status(id).unwrap_or_else(|e| e.exit());
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(&st).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&st).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
            let colored = match st.status.as_str() {
                "done" => st.status.as_str().green(),
                "failed" => st.status.as_str().red(),
                other => other.yellow(),
            };
            println!("{}{}", label("status:"), colored);
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
                    format!("Attach with: hotdata ingest status {id} --wait").dark_grey()
                );
            }
        }
    }
    std::process::exit(status_exit_code(&st.status));
}

// --- list-connections ------------------------------------------------------

fn list_connections(workspace_id: &str, output: &str, all: bool) {
    let client = IngestClient::new(workspace_id);
    let spinner = util::spinner("loading connections…");
    let resp = client.list_sources(all).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();

    match output {
        "json" => println!("{}", serde_json::to_string_pretty(&resp.sources).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&resp.sources).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            if resp.sources.is_empty() {
                eprintln!(
                    "{}",
                    "No connections yet. Add one with 'hotdata ingest new-connection'.".dark_grey()
                );
                return;
            }
            let mut headers = vec!["NAME", "FAMILY", "STATUS", "CREATED", "CONNECTION ID"];
            if all {
                headers.push("ACTIVE");
            }
            let rows: Vec<Vec<String>> = resp
                .sources
                .iter()
                .map(|s| {
                    let mut row = vec![
                        s.connector_type.clone().unwrap_or_else(|| "-".into()),
                        s.family.clone().unwrap_or_default(),
                        colored_status(&s.status),
                        date_only(s.created_at.as_deref()),
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
        }
    }
}

// --- list-imports ----------------------------------------------------------

fn list_imports(workspace_id: &str, output: &str) {
    let client = IngestClient::new(workspace_id);
    let spinner = util::spinner("loading imports…");
    let resp = client.list_queries().unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();

    match output {
        "json" => println!("{}", serde_json::to_string_pretty(&resp.queries).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&resp.queries).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            if resp.queries.is_empty() {
                eprintln!(
                    "{}",
                    "No imports yet. Create one with 'hotdata ingest new-import \
                     --source <connection> --all' (or pass SQL)."
                        .dark_grey()
                );
                return;
            }
            let rows: Vec<Vec<String>> = resp
                .queries
                .iter()
                .map(|q| {
                    vec![
                        q.connector_type.clone().unwrap_or_else(|| "-".into()),
                        q.query.clone().unwrap_or_default(),
                        colored_status(&q.status),
                        date_only(q.created_at.as_deref()),
                        q.ingest_id.clone(),
                        q.database_id.clone().unwrap_or_else(|| "-".into()),
                    ]
                })
                .collect();
            crate::output::table::print(
                &[
                    "SOURCE",
                    "SQL",
                    "STATUS",
                    "CREATED",
                    "IMPORT ID",
                    "DATABASE",
                ],
                &rows,
            );
        }
    }
}

/// Date part of an ISO timestamp for table display
/// (`2026-07-08T10:12:00+00:00` → `2026-07-08`).
fn date_only(ts: Option<&str>) -> String {
    ts.and_then(|t| t.split('T').next())
        .unwrap_or("-")
        .to_string()
}

/// Status cell for the listing tables, in the repo's status colors:
/// done → green, failed → red, anything in flight (pending/running/worker
/// stage states) → yellow. Table cells are ANSI-safe (`tabled` ansi feature).
fn colored_status(status: &str) -> String {
    use crossterm::style::Stylize;
    match status {
        "done" => status.green().to_string(),
        "failed" => status.red().to_string(),
        _ => status.yellow().to_string(),
    }
}

// --- shared run + poll ----------------------------------------------------

fn run_source(
    client: &IngestClient,
    output: &str,
    no_wait: bool,
    wait_timeout: u64,
    req: IngestRequest,
) {
    // The first add in a workspace deploys the dlt runtime (~15-30s); later
    // ones hash-short-circuit. The HTTP client allows 300s.
    let spinner = util::spinner("adding connection… (the first one in a workspace takes ~30s)");
    let ack = client.create_source(&req).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();
    if no_wait {
        render_ack(&ack, output);
        return;
    }
    let st = poll_ingest(
        client,
        &ack.ingest_id,
        wait_timeout,
        "discovering schema",
        true,
    );
    render_connection_added(client, &st, output);
}

/// Poll the ingest to a terminal state, returning the final (done) status for
/// the caller to render. `kick_drain` fires the drain up front (enqueue paths;
/// `trigger-import` skips it — the rerun already drained). Every non-terminal
/// status is shown live in the spinner (stage + detail); exits the process on
/// failure (1) or timeout (2). `verb` labels the spinner.
fn poll_ingest(
    client: &IngestClient,
    ingest_id: &str,
    timeout_secs: u64,
    verb: &str,
    kick_drain: bool,
) -> crate::client::ingest::JobStatus {
    if kick_drain {
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
                if consecutive_errors >= 3 || Instant::now() > deadline {
                    spinner.finish_and_clear();
                    e.exit();
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
            "failed" => {
                spinner.finish_and_clear();
                use crossterm::style::Stylize;
                let detail = st.detail.as_deref().unwrap_or("unknown error");
                eprintln!("{}", format!("ingest failed: {detail}").red());
                if detail.contains("Forbidden") {
                    eprintln!(
                        "{}",
                        "Forbidden at load time usually means a database-scoped API token — \
                         ingest needs a regular workspace API token."
                            .dark_grey()
                    );
                }
                std::process::exit(1);
            }
            // Any other status is in-progress. Surface it live rather than
            // treating it as terminal — the worker reports stage-level states
            // (e.g. extracting / normalizing / loading) and free-text detail as
            // the pipeline advances, and the CLI shouldn't need to know the set.
            stage => {
                spinner.set_message(progress_message(verb, stage, st.detail.as_deref()));
                // Re-kick the drain if the job never left `pending` — the
                // enqueue-time drain can race the request row (harmless double
                // drain; loads are replace-mode).
                if stage == "pending" {
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
            use crossterm::style::Stylize;
            eprintln!("{}", "ingest timed out".red());
            eprintln!(
                "{}",
                format!("Keep tracking it with: hotdata ingest status {ingest_id} --wait")
                    .dark_grey()
            );
            std::process::exit(2);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Spinner text for an in-progress poll: the verb plus the worker's current
/// stage, and its free-text detail when present (e.g. row counts / table name).
fn progress_message(verb: &str, stage: &str, detail: Option<&str>) -> String {
    match detail.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{verb}… {stage} — {d}"),
        None => format!("{verb}… {stage}"),
    }
}

// --- rendering -----------------------------------------------------------

fn render_ack(ack: &IngestAck, output: &str) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(ack).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(ack).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
            println!("{}{}", label("ingest id:"), ack.ingest_id);
            println!("{}{}", label("database:"), ack.database_id);
            println!("{}{}", label("status:"), ack.status.as_str().yellow());
            println!(
                "{}",
                format!(
                    "Track it with: hotdata ingest status {} --wait  (or: hotdata ingest list-imports)",
                    ack.ingest_id
                )
                .dark_grey()
            );
        }
    }
}

fn render_done(st: &crate::client::ingest::JobStatus, output: &str) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(st).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(st).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            let db = st.database_id.as_deref().unwrap_or("-");
            println!("{} → {}", "done".green(), db);
            println!(
                "{}",
                format!("Query it: hotdata query --database {db} \"SELECT * FROM …\"").dark_grey()
            );
        }
    }
}

/// Render the result of adding a connection: the discovered schema (tables +
/// columns), fetched from the schema-preview the metadata-only run landed. No
/// data was loaded — the closing hint points at `import` for that.
fn render_connection_added(
    client: &IngestClient,
    st: &crate::client::ingest::JobStatus,
    output: &str,
) {
    let db = st.database_id.as_deref().unwrap_or("");
    let schema = (!db.is_empty()).then(|| client.schema(db).ok()).flatten();
    let tables = schema
        .as_ref()
        .and_then(|s| s.get("tables"))
        .and_then(|t| t.as_object());

    match output {
        "json" | "yaml" => {
            let mut v = serde_json::to_value(st).unwrap_or_default();
            if let serde_json::Value::Object(ref mut m) = v {
                m.insert(
                    "tables".into(),
                    schema
                        .as_ref()
                        .and_then(|s| s.get("tables").cloned())
                        .unwrap_or(serde_json::Value::Null),
                );
            }
            if output == "yaml" {
                print!("{}", serde_yaml::to_string(&v).unwrap());
            } else {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
        }
        _ => {
            use crossterm::style::Stylize;
            let source = st.connector_type.as_deref().unwrap_or("source");
            println!("{} {}", "connection added".green(), source.dark_grey());
            if !db.is_empty() {
                println!("{}{}", format!("{:<12}", "database:").dark_grey(), db);
            }
            match tables {
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
            println!(
                "{}",
                format!(
                    "Import data with: hotdata ingest new-import --source {source} --all (or SQL)"
                )
                .dark_grey()
            );
        }
    }
}

// --- catalog fetch --------------------------------------------------------

fn fetch_catalog(client: &IngestClient) -> Vec<ConnectorEntry> {
    let spinner = util::spinner("loading connectors…");
    let resp = client.connectors().unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();
    resp.connectors
}

// --- input helpers --------------------------------------------------------
//
// All prompting is TTY-gated by callers (the wizard errors early on a non-TTY).
// `inquire` returns Err on Ctrl-C/ESC — exit cleanly, matching
// `connections/interactive`.

fn ask_text(label: &str) -> String {
    Text::new(label)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0))
}

fn ask_secret(label: &str) -> String {
    Password::new(label)
        .without_confirmation()
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0))
}

/// Optional value: the flag, else a prompt (TTY) whose blank answer = none.
fn optional(flag: Option<String>, label: &str) -> Option<String> {
    if flag.is_some() {
        return flag;
    }
    if util::is_interactive() {
        let v = ask_text(label);
        return (!v.trim().is_empty()).then_some(v);
    }
    None
}

/// Optional value with a default; non-interactive falls back to the default.
fn optional_default(label: &str, default: &str) -> Option<String> {
    if !util::is_interactive() {
        return Some(default.to_string());
    }
    let v = Text::new(label)
        .with_default(default)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    (!v.trim().is_empty()).then_some(v)
}

/// Select one of `options` (TTY only; None otherwise or on ESC).
fn select_optional(label: &str, options: &[&str]) -> Option<String> {
    if !util::is_interactive() {
        return None;
    }
    Select::new(label, options.to_vec())
        .prompt()
        .ok()
        .map(|s| s.to_string())
}

/// Prompt for a comma-separated list (TTY only; empty otherwise).
fn prompt_list(label: &str) -> Vec<String> {
    if !util::is_interactive() {
        return Vec::new();
    }
    ask_text(label)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Bare `rest` family: build a minimal dlt rest_api config interactively
/// (base_url + optional bearer token + resource paths).
fn rest_config(config: Option<&str>) -> serde_json::Value {
    if let Some(c) = config {
        return parse_config_arg(c);
    }
    let base_url = ask_text("REST base URL:");
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

/// Filesystem object-store credentials: prompt for S3-style creds (all optional
/// — blank access key = public bucket, no creds sent).
fn filesystem_credentials(config: Option<&str>) -> serde_json::Value {
    if let Some(c) = config {
        return parse_config_arg(c);
    }
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
/// token + namespace.
fn iceberg_catalog_config(config: Option<&str>) -> serde_json::Value {
    if let Some(c) = config {
        return parse_config_arg(c);
    }
    let mut m = serde_json::Map::new();
    m.insert("catalog_uri".into(), ask_text("Catalog URI:").into());
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
    fn import_query_all_uses_the_connection_name() {
        // Named source.
        assert_eq!(
            build_import_query(None, true, Some("bitcoin"), None).unwrap(),
            "SELECT * FROM bitcoin"
        );
        // Pinned connection id — name resolved from the registry by the caller.
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

    #[test]
    fn status_exit_codes_mirror_query_status() {
        assert_eq!(status_exit_code("done"), 0);
        assert_eq!(status_exit_code("failed"), 1);
        // Everything else is in flight — including worker stage states the
        // CLI doesn't enumerate (extracting/normalizing/loading).
        assert_eq!(status_exit_code("pending"), 2);
        assert_eq!(status_exit_code("running"), 2);
        assert_eq!(status_exit_code("loading"), 2);
    }

    #[test]
    fn date_only_trims_iso_timestamps() {
        assert_eq!(date_only(Some("2026-07-08T10:12:00+00:00")), "2026-07-08");
        assert_eq!(date_only(Some("2026-07-08")), "2026-07-08");
        assert_eq!(date_only(None), "-");
    }

    #[test]
    fn family_rank_orders_generic_before_rest() {
        assert!(family_rank("sql") < family_rank("rest"));
        assert!(family_rank("filesystem") < family_rank("rest"));
        assert!(family_rank("iceberg") < family_rank("rest"));
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
        }
    }

    #[test]
    fn sorted_for_display_groups_generic_families_before_rest() {
        let entries = vec![
            entry("stripe", "rest"),
            entry("postgres", "sql"),
            entry("filesystem", "filesystem"),
            entry("aikido", "rest"),
            entry("iceberg", "iceberg"),
        ];
        let names: Vec<String> = sorted_for_display(&entries)
            .into_iter()
            .map(|c| c.name)
            .collect();
        // Generic families (sql, filesystem, iceberg) first, then rest A→Z.
        assert_eq!(
            names,
            vec!["postgres", "filesystem", "iceberg", "aikido", "stripe"]
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
