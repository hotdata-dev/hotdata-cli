//! `hotdata ingest` — ingest data from external connectors into a managed
//! database via the hotdata ingest service.
//!
//! Transport is the raw-HTTP [`crate::client::ingest::IngestClient`] (the routes
//! aren't in the SDK yet). Family commands (`sql`/`rest`/`filesystem`/`iceberg`)
//! enqueue a source, fire the drain, and poll to completion — the same
//! spinner + 5-minute-deadline shape as `query::execute`. Enqueueing requires
//! a durable `hd_` API key (`--api-key`/`HOTDATA_API_KEY`); results land in the
//! authenticated workspace — there is no destination flag by design.

use crate::client::ingest::{
    ConnectorsResponse, IngestAck, IngestClient, IngestError, IngestRequest, JobStatus,
    QueryToIngest,
};
use crate::util;
use inquire::{Password, Select, Text};
use std::io::Read;
use std::time::{Duration, Instant};

/// Shared flags for the four family subcommands.
#[derive(clap::Args)]
pub struct CommonSourceArgs {
    /// Reuse an existing managed database (by id) instead of minting one
    #[arg(long = "database-id")]
    database_id: Option<String>,

    /// Credential-check + schema discovery only (no full download)
    #[arg(long)]
    validate_only: bool,

    /// Enqueue only; print the ingest id and return without waiting
    #[arg(long = "no-wait")]
    no_wait: bool,

    /// Seconds to wait for completion before giving up (default 300)
    #[arg(long = "wait-timeout", default_value = "300")]
    wait_timeout: u64,
}

#[derive(clap::Subcommand)]
pub enum IngestCommands {
    /// Ingest tables from a SQL database (Postgres, MySQL, Snowflake, …)
    Sql {
        /// Full connection URL, e.g. postgresql://user:pass@host:5432/db
        #[arg(long)]
        url: Option<String>,
        /// Dialect (postgres, mysql, snowflake, …); inferred from --url when omitted
        #[arg(long = "type")]
        r#type: Option<String>,
        /// Source host (discrete form, when not using --url)
        #[arg(long)]
        host: Option<String>,
        /// Source port
        #[arg(long)]
        port: Option<u16>,
        /// Source user
        #[arg(long)]
        user: Option<String>,
        /// Source database name
        #[arg(long = "dbname")]
        dbname: Option<String>,
        /// Source credentials as JSON (inc. password): inline, @file.json, or @-.
        /// Overlaid by any discrete --host/--user/--dbname flags.
        #[arg(long)]
        config: Option<String>,
        /// Schema to ingest
        #[arg(long)]
        schema: Option<String>,
        /// Table to ingest (repeatable; omit for all tables in the schema)
        #[arg(long = "table")]
        tables: Vec<String>,
        #[command(flatten)]
        common: CommonSourceArgs,
    },

    /// Ingest from a REST API described by a raw dlt rest_api config
    Rest {
        /// Connection name (names the managed DB and the `ingest query` lookup key)
        #[arg(long)]
        name: String,
        /// dlt rest_api config: inline JSON, @file.json, or @- for stdin.
        /// Omit on a TTY to build a basic config interactively.
        #[arg(long)]
        config: Option<String>,
        #[command(flatten)]
        common: CommonSourceArgs,
    },

    /// Ingest files from an object store (S3/GCS/Azure): parquet, csv, jsonl
    Filesystem {
        /// Optional managed-DB label
        #[arg(long)]
        name: Option<String>,
        /// Bucket URL, e.g. s3://bucket/prefix (prompted on a TTY if omitted)
        #[arg(long = "bucket-url")]
        bucket_url: Option<String>,
        /// Glob for files under the bucket, e.g. **/*.parquet
        #[arg(long)]
        glob: Option<String>,
        /// File format
        #[arg(long, value_parser = ["csv", "jsonl", "parquet"])]
        format: Option<String>,
        /// Object-store credentials: inline JSON, @file.json, or @-
        /// (prompted on a TTY if omitted; blank = public bucket)
        #[arg(long)]
        config: Option<String>,
        #[command(flatten)]
        common: CommonSourceArgs,
    },

