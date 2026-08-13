//! `hotdata ingest` — saved load definitions.
//!
//! An ingest is `datasource + selector + destination + type/schedule`. One
//! datasource can back many ingests; the ingest decides *what subset* to read
//! and *where it lands*. Every execution attempt is a run
//! (`hotdata ingest runs`, `hotdata run show`).
//!
//! **The definition is immutable.** Selector and destination are fixed at
//! creation — changing either means a new ingest. Only the schedule and the
//! lifecycle state (`cancel` / `resume` / `delete`) can move.
//!
//! **Cancel means both halves.** `cancel` stops the active run *and* stops
//! future scheduled dispatch; `resume` clears the stop and the backoff but
//! deliberately does **not** run anything immediately. Bringing the next run
//! forward is `ingest schedule <id> --next now`, which is also why there is no
//! `trigger-import` / `run-now` verb: a manual re-run would surprise a pipeline
//! whose contract is that later runs recover from the last committed state.
//!
//! **`--sql` is CLI sugar, not an API concept.** The restricted
//! `SELECT <cols> FROM [<schema>.]<table> [WHERE …] [LIMIT n]` grammar is
//! parsed here, client-side, into a structured `sql`-family selector plus a
//! destination, and the request that goes out is the same structured JSON
//! `--selector`/`--destination` would have sent. The service has no SQL
//! front-door.
//!
//! **Presentation contract:** ids are canonical everywhere (`ds_…`, `ing_…`,
//! `run_…`); display names are shown, never resolved against. Run status is a
//! closed set (queued | running | succeeded | failed | cancelled) with finer
//! progress demoted to `stage` — see `ingest_common`.

use crate::client::ingest::{Ingest, IngestClient, IngestCreate, SchedulePatch};
use crate::commands::ingest_common::{
    cell, date_cell, destination_cell, empty_notice, fail, field, hint, parse_duration,
    parse_json_arg, parse_next_run_at, presented_run_status, render, run_status_cell,
    schedule_cell, with_spinner,
};
use crate::util;

/// Wire values for the ingest type; the CLI spells the first one `one-time`.
const TYPES: [&str; 3] = ["one-time", "scheduled", "continuous"];

