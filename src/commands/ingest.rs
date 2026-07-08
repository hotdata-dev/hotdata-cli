//! `hotdata ingest` — pull data from external sources into a managed database.
//!
//! The surface mirrors `hotdata connections`: `new` (guided, interactive),
//! `list`, `create` (scriptable, `create list` browses the catalog), `import`
//! (SQL front-door against an onboarded source), and `refresh`. Onboarding
//! enqueues a source, fires the drain, and polls to completion — the read side
//! (inspecting/querying results) is the core `query`/`databases`/`results`
//! commands, so it isn't duplicated here.
//!
//! The `new` wizard is catalog-driven: the `/ingest/connectors` catalog returns
//! a `template` per REST service with the `base_url`, auth shape, and resources
//! already filled in, so the user only supplies the `<PLACEHOLDER>` secrets —
//! never a URL the catalog already knows. Enqueueing needs a durable `hd_` API
//! key (`--api-key`/`HOTDATA_API_KEY`); results land in the authenticated
//! workspace — there is no destination flag by design.

use crate::client::ingest::{
    ConnectorEntry, IngestAck, IngestClient, IngestRequest, QueryToIngest,
};
use crate::client::sdk::Api;
use crate::util;
use inquire::{Password, Select, Text};
use std::time::{Duration, Instant};

/// Status-poll cadence (what the hotdlt UI uses). Each poll is a control-store
/// query on the worker, so this is deliberately not sub-second.
const POLL_INTERVAL: Duration = Duration::from_millis(2_500);

/// A drain fired immediately after enqueue can miss the new row (control-store
/// read lag). If a job sits pending this long, kick the drain again.
const DRAIN_REKICK_AFTER: Duration = Duration::from_secs(30);

/// Rows an `import` with no explicit SQL pulls — "a reasonable amount" without
/// committing the user to a full-table download they didn't ask for.
const DEFAULT_IMPORT_LIMIT: u32 = 1_000;

#[derive(clap::Subcommand)]
pub enum IngestCommands {
    /// Interactively onboard a source, guided by the connector catalog
    New,

    /// List the sources you've ingested
    List,

    /// Onboard a source non-interactively (or `create list` to browse the catalog)
    Create {
        #[command(subcommand)]
        command: Option<CreateCommands>,

        /// Connector to onboard (a catalog name: postgres, bitcoin, filesystem, …)
        #[arg(long)]
        service: Option<String>,

        /// Family payload as JSON (inline, @file.json, or @-): SQL credentials,
        /// a REST rest_config, filesystem creds, or an Iceberg catalog_config
        #[arg(long)]
        config: Option<String>,

        /// Table to ingest (repeatable; sql/iceberg — omit for all)
        #[arg(long = "table")]
        tables: Vec<String>,

        /// Schema to ingest (sql)
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

        /// Credential-check + schema discovery only (no full download)
        #[arg(long = "validate-only")]
        validate_only: bool,

        #[command(flatten)]
        wait: WaitArgs,

        /// Reuse an existing managed database (by id) instead of minting one
        #[arg(long = "database-id")]
        database_id: Option<String>,
    },

    /// Import data from an onboarded source via SQL (defaults to a sample)
    Import {
        /// SELECT <cols|*> FROM <source>[.<resource>] [WHERE …] [LIMIT n].
        /// Omit to import a sample from --source.
        sql: Option<String>,

        /// Source to import from: a connector name (used in FROM / for the
        /// default sample) or an onboard ingest-id (pins resolution)
        #[arg(long)]
        source: Option<String>,

        /// Reuse an existing managed database (by id)
        #[arg(long = "database-id")]
        database_id: Option<String>,

        #[command(flatten)]
        wait: WaitArgs,
    },

    /// Re-run an ingest by id (re-drains and polls it to completion)
    Refresh {
        /// Ingest id (from `new`/`create`/`import`, or `ingest list`)
        id: String,

        /// Seconds to wait for completion (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
    },
}

/// Shared wait/no-wait flags for the enqueue commands.
#[derive(clap::Args)]
pub struct WaitArgs {
    /// Enqueue only; print the ingest id and return without waiting
    #[arg(long = "no-wait")]
    no_wait: bool,

    /// Seconds to wait for completion before giving up (default 300)
    #[arg(long = "wait-timeout", default_value = "300")]
    wait_timeout: u64,
}