    /// Ingest tables from an Apache Iceberg REST catalog
    Iceberg {
        /// Optional managed-DB label
        #[arg(long)]
        name: Option<String>,
        /// Catalog type, e.g. rest (prompted on a TTY if omitted)
        #[arg(long = "catalog-type")]
        catalog_type: Option<String>,
        /// Catalog connection config: inline JSON, @file.json, or @-
        /// (prompted on a TTY if omitted)
        #[arg(long)]
        config: Option<String>,
        /// Table to ingest as namespace.table (repeatable; prompted on a TTY if omitted)
        #[arg(long = "table")]
        tables: Vec<String>,
        #[command(flatten)]
        common: CommonSourceArgs,
    },

    /// Run a restricted SQL statement against an onboarded connector
    Query {
        /// SELECT <cols|*> FROM <connector>[.<resource>] [WHERE …] [LIMIT n]
        sql: String,
        /// Pin resolution to an exact prior source (recommended right after onboarding)
        #[arg(long = "source-ingest-id")]
        source_ingest_id: Option<String>,
        /// Reuse an existing managed database (by id)
        #[arg(long = "database-id")]
        database_id: Option<String>,
        /// Enqueue only; don't wait
        #[arg(long = "no-wait")]
        no_wait: bool,
        /// Seconds to wait for completion (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
    },

    /// Turn a natural-language request into an ingest SQL statement
    Translate {
        /// Free-text request
        text: String,
        /// After translating, run the produced SQL as an ingest query
        #[arg(long)]
        run: bool,
    },

    /// List the connector catalog (dialects, family templates, REST services)
    Connectors,

    /// Show the latest status of an ingest
    Status {
        /// Ingest id
        id: String,
    },

    /// Fire the drain job that processes pending ingests
    Drain,

    /// Show tables + columns of an ingest result database
    Schema {
        /// Managed database id
        database_id: String,
    },

    /// Preview sample rows from every table in a result database
    Preview {
        /// Managed database id
        database_id: String,
        /// Max rows per table (default 25)
        #[arg(long, default_value = "25")]
        limit: u32,
    },