#[derive(clap::Subcommand)]
pub enum IngestCommands {
    /// Create a load definition (and, for --type one-time, run it once)
    ///
    /// Selector and destination are fixed at creation — changing either means a
    /// new ingest. A one-time ingest runs immediately and reports its
    /// `initial_run_id`; scheduled/continuous ones start on the next scheduler
    /// tick.
    Create {
        /// Datasource to read from (from `hotdata datasource list`)
        #[arg(long = "datasource-id")]
        datasource_id: String,

        /// one-time runs once now; scheduled and continuous need --every or
        /// --schedule
        #[arg(long = "type", value_parser = TYPES, default_value = "one-time")]
        kind: String,

        /// What to read, as family-specific JSON (inline, @file.json, or @-)
        #[arg(long, conflicts_with = "sql")]
        selector: Option<String>,

        /// SQL-family shorthand for --selector + --destination:
        /// SELECT <cols|*> FROM [<schema>.]<table> [WHERE …] [LIMIT n].
        /// Parsed here into structured JSON — the FROM target names the SOURCE
        /// table, never a datasource (that is --datasource-id).
        #[arg(long)]
        sql: Option<String>,

        /// Where it lands, as JSON (inline, @file.json, or @-):
        /// {"database_id", "schema", "table", "write_mode"}
        #[arg(
            long,
            conflicts_with_all = ["database_id", "table", "schema", "write_mode"]
        )]
        destination: Option<String>,

        /// Destination managed database id (with --table, instead of
        /// --destination)
        #[arg(long = "database-id")]
        database_id: Option<String>,

        /// Destination table (defaults to the FROM table when --sql is used)
        #[arg(long)]
        table: Option<String>,

        /// Destination schema (default: public)
        #[arg(long)]
        schema: Option<String>,

        /// How each run writes (default: replace)
        #[arg(long = "write-mode", value_parser = ["replace", "append", "upsert"])]
        write_mode: Option<String>,

        /// Schedule as JSON (inline, @file.json, or @-):
        /// {"interval_seconds", "next_run_at"}
        #[arg(long, conflicts_with_all = ["every", "next"])]
        schedule: Option<String>,

        /// Run interval for scheduled/continuous ingests, e.g. 30s, 5m, 2h, 1d
        #[arg(long)]
        every: Option<String>,

        /// When the first run may be dispatched: `now` or an RFC 3339 timestamp
        #[arg(long)]
        next: Option<String>,
    },

    /// List the ingests in this workspace
    List {
        /// Only ingests reading from this datasource
        #[arg(long = "datasource-id")]
        datasource_id: Option<String>,

        /// Only this type
        #[arg(long = "type", value_parser = TYPES)]
        kind: Option<String>,

        /// Only this lifecycle state
        #[arg(long, value_parser = ["creating", "active", "stopped", "completed", "failed", "deleted"])]
        state: Option<String>,

        /// Include soft-deleted ingests
        #[arg(long = "include-deleted")]
        include_deleted: bool,
    },

    /// Show one ingest: state, selector, destination, schedule, latest run
    Show {
        /// Ingest id (from `hotdata ingest list`)
        ingest_id: String,
    },

    /// Stop an ingest: cancel the active run AND stop future runs
    ///
    /// Both halves, deliberately — an ingest you cancelled must not come back
    /// on the next scheduler tick. Idempotent. Start it again with
    /// `hotdata ingest resume`.
    Cancel {
        /// Ingest id (from `hotdata ingest list`)
        ingest_id: String,
    },

    /// Clear a stop and let the schedule dispatch again
    ///
    /// Does NOT run anything immediately: the next run follows the schedule.
    /// To bring it forward, `hotdata ingest schedule <id> --next now`.
    /// Rejected for one-time ingests — create a new one instead.
    Resume {
        /// Ingest id (from `hotdata ingest list`)
        ingest_id: String,
    },

    /// Change when a scheduled or continuous ingest runs next
    ///
    /// Never creates an extra run. `--next now` is the supported way to make
    /// the scheduler pick an ingest up on its next tick.
    Schedule {
        /// Ingest id (from `hotdata ingest list`)
        ingest_id: String,

        /// Run interval, e.g. 30s, 5m, 2h, 1d
        #[arg(long)]
        every: Option<String>,

        /// When the next run may be dispatched: `now` or an RFC 3339 timestamp
        #[arg(long)]
        next: Option<String>,

        /// Whole schedule as JSON (inline, @file.json, or @-), instead of
        /// --every/--next
        #[arg(long, conflicts_with_all = ["every", "next"])]
        schedule: Option<String>,
    },

    /// List the runs of one ingest, newest first
    Runs {
        /// Ingest id (or pass --ingest-id)
        #[arg(required_unless_present = "ingest_id_flag")]
        ingest_id: Option<String>,

        /// Ingest id, as a flag instead of the positional
        #[arg(
            long = "ingest-id",
            value_name = "INGEST_ID",
            conflicts_with = "ingest_id"
        )]
        ingest_id_flag: Option<String>,

        /// Only runs in this status
        #[arg(long, value_parser = ["queued", "running", "succeeded", "failed", "cancelled"])]
        status: Option<String>,
    },

    /// Delete an ingest and release its destination table
    ///
    /// Soft-delete: cancels an active run first, then releases destination
    /// table ownership. The destination table and its data are never deleted,
    /// and neither is the datasource.
    Delete {
        /// Ingest id (from `hotdata ingest list`)
        ingest_id: String,
    },

    /// Verbs removed in the datasource/ingest/run split. clap's own
    /// "unrecognized subcommand" cannot say *why* `trigger-import` is gone or
    /// where `new-import` went, so catch them and answer properly.
    #[command(external_subcommand)]
    Removed(Vec<String>),
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: IngestCommands) {
    match command {
        IngestCommands::Create {
            datasource_id,
            kind,
            selector,
            sql,
            destination,
            database_id,
            table,
            schema,
            write_mode,
            schedule,
            every,
            next,
        } => {
            let plan = CreatePlan {
                datasource_id: &datasource_id,
                kind: &kind,
                sql: sql.as_deref(),
                selector: selector.as_deref().map(|a| parse_json_arg("--selector", a)),
                destination: destination
                    .as_deref()
                    .map(|a| parse_json_arg("--destination", a)),
                database_id: database_id.as_deref(),
                schema: schema.as_deref(),
                table: table.as_deref(),
                write_mode: write_mode.as_deref(),
                schedule: schedule.as_deref().map(|a| parse_json_arg("--schedule", a)),
                every: every.as_deref(),
                next: next.as_deref(),
            };
            create(workspace_id, output, plan)
        }
        IngestCommands::List {
            datasource_id,
            kind,
            state,
            include_deleted,
        } => list(
            workspace_id,
            output,
            datasource_id,
            kind,
            state,
            include_deleted,
        ),
        IngestCommands::Show { ingest_id } => show(workspace_id, output, &ingest_id),
        IngestCommands::Cancel { ingest_id } => cancel(workspace_id, output, &ingest_id),
        IngestCommands::Resume { ingest_id } => resume(workspace_id, output, &ingest_id),
        IngestCommands::Schedule {
            ingest_id,
            every,
            next,
            schedule,
        } => {
            let parsed = schedule.as_deref().map(|a| parse_json_arg("--schedule", a));
            reschedule(
                workspace_id,
                output,
                &ingest_id,
                parsed,
                every.as_deref(),
                next.as_deref(),
            )
        }
        IngestCommands::Runs {
            ingest_id,
            ingest_id_flag,
            status,
        } => {
            // clap's required_unless_present already rejects "neither", so the
            // only way here is with one of them set.
            let id = ingest_id
                .or(ingest_id_flag)
                .unwrap_or_else(|| fail("an ingest id is required (positional or --ingest-id)"));
            runs(workspace_id, output, &id, status)
        }
        IngestCommands::Delete { ingest_id } => delete(workspace_id, output, &ingest_id),
        IngestCommands::Removed(argv) => removed(&argv),
    }
}

// --- create -----------------------------------------------------------------

/// Everything `ingest create` was given, with the JSON flags already parsed.
/// Grouped so the request builder can stay pure and unit-tested.
struct CreatePlan<'a> {
    datasource_id: &'a str,
    /// CLI spelling: `one-time` | `scheduled` | `continuous`.
    kind: &'a str,
    sql: Option<&'a str>,
    selector: Option<serde_json::Value>,
    destination: Option<serde_json::Value>,
    database_id: Option<&'a str>,
    schema: Option<&'a str>,
    table: Option<&'a str>,
    write_mode: Option<&'a str>,
    schedule: Option<serde_json::Value>,
    every: Option<&'a str>,
    next: Option<&'a str>,
}