#[derive(clap::Subcommand)]
pub enum CreateCommands {
    /// List the connector catalog (services, dialects, family templates)
    List {
        /// Filter to connectors whose name contains this text
        name: Option<String>,
        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: IngestCommands) {
    match command {
        IngestCommands::New => wizard(workspace_id, output),
        IngestCommands::List => list(workspace_id, output),
        IngestCommands::Create {
            command: Some(CreateCommands::List { name, output }),
            ..
        } => catalog_list(workspace_id, name.as_deref(), &output),
        IngestCommands::Create {
            command: None,
            service,
            config,
            tables,
            schema,
            bucket_url,
            format,
            glob,
            catalog_type,
            validate_only,
            wait,
            database_id,
        } => create(
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
                validate_only,
                database_id,
            },
            &wait,
        ),
        IngestCommands::Import {
            sql,
            source,
            database_id,
            wait,
        } => import(workspace_id, output, sql, source, database_id, &wait),
        IngestCommands::Refresh { id, wait_timeout } => {
            refresh(workspace_id, output, &id, wait_timeout)
        }
    }
}

// --- new: the guided wizard ----------------------------------------------

fn wizard(workspace_id: &str, output: &str) {
    if !util::is_interactive() {
        eprintln!(
            "error: 'ingest new' is interactive and stdin is not a TTY. \
             Use 'hotdata ingest create list' to browse connectors, then \
             'hotdata ingest create --service <name> …'."
        );
        std::process::exit(1);
    }
    let client = IngestClient::new(workspace_id);
    let entries = fetch_catalog(&client);
    let entry = select_connector(&entries);

    let req = match entry.family.as_str() {
        "sql" => build_sql_interactive(&entry),
        "filesystem" => build_filesystem_interactive(),
        "iceberg" => build_iceberg_interactive(&entry),
        _ => build_rest_interactive(&entry),
    };
    run_source(&client, output, false, 300, req);
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
            let name = ask_text("Connection name (names the source for `ingest import`):");
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

// --- create: scriptable onboarding ---------------------------------------

struct CreateArgs {
    service: Option<String>,
    config: Option<String>,
    tables: Vec<String>,
    schema: Option<String>,
    bucket_url: Option<String>,
    format: Option<String>,
    glob: Option<String>,
    catalog_type: Option<String>,
    validate_only: bool,
    database_id: Option<String>,
}

fn create(workspace_id: &str, output: &str, args: CreateArgs, wait: &WaitArgs) {
    let Some(service) = args.service.as_deref() else {
        eprintln!(
            "error: --service is required (a catalog connector name). \
             Browse them with 'hotdata ingest create list', or run 'hotdata ingest new'."
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
            eprintln!("error: unknown connector '{service}'. Run 'hotdata ingest create list'.");
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
                    "connector '{service}' needs secrets ({}) — pass a filled --config, or use 'hotdata ingest new'",
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
    req.validate_only = args.validate_only;
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
    match output {
        "json" => {
            let v: Vec<_> = entries
                .iter()
                .map(|c| serde_json::json!({"name": c.name, "family": c.family, "description": c.description}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        "yaml" => {
            let v: Vec<_> = entries
                .iter()
                .map(|c| serde_json::json!({"name": c.name, "family": c.family, "description": c.description}))
                .collect();
            print!("{}", serde_yaml::to_string(&v).unwrap());
        }
        _ => {
            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|c| vec![c.name.clone(), c.family.clone(), c.description.clone()])
                .collect();
            crate::output::table::print(&["NAME", "FAMILY", "DESCRIPTION"], &rows);
        }
    }
}

// --- import: SQL front-door ----------------------------------------------

fn import(
    workspace_id: &str,
    output: &str,
    sql: Option<String>,
    source: Option<String>,
    database_id: Option<String>,
    wait: &WaitArgs,
) {
    // A 32-char hex --source is an onboard id (pin resolution); anything else is
    // a connector name (the FROM target for the default sample).
    let (pin, name) = match source {
        Some(s) if looks_like_ingest_id(&s) => (Some(s), None),
        Some(s) => (None, Some(s)),
        None => (None, None),
    };

    let query = match sql {
        Some(q) => q,
        None => match &name {
            Some(n) => format!("SELECT * FROM {n} LIMIT {DEFAULT_IMPORT_LIMIT}"),
            None => {
                fail("provide a SQL query, or --source <connector-name> to import a default sample")
            }
        },
    };

    let client = IngestClient::new(workspace_id);
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
    if wait.no_wait {
        render_ack(&ack, output);
        return;
    }
    poll_ingest(&client, &ack.ingest_id, wait.wait_timeout, output);
}

fn looks_like_ingest_id(s: &str) -> bool {
    s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())
}

// --- refresh -------------------------------------------------------------

fn refresh(workspace_id: &str, output: &str, id: &str, wait_timeout: u64) {
    // No stored source config to re-onboard from (the worker holds it encrypted),
    // so refresh means "re-process": re-drain and poll the ingest to a terminal
    // state. Useful for a job stuck pending, and a no-op reload for a done one.
    let client = IngestClient::new(workspace_id);
    poll_ingest(&client, id, wait_timeout, output);
}

// --- list ----------------------------------------------------------------

fn list(workspace_id: &str, output: &str) {
    let api = Api::new(Some(workspace_id));
    let dbs = crate::commands::databases::list_id_name_pairs(&api).unwrap_or_else(|e| e.exit());
    // Ingest mints result databases named `ingest-<source>-<id8>` (onboards) and
    // `query-<source>-<id8>` (imports); everything else is a plain managed DB.
    let mut rows: Vec<IngestedRow> = dbs
        .into_iter()
        .filter_map(|(id, name)| {
            let name = name?;
            let (kind, source) = if let Some(rest) = name.strip_prefix("ingest-") {
                ("onboard", strip_id_suffix(rest))
            } else if let Some(rest) = name.strip_prefix("query-") {
                ("import", strip_id_suffix(rest))
            } else {
                return None;
            };
            Some(IngestedRow {
                source,
                kind: kind.to_string(),
                database_id: id,
            })
        })
        .collect();
    rows.sort_by(|a, b| a.source.cmp(&b.source).then_with(|| a.kind.cmp(&b.kind)));

    match output {
        "json" => println!("{}", serde_json::to_string_pretty(&rows).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&rows).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            if rows.is_empty() {
                eprintln!(
                    "{}",
                    "No ingested sources yet. Onboard one with 'hotdata ingest new'.".dark_grey()
                );
                return;
            }
            let table: Vec<Vec<String>> = rows
                .iter()
                .map(|r| vec![r.source.clone(), r.kind.clone(), r.database_id.clone()])
                .collect();
            crate::output::table::print(&["SOURCE", "KIND", "DATABASE"], &table);
        }
    }
}

#[derive(serde::Serialize)]
struct IngestedRow {
    source: String,
    kind: String,
    database_id: String,
}

/// Drop the trailing `-<8 hex>` id the worker appends to a result-DB name,
/// leaving the source/connector label.
fn strip_id_suffix(s: &str) -> String {
    match s.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.chars().all(|c| c.is_ascii_hexdigit()) => {
            head.to_string()
        }
        _ => s.to_string(),
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
    // The first enqueue in a workspace deploys the dlt runtime (~15-30s); later
    // enqueues hash-short-circuit. The HTTP client allows 300s.
    let spinner = util::spinner("enqueuing ingest… (the first one in a workspace takes ~30s)");
    let ack = client.create_source(&req).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();
    if no_wait {
        render_ack(&ack, output);
        return;
    }
    poll_ingest(client, &ack.ingest_id, wait_timeout, output);
}

/// Fire the drain, then poll the ingest to a terminal state.
/// Exit codes: 0 done, 1 failed, 2 still running / timed out.
fn poll_ingest(client: &IngestClient, ingest_id: &str, timeout_secs: u64, output: &str) {
    let _ = client.drain();
    let mut last_drain = Instant::now();

    let spinner = util::spinner("ingesting…");
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let st = client.job_status(ingest_id).unwrap_or_else(|e| {
            spinner.finish_and_clear();
            e.exit()
        });
        match st.status.as_str() {
            "done" => {
                spinner.finish_and_clear();
                render_done(&st, output);
                return;
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
            "pending" => {
                // The enqueue-time drain may have raced the request row; double
                // drains are harmless (loads are replace-mode).
                if last_drain.elapsed() > DRAIN_REKICK_AFTER {
                    let _ = client.drain();
                    last_drain = Instant::now();
                }
            }
            "running" | "queued" => {}
            other => {
                spinner.finish_and_clear();
                use crossterm::style::Stylize;
                eprintln!("{}", format!("ingest status: {other}").yellow());
                eprintln!(
                    "{}",
                    format!("Check status with: hotdata ingest refresh {ingest_id}").dark_grey()
                );
                std::process::exit(2);
            }
        }
        if Instant::now() > deadline {
            spinner.finish_and_clear();
            use crossterm::style::Stylize;
            eprintln!("{}", "ingest timed out".red());
            eprintln!(
                "{}",
                format!("Check status with: hotdata ingest refresh {ingest_id}").dark_grey()
            );
            std::process::exit(2);
        }
        std::thread::sleep(POLL_INTERVAL);
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
                format!("Track it with: hotdata ingest refresh {}", ack.ingest_id).dark_grey()
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
    fn strip_id_suffix_drops_the_8hex_tail_only() {
        assert_eq!(strip_id_suffix("bitcoin-6ed3a0ed"), "bitcoin");
        assert_eq!(
            strip_id_suffix("cli_e2e_bitcoin-4cc2e74d"),
            "cli_e2e_bitcoin"
        );
        // no 8-hex tail → unchanged
        assert_eq!(strip_id_suffix("postgres"), "postgres");
        assert_eq!(strip_id_suffix("my-source"), "my-source");
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
}