    /// Download a loaded table as a parquet file
    Download {
        /// Managed database id
        database_id: String,
        /// Table name
        table: String,
        /// Output path (default <table>.parquet)
        #[arg(long = "output-file", short = 'f')]
        output_file: Option<String>,
        /// Max rows (default 100000)
        #[arg(long, default_value = "100000")]
        limit: u32,
    },
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: IngestCommands) {
    match command {
        IngestCommands::Sql {
            url,
            r#type,
            host,
            port,
            user,
            dbname,
            config,
            schema,
            tables,
            common,
        } => {
            let connector_type = r#type.or_else(|| url.as_deref().and_then(infer_dialect));
            let credentials = sql_credentials(
                &url,
                config.as_deref(),
                &host,
                port,
                &user,
                &dbname,
                connector_type.as_deref(),
            );
            let req = IngestRequest {
                family: "sql".into(),
                connector_type,
                credentials,
                schema,
                table_names: tables,
                validate_only: common.validate_only,
                database_id: common.database_id.clone(),
                ..Default::default()
            };
            run_source(workspace_id, output, &common, req);
        }
        IngestCommands::Rest {
            name,
            config,
            common,
        } => {
            let req = IngestRequest {
                family: "rest".into(),
                connector_type: Some(name),
                rest_config: Some(rest_config(config.as_deref())),
                validate_only: common.validate_only,
                database_id: common.database_id.clone(),
                ..Default::default()
            };
            run_source(workspace_id, output, &common, req);
        }
        IngestCommands::Filesystem {
            name,
            bucket_url,
            glob,
            format,
            config,
            common,
        } => {
            let bucket_url = require(
                bucket_url,
                "Bucket URL (e.g. s3://bucket/prefix):",
                "--bucket-url",
            );
            let format =
                format.or_else(|| select_optional("File format:", &["parquet", "csv", "jsonl"]));
            let glob = optional(glob, "File glob (e.g. **/*.parquet, blank for all):");
            let req = IngestRequest {
                family: "filesystem".into(),
                connector_type: name,
                credentials: filesystem_credentials(config.as_deref()),
                bucket_url: Some(bucket_url),
                file_glob: glob,
                file_format: format,
                validate_only: common.validate_only,
                database_id: common.database_id.clone(),
                ..Default::default()
            };
            run_source(workspace_id, output, &common, req);
        }
        IngestCommands::Iceberg {
            name,
            catalog_type,
            config,
            tables,
            common,
        } => {
            let catalog_type = catalog_type.or_else(|| optional_default("Catalog type:", "rest"));
            let tables = if tables.is_empty() {
                prompt_list("Tables (namespace.table, comma-separated):")
            } else {
                tables
            };
            let req = IngestRequest {
                family: "iceberg".into(),
                catalog_name: name,
                catalog_type,
                catalog_config: Some(iceberg_catalog_config(config.as_deref())),
                tables,
                validate_only: common.validate_only,
                database_id: common.database_id.clone(),
                ..Default::default()
            };
            run_source(workspace_id, output, &common, req);
        }
        IngestCommands::Query {
            sql,
            source_ingest_id,
            database_id,
            no_wait,
            wait_timeout,
        } => run_query(
            workspace_id,
            output,
            sql,
            source_ingest_id,
            database_id,
            no_wait,
            wait_timeout,
        ),
        IngestCommands::Translate { text, run } => translate(workspace_id, output, &text, run),
        IngestCommands::Connectors => connectors(workspace_id, output),
        IngestCommands::Status { id } => status(workspace_id, output, &id),
        IngestCommands::Drain => drain(workspace_id, output),
        IngestCommands::Schema { database_id } => {
            let client = IngestClient::new(workspace_id);
            let v = client.schema(&database_id).unwrap_or_else(|e| e.exit());
            print_value(&v, output);
        }
        IngestCommands::Preview { database_id, limit } => {
            let client = IngestClient::new(workspace_id);
            let v = client
                .preview(&database_id, limit)
                .unwrap_or_else(|e| e.exit());
            print_value(&v, output);
        }
        IngestCommands::Download {
            database_id,
            table,
            output_file,
            limit,
        } => download(workspace_id, &database_id, &table, output_file, limit),
    }
}

// --- source flow ---------------------------------------------------------

fn run_source(workspace_id: &str, output: &str, common: &CommonSourceArgs, req: IngestRequest) {
    let client = IngestClient::new(workspace_id);
    // The first enqueue in a workspace deploys the dlt runtime (~15-30s);
    // later enqueues hash-short-circuit. The HTTP client allows 300s.
    let spinner = util::spinner("enqueuing ingest… (the first one in a workspace takes ~30s)");
    let ack = client.create_source(&req).unwrap_or_else(|e| {
        spinner.finish_and_clear();
        e.exit()
    });
    spinner.finish_and_clear();

    if common.no_wait {
        render_ack(&ack, output);
        return;
    }
    poll_to_completion(&client, &ack, common.wait_timeout, output);
}