fn create(workspace_id: &str, output: &str, plan: CreatePlan) {
    let req = build_create(plan).unwrap_or_else(|m| fail(&m));
    let client = IngestClient::new(workspace_id);
    let ing = with_spinner("creating ingest…", || client.create_ingest(&req));

    render(output, &ing, || {
        use crossterm::style::Stylize;
        println!("{}", "ingest created".green());
        field("ingest id:", &ing.ingest_id);
        field("datasource id:", &cell(ing.datasource_id.as_deref()));
        field("type:", &cell(ing.r#type.as_deref()));
        field("state:", &state_cell(ing.state.as_deref()));
        match ing.initial_run_id.as_deref() {
            Some(run_id) => {
                field("run id:", run_id);
                hint(&format!("Track it with: hotdata run show {run_id}"));
            }
            None => hint(&format!(
                "It runs on its schedule. Watch it with: hotdata ingest runs {}",
                ing.ingest_id
            )),
        }
    });
}

/// Build the `POST /ingests` body. Pure (JSON is pre-parsed, errors are
/// returned) so the `--sql` desugaring, the destination assembly, and the
/// type/schedule rules — the parts a server-side 422 would otherwise be the
/// first to catch — are unit-testable.
fn build_create(plan: CreatePlan) -> Result<IngestCreate, String> {
    let wire_type = wire_type(plan.kind)?;

    // --sql desugars into BOTH halves: a structured sql-family selector and a
    // default destination table. The service never sees the SQL.
    let parsed_sql = plan.sql.map(parse_select).transpose()?;
    let selector = match (plan.selector, &parsed_sql) {
        (Some(s), _) => {
            if !s.is_object() {
                return Err("--selector must be a JSON object".into());
            }
            s
        }
        (None, Some(p)) => sql_selector(p),
        (None, None) => {
            return Err(
                "provide --selector (family-specific JSON) or --sql (SELECT … FROM \
                 [<schema>.]<table> [WHERE …] [LIMIT n])"
                    .into(),
            );
        }
    };

    let destination = build_destination(
        plan.destination,
        plan.database_id,
        plan.schema,
        plan.table,
        plan.write_mode,
        parsed_sql.as_ref().map(|p| p.table.as_str()),
    )?;

    let schedule = build_schedule(plan.schedule, plan.every, plan.next)?;
    match (wire_type, &schedule) {
        ("one_time", Some(_)) => {
            return Err(
                "a one-time ingest has no schedule — drop --every/--next/--schedule, or use \
                 --type scheduled"
                    .into(),
            );
        }
        ("scheduled" | "continuous", None) => {
            return Err(format!(
                "--type {} needs a schedule: --every 5m (optionally --next now), or --schedule \
                 @schedule.json",
                plan.kind
            ));
        }
        _ => {}
    }

    Ok(IngestCreate {
        datasource_id: plan.datasource_id.to_string(),
        r#type: wire_type.to_string(),
        selector,
        destination,
        schedule,
    })
}

/// CLI spelling → wire value. The CLI uses kebab-case like every other flag
/// value; the API uses the snake_case enum from the control store.
fn wire_type(kind: &str) -> Result<&'static str, String> {
    match kind {
        "one-time" | "one_time" => Ok("one_time"),
        "scheduled" => Ok("scheduled"),
        "continuous" => Ok("continuous"),
        other => Err(format!(
            "unknown --type '{other}' — use one-time, scheduled, or continuous"
        )),
    }
}

/// The logical write target. `--destination` is passed through (the service
/// validates it by family); otherwise it is assembled from the convenience
/// flags, with the `--sql` FROM table as the default table name.
fn build_destination(
    destination: Option<serde_json::Value>,
    database_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    write_mode: Option<&str>,
    sql_table: Option<&str>,
) -> Result<serde_json::Value, String> {
    if let Some(d) = destination {
        // Accept the file either bare or wrapped, so @destination.json can be
        // the same document a request body would carry.
        let d = match d.get("destination") {
            Some(inner) if inner.is_object() => inner.clone(),
            _ => d,
        };
        if !d.is_object() {
            return Err("--destination must be a JSON object".into());
        }
        return Ok(d);
    }

    let Some(database_id) = database_id else {
        return Err(
            "a destination is required: --destination @destination.json, or --database-id \
             <db> with --table <name>"
                .into(),
        );
    };
    let Some(table) = table.or(sql_table) else {
        return Err("--table is required (or use --sql, whose FROM table names it)".into());
    };
    Ok(serde_json::json!({
        "database_id": database_id,
        "schema": schema.unwrap_or("public"),
        "table": table,
        "write_mode": write_mode.unwrap_or("replace"),
    }))
}

/// `None` means "no schedule at all" — a one-time ingest. `--schedule` is
/// passed through (bare or wrapped); `--every`/`--next` assemble the same
/// shape.
fn build_schedule(
    schedule: Option<serde_json::Value>,
    every: Option<&str>,
    next: Option<&str>,
) -> Result<Option<serde_json::Value>, String> {
    if let Some(s) = schedule {
        let s = match s.get("schedule") {
            Some(inner) if inner.is_object() => inner.clone(),
            _ => s,
        };
        if !s.is_object() {
            return Err("--schedule must be a JSON object".into());
        }
        return Ok(Some(s));
    }
    if every.is_none() && next.is_none() {
        return Ok(None);
    }
    let mut m = serde_json::Map::new();
    if let Some(e) = every {
        m.insert("interval_seconds".into(), parse_duration(e)?.into());
    }
    if let Some(n) = next {
        m.insert("next_run_at".into(), parse_next_run_at(n).into());
    }
    Ok(Some(serde_json::Value::Object(m)))
}

// --- the SELECT convenience -------------------------------------------------

/// One parsed `--sql` argument. Deliberately not a general SQL AST: this
/// grammar exists only to fill in a `sql`-family selector, and anything it
/// cannot express is rejected with a pointer at `--selector`.
#[derive(Debug, PartialEq)]
struct ParsedSelect {
    /// Empty means `*` — every column.
    columns: Vec<String>,
    schema: Option<String>,
    table: String,
    /// The WHERE text verbatim; the source engine parses it, not the CLI.
    filter: Option<String>,
    limit: Option<u64>,
}

/// Parse `SELECT <cols|*> FROM [<schema>.]<table> [WHERE …] [LIMIT n]`.
///
/// The FROM target names the **source table**, optionally schema-qualified.
/// It is not a datasource name: names are never resolved against (the
/// datasource is `--datasource-id`), so `FROM prod_pg.orders` means schema
/// `prod_pg`, table `orders`.
fn parse_select(sql: &str) -> Result<ParsedSelect, String> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.contains(';') {
        return Err("--sql takes a single SELECT statement".into());
    }
    // ASCII uppercasing is byte-length preserving, so indices found in `upper`
    // are valid in `trimmed`.
    let upper = trimmed.to_ascii_uppercase();
    if !upper.starts_with("SELECT ") {
        return Err("--sql must start with SELECT — use --selector for other shapes".into());
    }
    for kw in [" JOIN ", " UNION ", " GROUP BY ", " ORDER BY ", " HAVING "] {
        if upper.contains(kw) {
            return Err(format!(
                "--sql is a restricted grammar (SELECT … FROM … [WHERE …] [LIMIT n]) and cannot \
                 express{kw}— pass the full query as --selector \
                 '{{\"mode\":\"query\",\"query\":{{\"sql\":\"…\"}}}}'"
            ));
        }
    }

    let from = upper
        .find(" FROM ")
        .ok_or("--sql needs a FROM clause: SELECT <cols|*> FROM [<schema>.]<table>")?;
    let columns_text = trimmed["SELECT ".len()..from].trim();
    let rest = trimmed[from + " FROM ".len()..].trim();
    let rest_upper = rest.to_ascii_uppercase();

    // A trailing LIMIT only counts when what follows is a bare number —
    // otherwise the word came from inside the WHERE text.
    let limit_at = rest_upper.rfind(" LIMIT ").filter(|i| {
        rest[i + " LIMIT ".len()..]
            .trim()
            .parse::<u64>()
            .map(|n| n > 0)
            .unwrap_or(false)
    });
    let limit = limit_at.map(|i| rest[i + " LIMIT ".len()..].trim().parse::<u64>().unwrap());

    let where_at = rest_upper.find(" WHERE ").filter(|w| match limit_at {
        Some(l) => *w < l,
        None => true,
    });
    let target_end = where_at.or(limit_at).unwrap_or(rest.len());
    let target = rest[..target_end].trim();
    let filter = where_at.map(|w| {
        let start = w + " WHERE ".len();
        let end = limit_at.unwrap_or(rest.len());
        rest[start..end].trim().to_string()
    });

    let (schema, table) = parse_target(target)?;
    Ok(ParsedSelect {
        columns: parse_columns(columns_text)?,
        schema,
        table,
        filter: filter.filter(|f| !f.is_empty()),
        limit,
    })
}