fn run_query(
    workspace_id: &str,
    output: &str,
    sql: String,
    source_ingest_id: Option<String>,
    database_id: Option<String>,
    no_wait: bool,
    wait_timeout: u64,
) {
    let client = IngestClient::new(workspace_id);
    let req = QueryToIngest {
        query: sql,
        source_ingest_id,
        database_id,
    };
    let spinner = util::spinner("submitting query…");
    // The by-name connector lookup reads control-store snapshots that lag
    // writes, so a query right after onboarding can 404 briefly — retry a few
    // times before treating it as "never onboarded".
    let mut ack = client.create_query(&req);
    for _ in 0..3 {
        match &ack {
            Err(IngestError::Http { status: 404, .. }) => {
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
    if no_wait {
        render_ack(&ack, output);
        return;
    }
    poll_to_completion(&client, &ack, wait_timeout, output);
}

/// Status-poll cadence (what the hotdlt UI uses). Each poll is a control-store
/// query on the worker, so this is deliberately not sub-second.
const POLL_INTERVAL: Duration = Duration::from_millis(2_500);

/// A drain fired immediately after enqueue can miss the new row (control-store
/// read lag). If a job sits pending this long, kick the drain again.
const DRAIN_REKICK_AFTER: Duration = Duration::from_secs(30);

/// Fire the drain, then poll the ingest to a terminal state. Mirrors
/// `query::execute`: 5-minute (configurable) deadline, spinner.
/// Exit codes: 0 done, 1 failed, 2 still running / timed out.
fn poll_to_completion(client: &IngestClient, ack: &IngestAck, timeout_secs: u64, output: &str) {
    // Best-effort: the worker is on-demand only, so nothing runs until drained.
    let _ = client.drain();
    let mut last_drain = Instant::now();

    let spinner = util::spinner("ingesting…");
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        let st = client.job_status(&ack.ingest_id).unwrap_or_else(|e| {
            spinner.finish_and_clear();
            e.exit()
        });
        match st.status.as_str() {
            "done" => {
                spinner.finish_and_clear();
                render_done(ack, &st, output);
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
                // The drain that ran at enqueue may have raced the request row;
                // double-drains are harmless (loads are replace-mode).
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
                    format!("Check status with: hotdata ingest status {}", ack.ingest_id)
                        .dark_grey()
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
                format!("Check status with: hotdata ingest status {}", ack.ingest_id).dark_grey()
            );
            std::process::exit(2);
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

// --- other commands ------------------------------------------------------

fn translate(workspace_id: &str, output: &str, text: &str, run: bool) {
    let client = IngestClient::new(workspace_id);
    let resp = client
        .translate(text, serde_json::json!([]))
        .unwrap_or_else(|e| e.exit());
    if let Some(reason) = resp.get("blocked").and_then(|v| v.as_str()) {
        use crossterm::style::Stylize;
        eprintln!("{}", format!("blocked: {reason}").yellow());
        std::process::exit(1);
    }
    let sql = resp
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if !run {
        print_value(&resp, output);
        return;
    }
    if sql.is_empty() {
        eprintln!("no SQL produced to run");
        std::process::exit(1);
    }
    run_query(workspace_id, output, sql, None, None, false, 300);
}

fn connectors(workspace_id: &str, output: &str) {
    let client = IngestClient::new(workspace_id);
    let ConnectorsResponse { connectors } = client.connectors().unwrap_or_else(|e| e.exit());
    match output {
        "json" => {
            let v: Vec<_> = connectors
                .iter()
                .map(|c| serde_json::json!({"name": c.name, "family": c.family, "description": c.description}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&v).unwrap());
        }
        _ => {
            let rows: Vec<Vec<String>> = connectors
                .iter()
                .map(|c| vec![c.name.clone(), c.family.clone(), c.description.clone()])
                .collect();
            crate::output::table::print(&["NAME", "FAMILY", "DESCRIPTION"], &rows);
        }
    }
}

fn status(workspace_id: &str, output: &str, id: &str) {
    let client = IngestClient::new(workspace_id);
    let st = client.job_status(id).unwrap_or_else(|e| e.exit());
    render_status(&st, output);
}

fn drain(workspace_id: &str, output: &str) {
    let client = IngestClient::new(workspace_id);
    let v = client.drain().unwrap_or_else(|e| e.exit());
    match output {
        "json" | "yaml" => print_value(&v, output),
        _ => {
            use crossterm::style::Stylize;
            eprintln!("{}", "drain triggered".green());
        }
    }
}

fn download(
    workspace_id: &str,
    database_id: &str,
    table: &str,
    output_file: Option<String>,
    limit: u32,
) {
    let client = IngestClient::new(workspace_id);
    let path = output_file.unwrap_or_else(|| format!("{table}.parquet"));
    let spinner = util::spinner("downloading…");
    let (bytes, truncated) = client
        .download(database_id, table, limit)
        .unwrap_or_else(|e| {
            spinner.finish_and_clear();
            e.exit()
        });
    spinner.finish_and_clear();
    if let Err(e) = std::fs::write(&path, &bytes) {
        eprintln!("error writing {path}: {e}");
        std::process::exit(1);
    }
    use crossterm::style::Stylize;
    println!("{} ({} bytes)", path.clone().green(), bytes.len());
    if truncated {
        eprintln!(
            "{}",
            "warning: result was truncated (row cap hit) — increase --limit for the full table"
                .yellow()
        );
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
                format!("Track it with: hotdata ingest status {}", ack.ingest_id).dark_grey()
            );
        }
    }
}

fn render_status(st: &JobStatus, output: &str) {
    match output {
        "json" => println!("{}", serde_json::to_string_pretty(st).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(st).unwrap()),
        _ => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
            let colored = match st.status.as_str() {
                "done" => st.status.as_str().green().to_string(),
                "failed" => st.status.as_str().red().to_string(),
                _ => st.status.as_str().yellow().to_string(),
            };
            println!("{}{}", label("ingest id:"), st.ingest_id);
            println!("{}{}", label("status:"), colored);
            if let Some(c) = &st.connector_type {
                println!("{}{}", label("connector:"), c);
            }
            if let Some(db) = &st.database_id {
                println!("{}{}", label("database:"), db);
            }
            if let Some(u) = &st.updated_at {
                println!("{}{}", label("updated:"), util::format_date(u));
            }
            if let Some(d) = &st.detail {
                println!("{}{}", label("detail:"), d);
            }
        }
    }
}

fn render_done(ack: &IngestAck, st: &JobStatus, output: &str) {
    match output {
        "json" | "yaml" => render_status(st, output),
        _ => {
            use crossterm::style::Stylize;
            println!("{} → {}", "done".green(), ack.database_id);
            println!(
                "{}",
                format!(
                    "Query it: hotdata query --database {} \"SELECT * FROM …\"",
                    ack.database_id
                )
                .dark_grey()
            );
        }
    }
}

fn print_value(v: &serde_json::Value, output: &str) {
    match output {
        "yaml" => print!("{}", serde_yaml::to_string(v).unwrap()),
        // schema/preview return arbitrary JSON — pretty-print for both json and table.
        _ => println!("{}", serde_json::to_string_pretty(v).unwrap()),
    }
}

// --- input helpers -------------------------------------------------------

/// Gather the `sql` family source credentials — ingest's own credential path, no
/// dependency on the hotdata connections store (which ingest will supersede).
///
/// Precedence: `--url` wins (a full connection string); otherwise start from
/// `--config` (a credentials JSON, out of argv via `@file`/`@-`), overlay the
/// discrete non-secret flags, then — **only on an interactive TTY** — prompt for
/// any still-missing fields (password hidden). In `--no-input`/non-TTY runs no
/// prompt happens: the flags must supply everything. These are the *source*
/// database's credentials; the CLI's hotdata auth is the separate JWT bearer.
fn sql_credentials(
    url: &Option<String>,
    config: Option<&str>,
    host: &Option<String>,
    port: Option<u16>,
    user: &Option<String>,
    dbname: &Option<String>,
    dialect: Option<&str>,
) -> serde_json::Value {
    if let Some(url) = url {
        return serde_json::json!({ "connection_string": url });
    }
    let mut m = match config.map(parse_config_arg) {
        Some(serde_json::Value::Object(m)) => m,
        Some(_) => fail_config("must be a JSON object of credentials"),
        None => serde_json::Map::new(),
    };
    if let Some(h) = host {
        m.insert("host".into(), h.clone().into());
    }
    if let Some(p) = port {
        m.insert("port".into(), p.into());
    }
    if let Some(u) = user {
        m.insert("username".into(), u.clone().into());
    }
    if let Some(d) = dbname {
        m.insert("database".into(), d.clone().into());
    }

    if util::is_interactive() {
        prompt_missing_sql_fields(&mut m, dialect);
    }

    if m.is_empty() {
        eprintln!(
            "error: no source credentials — provide --url/--config, discrete \
             --host/--user/--dbname flags, or run interactively (a TTY)"
        );
        std::process::exit(1);
    }
    serde_json::Value::Object(m)
}

/// Fill still-missing source-DB fields by prompting the user. `inquire` prompts
/// abort (Err) on Ctrl-C/ESC — exit cleanly like `connections/interactive`.
fn prompt_missing_sql_fields(
    m: &mut serde_json::Map<String, serde_json::Value>,
    dialect: Option<&str>,
) {
    if !m.contains_key("host") {
        let host = Text::new("Source host:")
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        m.insert("host".into(), host.into());
    }
    if !m.contains_key("port") {
        let default_port = default_port_for(dialect);
        let mut p = Text::new("Source port:");
        if let Some(dp) = default_port {
            p = p.with_default(dp);
        }
        let port = p.prompt().unwrap_or_else(|_| std::process::exit(0));
        if let Ok(n) = port.trim().parse::<u16>() {
            m.insert("port".into(), n.into());
        }
    }
    if !m.contains_key("username") {
        let user = Text::new("Source user:")
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        m.insert("username".into(), user.into());
    }
    if !m.contains_key("password") {
        let pw = Password::new("Source password:")
            .without_confirmation()
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        if !pw.is_empty() {
            m.insert("password".into(), pw.into());
        }
    }
    if !m.contains_key("database") {
        let db = Text::new("Source database:")
            .prompt()
            .unwrap_or_else(|_| std::process::exit(0));
        m.insert("database".into(), db.into());
    }
}

fn default_port_for(dialect: Option<&str>) -> Option<&'static str> {
    match dialect {
        Some("postgres") => Some("5432"),
        Some("mysql") => Some("3306"),
        Some("mssql") => Some("1433"),
        _ => None,
    }
}

// --- shared interactive gather helpers (rest / filesystem / iceberg) ------
//
// All prompting is TTY-gated by `util::is_interactive()` (false under
// `--no-input` / non-TTY). `inquire` returns Err on Ctrl-C/ESC — exit cleanly,
// matching `connections/interactive`.

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

/// Required value: the flag, else a prompt (TTY), else a hard error.
fn require(flag: Option<String>, label: &str, what: &str) -> String {
    if let Some(v) = flag {
        return v;
    }
    if util::is_interactive() {
        return ask_text(label);
    }
    eprintln!("error: {what} is required (pass the flag or run interactively on a TTY)");
    std::process::exit(1);
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

/// REST `rest_config`: use `--config` if given, else build a basic dlt rest_api
/// config interactively (base_url + optional bearer token + resource paths).
/// For richer configs pass `--config` (or start from an `ingest connectors`
/// template).
fn rest_config(config: Option<&str>) -> serde_json::Value {
    if let Some(c) = config {
        return parse_config_arg(c);
    }
    if !util::is_interactive() {
        eprintln!("error: --config is required for `ingest rest` in non-interactive mode");
        std::process::exit(1);
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

/// Filesystem object-store credentials: `--config` if given, else prompt for
/// S3-style creds (all optional — blank access key = public bucket, no creds
/// sent). Non-interactive with no `--config` = public bucket.
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

/// Iceberg catalog connection config: `--config` if given, else prompt for
/// catalog URI + warehouse + token + namespace.
fn iceberg_catalog_config(config: Option<&str>) -> serde_json::Value {
    if let Some(c) = config {
        return parse_config_arg(c);
    }
    if !util::is_interactive() {
        eprintln!("error: --config is required for `ingest iceberg` in non-interactive mode");
        std::process::exit(1);
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
    let raw = if arg == "@-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .unwrap_or_else(|e| fail_config(&format!("reading stdin: {e}")));
        s
    } else if let Some(path) = arg.strip_prefix('@') {
        std::fs::read_to_string(path)
            .unwrap_or_else(|e| fail_config(&format!("reading {path}: {e}")))
    } else {
        arg.to_string()
    };
    serde_json::from_str(&raw).unwrap_or_else(|e| fail_config(&format!("invalid JSON: {e}")))
}

fn fail_config(msg: &str) -> ! {
    eprintln!("error: --config {msg}");
    std::process::exit(1);
}

/// Best-effort dialect inference from a connection URL scheme.
fn infer_dialect(url: &str) -> Option<String> {
    let scheme = url.split("://").next()?.split('+').next()?;
    match scheme {
        "postgres" | "postgresql" => Some("postgres".into()),
        "mysql" | "mariadb" => Some("mysql".into()),
        "snowflake" => Some("snowflake".into()),
        "mssql" | "sqlserver" => Some("mssql".into()),
        "redshift" => Some("redshift".into()),
        "oracle" => Some("oracle".into()),
        other if !other.is_empty() => Some(other.into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn infer_dialect_normalizes_scheme_aliases() {
        assert_eq!(
            infer_dialect("postgresql://u@h/db").as_deref(),
            Some("postgres")
        );
        assert_eq!(
            infer_dialect("postgres://u@h/db").as_deref(),
            Some("postgres")
        );
        assert_eq!(infer_dialect("mariadb://u@h/db").as_deref(), Some("mysql"));
        assert_eq!(
            infer_dialect("sqlserver://u@h/db").as_deref(),
            Some("mssql")
        );
        // dlt-style driver suffixes are stripped before matching.
        assert_eq!(
            infer_dialect("postgresql+psycopg2://u@h/db").as_deref(),
            Some("postgres")
        );
        // Unknown schemes pass through as-is (the worker validates them).
        assert_eq!(infer_dialect("duckdb://f").as_deref(), Some("duckdb"));
    }

    #[test]
    fn parse_config_arg_accepts_inline_json() {
        let v = parse_config_arg(r#"{"host": "h", "port": 5432}"#);
        assert_eq!(v["host"], "h");
        assert_eq!(v["port"], 5432);
    }

    #[test]
    fn parse_config_arg_reads_at_file() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, r#"{{"connection_string": "postgresql://u:p@h/db"}}"#).unwrap();
        let v = parse_config_arg(&format!("@{}", f.path().display()));
        assert_eq!(v["connection_string"], "postgresql://u:p@h/db");
    }

    #[test]
    fn sql_credentials_url_wins_over_everything() {
        let v = sql_credentials(
            &Some("postgresql://u:p@h:5432/db".into()),
            Some(r#"{"host": "ignored"}"#),
            &Some("also-ignored".into()),
            None,
            &None,
            &None,
            Some("postgres"),
        );
        assert_eq!(
            v,
            serde_json::json!({"connection_string": "postgresql://u:p@h:5432/db"})
        );
    }

    #[test]
    fn sql_credentials_overlays_discrete_flags_on_config() {
        // Non-interactive path (tests run without a TTY): config supplies the
        // secret, flags override the rest.
        let v = sql_credentials(
            &None,
            Some(r#"{"password": "s3cret", "host": "old-host"}"#),
            &Some("new-host".into()),
            Some(5433),
            &Some("alice".into()),
            &Some("app".into()),
            Some("postgres"),
        );
        assert_eq!(v["password"], "s3cret");
        assert_eq!(v["host"], "new-host");
        assert_eq!(v["port"], 5433);
        assert_eq!(v["username"], "alice");
        assert_eq!(v["database"], "app");
    }
}