fn parse_columns(text: &str) -> Result<Vec<String>, String> {
    if text.is_empty() {
        return Err("--sql needs a column list or * after SELECT".into());
    }
    if text == "*" {
        return Ok(Vec::new());
    }
    if text.contains('(') {
        return Err(
            "--sql selects plain columns only — expressions and aggregates belong in a \
             --selector with \"mode\":\"query\""
                .into(),
        );
    }
    let columns: Vec<String> = text
        .split(',')
        .map(|c| unquote(c.trim()).to_string())
        .filter(|c| !c.is_empty())
        .collect();
    if columns.is_empty() {
        return Err("--sql needs a column list or * after SELECT".into());
    }
    Ok(columns)
}

fn parse_target(target: &str) -> Result<(Option<String>, String), String> {
    if target.is_empty() {
        return Err("--sql needs a table after FROM".into());
    }
    if target.contains('(') || target.contains(' ') {
        return Err(format!(
            "'{target}' is not a plain table name — --sql reads FROM [<schema>.]<table> only \
             (the datasource is --datasource-id)"
        ));
    }
    match target.split_once('.') {
        Some((schema, table)) => {
            let table = unquote(table);
            if table.is_empty() {
                return Err("--sql needs a table after FROM".into());
            }
            Ok((Some(unquote(schema).to_string()), table.to_string()))
        }
        None => Ok((None, unquote(target).to_string())),
    }
}

/// Strip one layer of SQL quoting from an identifier.
fn unquote(s: &str) -> &str {
    for q in ['"', '`', '\''] {
        if s.len() >= 2 && s.starts_with(q) && s.ends_with(q) {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// The `sql`-family selector a parsed SELECT desugars to. The service sees
/// only this — the SQL text never leaves the CLI.
fn sql_selector(p: &ParsedSelect) -> serde_json::Value {
    let mut table = serde_json::Map::new();
    if let Some(s) = &p.schema {
        table.insert("schema".into(), s.clone().into());
    }
    table.insert("table".into(), p.table.clone().into());
    if !p.columns.is_empty() {
        table.insert("columns".into(), p.columns.clone().into());
    }
    if let Some(f) = &p.filter {
        table.insert("where".into(), f.clone().into());
    }
    if let Some(n) = p.limit {
        table.insert("limit".into(), n.into());
    }
    serde_json::json!({
        "mode": "tables",
        "tables": [serde_json::Value::Object(table)],
    })
}

// --- list -------------------------------------------------------------------

fn list(
    workspace_id: &str,
    output: &str,
    datasource_id: Option<String>,
    kind: Option<String>,
    state: Option<String>,
    include_deleted: bool,
) {
    let mut filters: Vec<(&str, String)> = Vec::new();
    if let Some(d) = datasource_id {
        filters.push(("datasource_id", d));
    }
    if let Some(k) = kind {
        filters.push(("type", wire_type(&k).unwrap_or_else(|m| fail(&m)).into()));
    }
    if let Some(s) = state {
        filters.push(("state", s));
    }
    if include_deleted {
        filters.push(("include_deleted", "true".into()));
    }

    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("loading ingests…", || client.list_ingests(&filters));

    render(output, &resp.ingests, || {
        if resp.ingests.is_empty() {
            empty_notice(
                "No ingests yet. Create one with 'hotdata ingest create --datasource-id <id> \
                 --type one-time --selector @selector.json --destination @destination.json'.",
            );
            return;
        }
        let rows: Vec<Vec<String>> = resp
            .ingests
            .iter()
            // Oldest at the top, newest at the bottom — the freshest row lands
            // next to the prompt. (The server returns newest-first; json/yaml
            // keep that order for scripting.)
            .rev()
            .map(|i| {
                vec![
                    i.ingest_id.clone(),
                    cell(i.datasource_id.as_deref()),
                    cell(i.r#type.as_deref()),
                    state_cell(i.state.as_deref()),
                    destination_cell(
                        i.destination_database_id.as_deref(),
                        i.destination_schema.as_deref(),
                        i.destination_table.as_deref(),
                    ),
                    schedule_cell(i.schedule.as_ref(), i.next_attempt_at.as_deref()),
                    date_cell(i.created_at.as_deref()),
                ]
            })
            .collect();
        crate::output::table::print(
            &[
                "INGEST ID",
                "DATASOURCE ID",
                "TYPE",
                "STATE",
                "DESTINATION",
                "SCHEDULE",
                "CREATED",
            ],
            &rows,
        );
    });
}

// --- show -------------------------------------------------------------------

fn show(workspace_id: &str, output: &str, ingest_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ing = client.get_ingest(ingest_id).unwrap_or_else(|e| e.exit());

    render(output, &ing, || {
        print_ingest_identity(&ing);
        if let Some(s) = ing.selector.as_ref() {
            field("selector:", &compact_json(s));
        }
        if let Some(t) = ing.created_at.as_deref() {
            field("created:", &util::format_date(t));
        }
        if let Some(r) = ing.latest_run.as_ref() {
            field(
                "latest run:",
                &format!(
                    "{}  {}",
                    r.run_id,
                    run_status_cell(&r.status, r.stage.as_deref())
                ),
            );
        }
        hint(&format!("Its runs: hotdata ingest runs {}", ing.ingest_id));
    });
}

/// The identity block every ingest view opens with. One definition so `show`,
/// `resume`, and `schedule` cannot disagree about what an ingest *is*.
fn print_ingest_identity(ing: &Ingest) {
    field("ingest id:", &ing.ingest_id);
    field("datasource id:", &cell(ing.datasource_id.as_deref()));
    field("type:", &cell(ing.r#type.as_deref()));
    field("state:", &state_cell(ing.state.as_deref()));
    if let Some(r) = ing
        .stopped_reason
        .as_deref()
        .filter(|r| !r.trim().is_empty())
    {
        field("stopped by:", r);
    }
    field(
        "destination:",
        &destination_cell(
            ing.destination_database_id.as_deref(),
            ing.destination_schema.as_deref(),
            ing.destination_table.as_deref(),
        ),
    );
    field(
        "schedule:",
        &schedule_cell(ing.schedule.as_ref(), ing.next_attempt_at.as_deref()),
    );
}

/// STATE cell for ingests and datasources. The lifecycle vocabulary is the
/// server's; `color_status` already greens the terminal-good ones and leaves
/// everything else in flight yellow.
fn state_cell(state: Option<&str>) -> String {
    state.map(util::color_status).unwrap_or_else(|| "-".into())
}

fn compact_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "-".into())
}

// --- cancel / resume / delete ------------------------------------------------

fn cancel(workspace_id: &str, output: &str, ingest_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("stopping ingest…", || client.cancel_ingest(ingest_id));

    render(output, &ack, || {
        field("ingest id:", &ack.ingest_id);
        field("state:", &state_cell(ack.state.as_deref()));
        match ack.cancelled_run_id.as_deref() {
            Some(run_id) => field("cancelled run:", run_id),
            None => field("cancelled run:", "- (none was in flight)"),
        }
        hint(&format!(
            "Future runs are stopped too. Start it again with: hotdata ingest resume {}",
            ack.ingest_id
        ));
    });
}

fn resume(workspace_id: &str, output: &str, ingest_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ing = with_spinner("resuming ingest…", || client.resume_ingest(ingest_id));

    render(output, &ing, || {
        print_ingest_identity(&ing);
        // DR-12: resume is not a run trigger, and saying so here is cheaper
        // than a support question.
        hint(&format!(
            "No run was started — the next one follows the schedule. To bring it forward: \
             hotdata ingest schedule {} --next now",
            ing.ingest_id
        ));
    });
}

fn delete(workspace_id: &str, output: &str, ingest_id: &str) {
    let client = IngestClient::new(workspace_id);
    let ack = with_spinner("deleting ingest…", || client.delete_ingest(ingest_id));

    render(output, &ack, || {
        use crossterm::style::Stylize;
        println!("{} {}", "ingest deleted".green(), ingest_id.dark_grey());
        hint("The destination table and its data were not touched.");
    });
}

// --- schedule ----------------------------------------------------------------

fn reschedule(
    workspace_id: &str,
    output: &str,
    ingest_id: &str,
    schedule: Option<serde_json::Value>,
    every: Option<&str>,
    next: Option<&str>,
) {
    let Some(schedule) = build_schedule(schedule, every, next).unwrap_or_else(|m| fail(&m)) else {
        fail("nothing to change — pass --every 5m, --next now, or --schedule @schedule.json");
    };
    let client = IngestClient::new(workspace_id);
    let ing = with_spinner("updating schedule…", || {
        client.update_schedule(ingest_id, &SchedulePatch { schedule })
    });

    render(output, &ing, || {
        print_ingest_identity(&ing);
        hint("No extra run was created — the change applies to future dispatch only.");
    });
}

// --- runs --------------------------------------------------------------------

fn runs(workspace_id: &str, output: &str, ingest_id: &str, status: Option<String>) {
    let mut filters: Vec<(&str, String)> = Vec::new();
    if let Some(s) = status {
        filters.push(("status", s));
    }

    let client = IngestClient::new(workspace_id);
    let resp = with_spinner("loading runs…", || client.list_runs(ingest_id, &filters));

    let projected: Vec<_> = resp
        .runs
        .iter()
        .map(|r| {
            let (status, stage) = presented_run_status(&r.status, r.stage.as_deref());
            serde_json::json!({
                "run_id": r.run_id,
                "ingest_id": r.ingest_id,
                "attempt": r.attempt,
                "status": status,
                "stage": stage,
                "detail": r.detail,
                "config_version_id": r.config_version_id,
                "queued_at": r.queued_at,
                "started_at": r.started_at,
                "finished_at": r.finished_at,
            })
        })
        .collect();
    render(output, &projected, || {
        if resp.runs.is_empty() {
            empty_notice(&format!(
                "No runs for {ingest_id} yet. A scheduled ingest starts on the next tick; \
                 bring it forward with 'hotdata ingest schedule {ingest_id} --next now'."
            ));
            return;
        }
        let rows: Vec<Vec<String>> = resp
            .runs
            .iter()
            // Oldest at the top, newest at the bottom (see `list`).
            .rev()
            .map(|r| {
                vec![
                    r.run_id.clone(),
                    run_status_cell(&r.status, r.stage.as_deref()),
                    r.attempt
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "-".into()),
                    date_cell(r.started_at.as_deref().or(r.queued_at.as_deref())),
                    date_cell(r.finished_at.as_deref()),
                    cell(r.detail.as_deref()),
                ]
            })
            .collect();
        crate::output::table::print(
            &[
                "RUN ID", "STATUS", "ATTEMPT", "STARTED", "FINISHED", "DETAIL",
            ],
            &rows,
        );
    });
}

// --- removed verbs ------------------------------------------------------------

/// What to say when someone types a verb from before the split. Pure so the
/// mapping is pinned by tests rather than by whichever message was edited last.
fn removal_message(verb: &str) -> Option<String> {
    let replacement = match verb {
        "new-datasource" | "new-connection" => {
            "hotdata datasource create --family <f> --config @source.json"
        }
        "list-datasources" | "list-connections" => "hotdata datasource list",
        "show-datasource" | "show-connection" => "hotdata datasource show <datasource-id>",
        "delete-datasource" | "delete-connection" => "hotdata datasource delete <datasource-id>",
        "datasources" | "connectors" => "hotdata datasource types",
        "new-import" => {
            "hotdata ingest create --datasource-id <id> --type one-time --sql \"SELECT …\" \
             --database-id <db>"
        }
        "list-imports" => "hotdata ingest list",
        "status" => "hotdata run show <run-id>  (or: hotdata ingest runs <ingest-id>)",
        "raw-sql" => {
            "hotdata ingest create --datasource-id <id> --selector \
             '{\"mode\":\"query\",\"query\":{\"sql\":\"…\"}}' --database-id <db> --table <t>"
        }
        // The one verb with no replacement at all — say why, not just "gone".
        "trigger-import" | "rerun" | "run-now" => {
            return Some(format!(
                "'hotdata ingest {verb}' was removed and has no replacement.\n\
                 A one-time ingest runs when it is created, and a scheduled or continuous one \
                 runs on its schedule — each run recovers from the last committed state, so an \
                 out-of-band re-run would race the pipeline rather than repair it.\n\
                 \n\
                 To make the next scheduled run happen now:\n    \
                 hotdata ingest schedule <ingest-id> --next now\n\
                 To load the same data again from scratch:\n    \
                 hotdata ingest create --datasource-id <id> --type one-time …\n\
                 To restart an ingest you stopped:\n    \
                 hotdata ingest resume <ingest-id>"
            ));
        }
        _ => return None,
    };
    Some(format!(
        "'hotdata ingest {verb}' was removed in the datasource/ingest/run split.\n\
         Use instead:\n    {replacement}"
    ))
}

fn removed(argv: &[String]) -> ! {
    use crossterm::style::Stylize;
    let verb = argv.first().map(String::as_str).unwrap_or("");
    match removal_message(verb) {
        Some(msg) => eprintln!("{}", format!("error: {msg}").red()),
        None => {
            eprintln!(
                "{}",
                format!("error: unrecognized subcommand 'hotdata ingest {verb}'").red()
            );
            eprintln!(
                "{}",
                "Verbs: create, list, show, cancel, resume, schedule, runs, delete. \
                 Datasources are 'hotdata datasource', runs are 'hotdata run'."
                    .dark_grey()
            );
        }
    }
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the SELECT convenience ----------------------------------------------

    #[test]
    fn select_parses_columns_table_where_and_limit() {
        let p =
            parse_select("SELECT id, status FROM public.orders WHERE status = 'open' LIMIT 100")
                .unwrap();
        assert_eq!(p.columns, vec!["id", "status"]);
        assert_eq!(p.schema.as_deref(), Some("public"));
        assert_eq!(p.table, "orders");
        assert_eq!(p.filter.as_deref(), Some("status = 'open'"));
        assert_eq!(p.limit, Some(100));
    }

    #[test]
    fn select_star_and_bare_table_are_the_minimum_form() {
        let p = parse_select("SELECT * FROM orders").unwrap();
        assert!(p.columns.is_empty(), "* means every column, not a column");
        assert_eq!(p.schema, None);
        assert_eq!(p.table, "orders");
        assert_eq!(p.filter, None);
        assert_eq!(p.limit, None);
    }

    #[test]
    fn select_is_case_insensitive_and_tolerates_quoting_and_semicolons() {
        let p = parse_select("select \"id\" from \"public\".\"orders\" limit 5;").unwrap();
        assert_eq!(p.columns, vec!["id"]);
        assert_eq!(p.schema.as_deref(), Some("public"));
        assert_eq!(p.table, "orders");
        assert_eq!(p.limit, Some(5));
    }

    #[test]
    fn select_does_not_mistake_the_word_limit_inside_a_predicate() {
        // "LIMIT" in a string literal is not a LIMIT clause: only a trailing
        // bare number counts.
        let p = parse_select("SELECT * FROM t WHERE name = 'over the LIMIT now'").unwrap();
        assert_eq!(p.limit, None);
        assert_eq!(p.filter.as_deref(), Some("name = 'over the LIMIT now'"));
    }

    #[test]
    fn select_rejects_what_the_grammar_cannot_express() {
        for bad in [
            "SELECT a FROM x JOIN y ON x.id = y.id",
            "SELECT a, count(*) FROM x GROUP BY a",
            "SELECT a FROM x ORDER BY a",
            "SELECT a FROM x UNION SELECT a FROM y",
        ] {
            let err = parse_select(bad).unwrap_err();
            assert!(err.contains("--selector"), "{bad}: {err}");
        }
        // Not a SELECT at all, and a SELECT with no FROM.
        assert!(
            parse_select("DELETE FROM x")
                .unwrap_err()
                .contains("SELECT")
        );
        assert!(parse_select("SELECT 1").unwrap_err().contains("FROM"));
        // Multiple statements.
        assert!(
            parse_select("SELECT * FROM a; DROP TABLE b")
                .unwrap_err()
                .contains("single SELECT")
        );
    }

    #[test]
    fn select_desugars_to_a_structured_sql_selector() {
        // The whole point: the SQL text never reaches the API.
        let p = parse_select("SELECT id FROM public.orders WHERE id > 5 LIMIT 10").unwrap();
        let selector = sql_selector(&p);
        assert_eq!(
            selector,
            serde_json::json!({
                "mode": "tables",
                "tables": [{
                    "schema": "public",
                    "table": "orders",
                    "columns": ["id"],
                    "where": "id > 5",
                    "limit": 10,
                }],
            })
        );
        // SELECT * omits `columns` entirely rather than sending an empty list.
        let star = sql_selector(&parse_select("SELECT * FROM orders").unwrap());
        assert_eq!(
            star,
            serde_json::json!({"mode": "tables", "tables": [{"table": "orders"}]})
        );
    }

    // --- request construction -------------------------------------------------

    fn plan<'a>(datasource_id: &'a str, kind: &'a str) -> CreatePlan<'a> {
        CreatePlan {
            datasource_id,
            kind,
            sql: None,
            selector: None,
            destination: None,
            database_id: None,
            schema: None,
            table: None,
            write_mode: None,
            schedule: None,
            every: None,
            next: None,
        }
    }

    #[test]
    fn create_sends_the_structured_body_for_the_sql_shorthand() {
        let mut p = plan("ds_pg", "one-time");
        p.sql = Some("SELECT id, status FROM public.orders WHERE status = 'open'");
        p.database_id = Some("db_123");
        let req = build_create(p).unwrap();

        assert_eq!(req.datasource_id, "ds_pg");
        assert_eq!(req.r#type, "one_time"); // kebab in, snake out
        assert_eq!(req.selector["mode"], "tables");
        assert_eq!(req.selector["tables"][0]["table"], "orders");
        // The destination table defaults to the FROM table.
        assert_eq!(
            req.destination,
            serde_json::json!({
                "database_id": "db_123",
                "schema": "public",
                "table": "orders",
                "write_mode": "replace",
            })
        );
        assert!(req.schedule.is_none());
    }

    #[test]
    fn create_passes_explicit_selector_and_destination_through_untouched() {
        let selector = serde_json::json!({
            "prefix": "orders/", "glob": "**/*.parquet", "format": "parquet"
        });
        let destination = serde_json::json!({
            "database_id": "db_456", "schema": "public",
            "table": "orders_raw", "write_mode": "upsert"
        });
        let mut p = plan("ds_s3", "continuous");
        p.selector = Some(selector.clone());
        p.destination = Some(destination.clone());
        p.every = Some("5m");
        p.next = Some("now");
        let req = build_create(p).unwrap();

        assert_eq!(req.selector, selector);
        assert_eq!(req.destination, destination);
        assert_eq!(
            req.schedule.unwrap(),
            serde_json::json!({"interval_seconds": 300, "next_run_at": "now"})
        );
    }

    #[test]
    fn create_unwraps_a_wrapped_destination_or_schedule_document() {
        // @destination.json may be either the bare object or the same document
        // a request body carries.
        let mut p = plan("ds_1", "scheduled");
        p.selector = Some(serde_json::json!({"mode": "tables"}));
        p.destination = Some(serde_json::json!({
            "destination": {"database_id": "db_1", "table": "t"}
        }));
        p.schedule = Some(serde_json::json!({"schedule": {"interval_seconds": 600}}));
        let req = build_create(p).unwrap();
        assert_eq!(req.destination["database_id"], "db_1");
        assert_eq!(req.schedule.unwrap()["interval_seconds"], 600);
    }

    #[test]
    fn create_requires_a_selector_and_a_destination() {
        let mut p = plan("ds_1", "one-time");
        assert!(build_create(p).unwrap_err().contains("--selector"));

        // Selector but no destination.
        p = plan("ds_1", "one-time");
        p.selector = Some(serde_json::json!({"mode": "tables"}));
        assert!(build_create(p).unwrap_err().contains("--destination"));

        // A database with no table and no --sql to name one.
        p = plan("ds_1", "one-time");
        p.selector = Some(serde_json::json!({"mode": "tables"}));
        p.database_id = Some("db_1");
        assert!(build_create(p).unwrap_err().contains("--table"));
    }

    #[test]
    fn schedule_rules_follow_the_ingest_type() {
        // scheduled/continuous need one.
        let mut p = plan("ds_1", "continuous");
        p.selector = Some(serde_json::json!({}));
        p.database_id = Some("db_1");
        p.table = Some("t");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("--every"), "{err}");

        // one-time must not have one.
        let mut p = plan("ds_1", "one-time");
        p.selector = Some(serde_json::json!({}));
        p.database_id = Some("db_1");
        p.table = Some("t");
        p.every = Some("5m");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("one-time ingest has no schedule"), "{err}");
    }

    #[test]
    fn destination_flags_default_schema_and_write_mode() {
        let d = build_destination(None, Some("db_1"), None, Some("t"), None, None).unwrap();
        assert_eq!(d["schema"], "public");
        assert_eq!(d["write_mode"], "replace");
        // Explicit values win.
        let d = build_destination(
            None,
            Some("db_1"),
            Some("staging"),
            Some("t"),
            Some("upsert"),
            None,
        )
        .unwrap();
        assert_eq!(d["schema"], "staging");
        assert_eq!(d["write_mode"], "upsert");
        // --table beats the FROM table when both are present.
        let d = build_destination(
            None,
            Some("db_1"),
            None,
            Some("renamed"),
            None,
            Some("orders"),
        )
        .unwrap();
        assert_eq!(d["table"], "renamed");
    }

    #[test]
    fn schedule_builds_from_every_and_next_independently() {
        // --next alone is legal: "run on the next tick", no interval change.
        let s = build_schedule(None, None, Some("now")).unwrap().unwrap();
        assert_eq!(s, serde_json::json!({"next_run_at": "now"}));
        // --every alone leaves the next dispatch to the server.
        let s = build_schedule(None, Some("2h"), None).unwrap().unwrap();
        assert_eq!(s, serde_json::json!({"interval_seconds": 7200}));
        // Neither means "no schedule", which is what a one-time ingest sends.
        assert!(build_schedule(None, None, None).unwrap().is_none());
        // A bad duration is the CLI's error, not a server 422.
        assert!(build_schedule(None, Some("soon"), None).is_err());
    }

    #[test]
    fn type_spellings_map_to_the_wire_enum() {
        assert_eq!(wire_type("one-time").unwrap(), "one_time");
        assert_eq!(wire_type("one_time").unwrap(), "one_time");
        assert_eq!(wire_type("scheduled").unwrap(), "scheduled");
        assert_eq!(wire_type("continuous").unwrap(), "continuous");
        assert!(wire_type("hourly").is_err());
    }

    // --- removed verbs ---------------------------------------------------------

    #[test]
    fn trigger_import_removal_says_why_and_what_to_do_instead() {
        let msg = removal_message("trigger-import").unwrap();
        assert!(msg.contains("no replacement"), "{msg}");
        assert!(msg.contains("last committed state"), "{msg}");
        assert!(msg.contains("--next now"), "{msg}");
    }

    #[test]
    fn old_verbs_name_their_replacements() {
        for (verb, needle) in [
            ("new-datasource", "hotdata datasource create"),
            ("list-datasources", "hotdata datasource list"),
            ("show-datasource", "hotdata datasource show"),
            ("delete-datasource", "hotdata datasource delete"),
            ("datasources", "hotdata datasource types"),
            ("new-import", "hotdata ingest create"),
            ("list-imports", "hotdata ingest list"),
            ("status", "hotdata run show"),
            ("raw-sql", "--selector"),
        ] {
            let msg = removal_message(verb).unwrap_or_else(|| panic!("{verb} unmapped"));
            assert!(msg.contains(needle), "{verb}: {msg}");
        }
        // A genuine typo is not claimed to be a removed verb.
        assert!(removal_message("crate").is_none());
    }
}
