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
//! **The shorthand flags are CLI sugar, not API concepts.** `--table`,
//! `--schema`, `--format`, `--glob`, `--record-shape`, `--all`, `--raw-sql` and
//! `--sql` all BUILD the one document `--selector` would have carried, and
//! `--selector` stays the escape hatch for anything they cannot say. Nothing
//! new goes on the wire: the request from `--table orders` is the request from
//! the equivalent `--selector` JSON, byte for byte, which is what the contract
//! tests at the bottom of this file hold still.
//!
//! `--sql` and `--raw-sql` are the two that carry SQL, and neither sends any.
//! `--sql`'s restricted `SELECT <cols> FROM [<schema>.]<table> [WHERE …]
//! [LIMIT n]` grammar is parsed HERE into a structured selector plus a
//! destination; `--raw-sql` sends the statement as the `sql` field of a
//! query-mode selector, for the source engine to run in its own dialect. The
//! service has no SQL front-door either way.
//!
//! **Waiting is watching.** `--wait` polls; the scheduler owns dispatch, so
//! nothing here can make a queued run start. `ingest schedule <id> --next now`
//! is what moves the next one forward.
//!
//! **Presentation contract:** ids are canonical everywhere (`ds_…`, `ing_…`,
//! `run_…`); display names are shown, never resolved against. Run status is a
//! closed set (queued | running | succeeded | failed | cancelled) with finer
//! progress demoted to `stage` — see `ingest_common`.

use crate::client::ingest::{Ingest, IngestClient, IngestCreate, SchedulePatch};
use crate::commands::ingest_common::{
    cell, date_cell, destination_cell, empty_notice, fail, field, hint, is_terminal,
    parse_duration, parse_json_arg, parse_next_run_at, poll_until, presented_run_status, render,
    run_status_cell, schedule_cell, wait_timed_out, with_spinner,
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
    ///
    /// The fields --selector takes, and which write modes and types the
    /// datasource's family supports: `hotdata datasource fields <family>`.
    Create {
        /// Datasource to read from, by `ds_…` id (from `hotdata datasource
        /// list`). --source takes a display name too.
        #[arg(long = "datasource-id", required_unless_present = "source")]
        datasource_id: Option<String>,

        /// Datasource to read from: a `ds_…` id, or a display name resolved
        /// here against `hotdata datasource list`. Two datasources sharing a
        /// name is an error listing both — names are labels, not identity, so
        /// nothing picks between them for you.
        #[arg(long, conflicts_with = "datasource_id")]
        source: Option<String>,

        /// one-time runs once now; scheduled and continuous need --every or
        /// --schedule
        #[arg(long = "type", value_parser = TYPES, default_value = "one-time")]
        kind: String,

        /// What to read, as family-specific JSON (inline, @file.json, or @-).
        /// The escape hatch the shorthand flags below build for you.
        /// Field reference: `hotdata datasource fields <family>` (SELECTOR).
        #[arg(
            long,
            conflicts_with_all = ["sql", "raw_sql", "all", "tables", "schema",
                                  "format", "glob", "record_shape", "limit"]
        )]
        selector: Option<String>,

        /// SQL-family shorthand for --selector + --destination:
        /// SELECT <cols|*> FROM [<schema>.]<table> [WHERE …] [LIMIT n].
        /// Parsed here into structured JSON — the FROM target names the SOURCE
        /// table, never a datasource (that is --source).
        #[arg(
            long,
            conflicts_with_all = ["raw_sql", "all", "tables", "schema", "format",
                                  "glob", "record_shape", "limit"]
        )]
        sql: Option<String>,

        /// Run one read-only statement in the SOURCE engine's own dialect and
        /// load its result set (SQL sources).
        ///
        /// Unlike --sql's restricted grammar, this runs verbatim at the source:
        /// joins, aggregates, CTEs and window functions all execute there and
        /// only the result transfers. The result lands in --table.
        #[arg(
            long = "raw-sql",
            conflicts_with_all = ["all", "schema", "format", "glob", "record_shape"]
        )]
        raw_sql: Option<String>,

        /// Load everything the datasource exposes, with nothing narrowed
        #[arg(long, conflicts_with_all = ["tables", "schema"])]
        all: bool,

        /// Source table to load, repeatable (SQL, Iceberg, DuckLake sources).
        /// The destination table follows the single one named, unless
        /// --dest-table says otherwise. With --raw-sql it names the result
        /// table, since a query has no source table to take a name from.
        #[arg(long = "table")]
        tables: Vec<String>,

        /// Source schema the tables live in (SQL sources)
        #[arg(long)]
        schema: Option<String>,

        /// File format to read (bucket sources): csv, jsonl, or parquet
        #[arg(long, value_parser = ["csv", "jsonl", "parquet"])]
        format: Option<String>,

        /// Which keys under the datasource root to read (bucket sources),
        /// e.g. **/*.parquet
        #[arg(long)]
        glob: Option<String>,

        /// Flatten each source record into rows with a named shape (bucket
        /// sources). The shapes: otel_traces, mqtt_observations.
        ///
        /// Not checked here: the service owns the list, so a shape added there
        /// is usable without a CLI release, and an unknown one comes back as a
        /// 422 naming the shapes that exist.
        #[arg(long = "record-shape")]
        record_shape: Option<String>,

        /// Stop after N source rows
        #[arg(long, conflicts_with = "sql")]
        limit: Option<u64>,

        /// Where it lands, as JSON (inline, @file.json, or @-):
        /// {"database_id", "schema", "table", "write_mode"}
        #[arg(
            long,
            conflicts_with_all = ["database_id", "dest_table", "dest_schema", "write_mode"]
        )]
        destination: Option<String>,

        /// Destination managed database id
        #[arg(long = "database-id")]
        database_id: Option<String>,

        /// Destination table name. Defaults to the single --table, or to the
        /// FROM table of --sql.
        #[arg(long = "dest-table")]
        dest_table: Option<String>,

        /// Destination schema (default: public)
        #[arg(long = "dest-schema")]
        dest_schema: Option<String>,

        /// How each run writes (default: replace). `upsert` needs a family
        /// whose load path stamps a row key — today that is a continuous bucket
        /// ingest, and `hotdata datasource fields <family>` reports which modes
        /// a family accepts for which type.
        ///
        /// The two listed are the two the destination accepts anywhere. Offering
        /// a third would be offering a request that is refused on arrival, which
        /// costs the user a round trip to learn what `--help` could have said.
        #[arg(long = "write-mode", value_parser = ["replace", "upsert"])]
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

        /// Watch until the newest run finishes.
        ///
        /// Polling only: the scheduler decides when a run starts, so this
        /// cannot bring one forward (`ingest schedule <id> --next now` can). A
        /// recurring ingest keeps producing runs, so it returns on the first
        /// one to reach a terminal status.
        #[arg(long)]
        wait: bool,

        /// Seconds to watch with --wait (default 300)
        #[arg(long = "wait-timeout", default_value = "300")]
        wait_timeout: u64,
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
            source,
            kind,
            selector,
            sql,
            raw_sql,
            all,
            tables,
            schema,
            format,
            glob,
            record_shape,
            limit,
            destination,
            database_id,
            dest_table,
            dest_schema,
            write_mode,
            schedule,
            every,
            next,
        } => {
            let client = IngestClient::new(workspace_id);
            // clap's required_unless_present already rejects "neither".
            let named = datasource_id
                .or(source)
                .unwrap_or_else(|| fail("a datasource is required: --source or --datasource-id"));
            let resolved = resolve_datasource(&client, &named);
            // Only the --table shorthand needs the family, and only to decide
            // one key, so the lookup is paid for only when it is used: an
            // explicit --selector, --sql, --raw-sql or a bucket shorthand all
            // say enough on their own.
            let family = if tables.is_empty() || selector.is_some() || sql.is_some() {
                resolved.family
            } else {
                resolved.family.or_else(|| {
                    with_spinner("reading the datasource…", || {
                        client.get_datasource(&resolved.id)
                    })
                    .family
                })
            };
            let plan = CreatePlan {
                datasource_id: &resolved.id,
                family: family.as_deref(),
                kind: &kind,
                sql: sql.as_deref(),
                raw_sql: raw_sql.as_deref(),
                all,
                tables: &tables,
                schema: schema.as_deref(),
                format: format.as_deref(),
                glob: glob.as_deref(),
                record_shape: record_shape.as_deref(),
                limit,
                selector: selector.as_deref().map(|a| parse_json_arg("--selector", a)),
                destination: destination
                    .as_deref()
                    .map(|a| parse_json_arg("--destination", a)),
                database_id: database_id.as_deref(),
                dest_schema: dest_schema.as_deref(),
                dest_table: dest_table.as_deref(),
                write_mode: write_mode.as_deref(),
                schedule: schedule.as_deref().map(|a| parse_json_arg("--schedule", a)),
                every: every.as_deref(),
                next: next.as_deref(),
            };
            create(&client, output, plan)
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
            wait,
            wait_timeout,
        } => {
            // clap's required_unless_present already rejects "neither", so the
            // only way here is with one of them set.
            let id = ingest_id
                .or(ingest_id_flag)
                .unwrap_or_else(|| fail("an ingest id is required (positional or --ingest-id)"));
            runs(workspace_id, output, &id, status, wait, wait_timeout)
        }
        IngestCommands::Delete { ingest_id } => delete(workspace_id, output, &ingest_id),
        IngestCommands::Removed(argv) => removed(&argv),
    }
}

// --- create -----------------------------------------------------------------

/// Everything `ingest create` was given, with the JSON flags already parsed.
/// Grouped so the request builder can stay pure and unit-tested.
struct CreatePlan<'a> {
    /// Already resolved to a `ds_…` id: a display name is turned into one
    /// here, client-side, because the API resolves ids and nothing else.
    datasource_id: &'a str,
    /// The datasource's family, when the shorthand being used needs it. Read
    /// only by the `--table` path, and only to decide one key — see
    /// [`build_selector`].
    family: Option<&'a str>,
    /// CLI spelling: `one-time` | `scheduled` | `continuous`.
    kind: &'a str,
    sql: Option<&'a str>,
    raw_sql: Option<&'a str>,
    all: bool,
    tables: &'a [String],
    schema: Option<&'a str>,
    format: Option<&'a str>,
    glob: Option<&'a str>,
    record_shape: Option<&'a str>,
    limit: Option<u64>,
    selector: Option<serde_json::Value>,
    destination: Option<serde_json::Value>,
    database_id: Option<&'a str>,
    dest_schema: Option<&'a str>,
    dest_table: Option<&'a str>,
    write_mode: Option<&'a str>,
    schedule: Option<serde_json::Value>,
    every: Option<&'a str>,
    next: Option<&'a str>,
}

/// A datasource the caller named, as the two things a create needs to know
/// about it.
struct Resolved {
    id: String,
    /// Known for free when the datasource was resolved by name, since the
    /// listing carries it; looked up on demand otherwise.
    family: Option<String>,
}

/// A `ds_…` id for whatever the caller named.
///
/// A display name is resolved HERE, against the listing, and the id is what
/// goes on the wire: the API deliberately has no name lookup, because a label
/// that is not unique cannot decide which datasource a load reads from. That
/// is also why two matches is an error rather than the newest one — the CLI
/// knows the name is ambiguous and the user does not, so guessing would put a
/// load on a source they never chose.
fn resolve_datasource(client: &IngestClient, named: &str) -> Resolved {
    if named.starts_with("ds_") {
        return Resolved {
            id: named.to_string(),
            family: None,
        };
    }
    let resp = with_spinner("resolving the datasource…", || {
        client.list_datasources(&[])
    });
    let matches: Vec<&crate::client::ingest::Datasource> = resp
        .datasources
        .iter()
        .filter(|d| {
            d.display_name
                .as_deref()
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(named.trim()))
        })
        .collect();
    match matches.as_slice() {
        [one] => Resolved {
            id: one.datasource_id.clone(),
            family: one.family.clone(),
        },
        [] => fail(&format!(
            "no datasource named '{named}' — list them with 'hotdata datasource list', \
             or pass the ds_… id"
        )),
        several => {
            let listed: Vec<String> = several
                .iter()
                .map(|d| format!("  {}  {}", d.datasource_id, cell(d.family.as_deref())))
                .collect();
            fail(&format!(
                "'{named}' names {} datasources — display names are labels, not identity. \
                 Pass one of these ids:\n{}",
                several.len(),
                listed.join("\n")
            ))
        }
    }
}

fn create(client: &IngestClient, output: &str, plan: CreatePlan) {
    let req = build_create(plan).unwrap_or_else(|m| fail(&m));
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
/// returned) so the selector shorthands, the `--sql` desugaring, the
/// destination assembly, and the type/schedule rules — the parts a server-side
/// 422 would otherwise be the first to catch — are unit-testable.
fn build_create(plan: CreatePlan) -> Result<IngestCreate, String> {
    let wire_type = wire_type(plan.kind)?;
    let (selector, source_table) = build_selector(&plan)?;
    let destination = build_destination(
        plan.destination,
        plan.database_id,
        plan.dest_schema,
        plan.dest_table,
        plan.write_mode,
        source_table.as_deref(),
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

/// The selector, plus the table name the destination should inherit when the
/// caller did not name one.
///
/// Every branch here BUILDS the JSON `--selector` would have carried; the
/// shorthands are sugar over one document, never a second request shape. Which
/// keys go in follows the flags rather than the family: `tables` means the same
/// thing to SQL, Iceberg and DuckLake, and a key a family does not have comes
/// back as a 422 naming it.
///
/// `mode` is the exception, and it is why `--table` is the one shorthand that
/// costs a family lookup. SQL's selector is a union discriminated on `mode`, so
/// it must be told which member this is EVEN THOUGH the member declares
/// `tables` as its default — a default fills a field that is present, and a
/// discriminator has to be read before there is a member to take defaults from.
/// The other two families forbid unknown keys, so the same one word makes their
/// request invalid. Sending it always, or never, is wrong for someone.
fn build_selector(plan: &CreatePlan) -> Result<(serde_json::Value, Option<String>), String> {
    if let Some(s) = &plan.selector {
        if !s.is_object() {
            return Err("--selector must be a JSON object".into());
        }
        return Ok((s.clone(), None));
    }

    // --sql desugars into BOTH halves: a structured sql-family selector and a
    // default destination table. The service never sees the SQL.
    if let Some(sql) = plan.sql {
        let parsed = parse_select(sql)?;
        let table = parsed.table.clone();
        return Ok((sql_selector(&parsed), Some(table)));
    }

    if let Some(sql) = plan.raw_sql {
        if sql.trim().is_empty() {
            return Err("--raw-sql needs a statement".into());
        }
        let mut selector = serde_json::Map::new();
        selector.insert("mode".into(), "query".into());
        selector.insert("sql".into(), sql.trim().into());
        if let Some(n) = plan.limit {
            selector.insert("limit".into(), n.into());
        }
        // A query has no source table, so --table names where the result
        // lands rather than what to read.
        return match plan.tables {
            [] => Ok((serde_json::Value::Object(selector), None)),
            [one] => Ok((serde_json::Value::Object(selector), Some(one.clone()))),
            _ => Err(
                "--raw-sql lands one result set — name one table with --table (or \
                 --dest-table)"
                    .into(),
            ),
        };
    }

    if plan.all {
        // "Everything" is only a selector the API accepts where the family's
        // own selector can express it: a bucket root has a natural whole, and
        // a database or a catalog requires the list of what to read. Saying so
        // beats sending a selector that comes back 422 for a missing field.
        let Some(format) = plan.format else {
            return Err(
                "--all loads a whole bucket root and needs --format (csv, jsonl, parquet). \
                 For a database, catalog, or API source, name what to load with --table."
                    .into(),
            );
        };
        let mut selector = serde_json::Map::new();
        selector.insert("prefix".into(), "".into());
        selector.insert("glob".into(), plan.glob.unwrap_or("**").into());
        selector.insert("file_format".into(), format.into());
        add_shape_and_limit(&mut selector, plan);
        return Ok((serde_json::Value::Object(selector), None));
    }

    let mut selector = serde_json::Map::new();
    if !plan.tables.is_empty() {
        if plan.family == Some("sql") {
            selector.insert("mode".into(), "tables".into());
        }
        if let Some(s) = plan.schema {
            selector.insert("schema".into(), s.into());
        }
        selector.insert("tables".into(), plan.tables.to_vec().into());
    }
    if let Some(f) = plan.format {
        selector.insert("file_format".into(), f.into());
    }
    if let Some(g) = plan.glob {
        selector.insert("glob".into(), g.into());
    }
    add_shape_and_limit(&mut selector, plan);
    if selector.is_empty() {
        return Err(
            "nothing to read — pass --table <name> (SQL, Iceberg, DuckLake), --format \
             with an optional --glob (buckets), --sql, --raw-sql, --all, or the whole \
             --selector as JSON"
                .into(),
        );
    }
    // A single named table is the destination's name too, unless overridden.
    let source_table = match plan.tables {
        [one] => Some(one.clone()),
        _ => None,
    };
    Ok((serde_json::Value::Object(selector), source_table))
}

/// The two qualifiers that ride along with any shorthand shape.
fn add_shape_and_limit(
    selector: &mut serde_json::Map<String, serde_json::Value>,
    plan: &CreatePlan,
) {
    if let Some(s) = plan.record_shape {
        selector.insert("record_shape".into(), s.into());
    }
    if let Some(n) = plan.limit {
        selector.insert("limit".into(), n.into());
    }
}

/// CLI spelling → wire value. The CLI uses kebab-case like every other flag
/// value; the API's `type` field is snake_case.
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
/// flags.
///
/// `source_table` is the name the selector already implies — the single
/// `--table`, or the `--sql` FROM table — and it is the default so that loading
/// one table does not mean typing its name twice. `--dest-table` wins, which is
/// how a table lands under a different name.
fn build_destination(
    destination: Option<serde_json::Value>,
    database_id: Option<&str>,
    schema: Option<&str>,
    table: Option<&str>,
    write_mode: Option<&str>,
    source_table: Option<&str>,
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
             <db> (the table follows --table)"
                .into(),
        );
    };
    let Some(table) = table.or(source_table) else {
        return Err(
            "a destination table is required: --dest-table <name>, or a single --table \
             whose name it takes"
                .into(),
        );
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
    let raw = sql.trim().trim_end_matches(';').trim();
    if raw.contains(';') {
        return Err("--sql takes a single SELECT statement".into());
    }
    // Whitespace is collapsed to single spaces before anything is matched.
    // The keyword searches below look for literal " FROM " and "SELECT ", so
    // without this a newline or a tab between clauses reads as a missing
    // clause -- and SQL pasted out of a script or a heredoc is normally
    // indented across several lines. Reporting "needs a FROM clause" about a
    // query whose second line is FROM is the kind of error that sends someone
    // looking for a typo that is not there.
    //
    // A multi-space run inside a string literal is normalised too. That is
    // accepted: the predicate is passed through to the source engine, which
    // parses it, and no engine distinguishes `a = 'x  y'` on whitespace inside
    // a quoted literal differently from how it would after this -- the far
    // more common case is indentation, which this fixes.
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.as_str();
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
                 '{{\"mode\":\"query\",\"sql\":\"…\"}}'"
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
///
/// The shape is flat and `tables` is a list of bare table names, because that
/// is what the service's selector model accepts: `schema`, `columns`, `where`,
/// and `limit` sit beside `tables` and apply to all of them. The model forbids
/// unknown keys, so a per-table object inside `tables` is a 422 rather than a
/// tolerated variant — and the `--sql` grammar reads exactly one table, so one
/// set of qualifiers is never a loss of expression.
///
/// A qualifier the SELECT did not give is omitted, not sent empty: `columns:
/// []` would read as "project no columns", and the model has no meaning for a
/// null `where`.
fn sql_selector(p: &ParsedSelect) -> serde_json::Value {
    let mut selector = serde_json::Map::new();
    selector.insert("mode".into(), "tables".into());
    if let Some(s) = &p.schema {
        selector.insert("schema".into(), s.clone().into());
    }
    selector.insert("tables".into(), vec![p.table.clone()].into());
    if !p.columns.is_empty() {
        selector.insert("columns".into(), p.columns.clone().into());
    }
    if let Some(f) = &p.filter {
        selector.insert("where".into(), f.clone().into());
    }
    if let Some(n) = p.limit {
        selector.insert("limit".into(), n.into());
    }
    serde_json::Value::Object(selector)
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
                    cell(i.r#type.as_deref()),
                    state_cell(i.state.as_deref()),
                    destination_cell(i.destination.as_ref()),
                    schedule_cell(i.schedule.as_ref(), i.next_attempt_at.as_deref()),
                    selector_cell(i.selector.as_ref()),
                    date_cell(i.created_at.as_deref()),
                    cell(i.datasource_id.as_deref()),
                ]
            })
            .collect();
        // Most important first: a narrow terminal takes columns from the right.
        // What an ingest IS — its id, what it writes, whether it is running —
        // outranks where it reads from. The datasource id is a 30-character
        // token that this listing takes as a FILTER (`--datasource-id`) rather
        // than something a reader picks a row by, so it goes last: shown when
        // the width is there, and one `hotdata ingest show` away when it is not.
        crate::output::table::print(
            &[
                "INGEST ID",
                "TYPE",
                "STATE",
                "DESTINATION",
                "SCHEDULE",
                "READS",
                "CREATED",
                "DATASOURCE ID",
            ],
            &rows,
        );
    });
}

/// What one ingest reads, in a listing cell.
///
/// The selector is the half of an ingest that says what it is FOR, and two
/// ingests off one datasource into two tables are otherwise told apart only by
/// their ids. Pure so the shapes it summarises are pinned by tests rather than
/// by whichever family someone last looked at a listing for.
///
/// The families are read by the keys they use, not by a `family` field, so a
/// family this build has never heard of still gets a summary as long as it
/// names its subset with one of the same words. Anything unrecognised falls
/// back to compact JSON, truncated — a cell that is hard to read beats a cell
/// that says nothing about an ingest that is running.
fn selector_cell(selector: Option<&serde_json::Value>) -> String {
    let Some(s) = selector.filter(|s| s.is_object() && !s.as_object().unwrap().is_empty()) else {
        return "-".into();
    };
    let str_at = |key: &str| {
        s.get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|v| !v.trim().is_empty())
            .map(str::to_string)
    };
    let list_at = |key: &str| {
        s.get(key)
            .and_then(serde_json::Value::as_array)
            .filter(|a| !a.is_empty())
            .map(|a| {
                a.iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_string)
                            // A REST resource can be an object; its `name` is
                            // the part a reader recognises.
                            .or_else(|| {
                                v.get("name")
                                    .and_then(serde_json::Value::as_str)
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| v.to_string())
                    })
                    .collect::<Vec<_>>()
            })
    };

    // A source-native query is summarised by its text, because the text IS the
    // selection — every other field of that selector is a qualifier on it.
    if s.get("mode").and_then(|m| m.as_str()) == Some("query")
        && let Some(sql) = str_at("sql")
    {
        return truncated(&format!(
            "query: {}",
            sql.split_whitespace().collect::<Vec<_>>().join(" ")
        ));
    }
    for (key, prefix) in [("tables", ""), ("topics", "topics: "), ("resources", "")] {
        if let Some(names) = list_at(key) {
            let qualified = match (str_at("schema"), names.len()) {
                (Some(schema), 1) => format!("{schema}.{}", names[0]),
                _ => names.join(", "),
            };
            return truncated(&format!("{prefix}{qualified}"));
        }
    }
    if let Some(path) = str_at("table_path") {
        return truncated(&path);
    }
    // The bucket families select by position rather than by name: the prefix
    // and glob together are the path pattern that was matched.
    let prefix = s.get("prefix").and_then(|p| p.as_str()).unwrap_or("");
    if let Some(glob) = str_at("glob").or_else(|| str_at("file_format").map(|f| format!("*.{f}"))) {
        return truncated(&format!("{prefix}{glob}"));
    }
    truncated(&compact_json(s))
}

/// Keep a listing cell to one readable width. The full value is one
/// `hotdata ingest show` away, and `-o json` never passes through here.
fn truncated(s: &str) -> String {
    const WIDTH: usize = 40;
    if s.chars().count() <= WIDTH {
        return s.to_string();
    }
    format!("{}…", s.chars().take(WIDTH - 1).collect::<String>())
}

/// DETAIL for a run listing.
///
/// A failure detail is free text from the pipeline — hundreds of characters
/// over several lines — and a table renders every one of those lines as a row
/// of its own, so one failed run is what turns a listing into a screenful of
/// fragments. Collapsing the whitespace first is what makes the truncation
/// hold: cutting to 40 characters does nothing if the first newline arrives at
/// character 12. `hotdata run show <run-id>` and `-o json` carry all of it.
fn detail_cell(detail: Option<&str>) -> String {
    match detail.filter(|d| !d.trim().is_empty()) {
        Some(d) => truncated(&d.split_whitespace().collect::<Vec<_>>().join(" ")),
        None => "-".into(),
    }
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
    field("destination:", &destination_cell(ing.destination.as_ref()));
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
        // Resume is not a run trigger, and saying so here is cheaper
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

fn runs(
    workspace_id: &str,
    output: &str,
    ingest_id: &str,
    status: Option<String>,
    wait: bool,
    wait_timeout: u64,
) {
    let mut filters: Vec<(&str, String)> = Vec::new();
    if let Some(s) = status {
        filters.push(("status", s));
    }

    let client = IngestClient::new(workspace_id);
    let resp = if wait {
        wait_for_newest_run(&client, ingest_id, &filters, wait_timeout)
    } else {
        with_spinner("loading runs…", || client.list_runs(ingest_id, &filters))
    };

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
                    detail_cell(r.detail.as_deref()),
                    date_cell(r.started_at.as_deref().or(r.queued_at.as_deref())),
                    date_cell(r.finished_at.as_deref()),
                    r.attempt
                        .map(|a| a.to_string())
                        .unwrap_or_else(|| "-".into()),
                ]
            })
            .collect();
        // Most important first; a narrow terminal takes columns from the right.
        // DETAIL sits next to STATUS because it is why a run has the status it
        // has, and dropping it would leave a failed listing saying only that
        // something failed. ATTEMPT is last: it repeats what a listing of
        // several attempts already shows by having several rows.
        crate::output::table::print(
            &[
                "RUN ID", "STATUS", "DETAIL", "STARTED", "FINISHED", "ATTEMPT",
            ],
            &rows,
        );
    });
}

/// Re-read the run list until its newest entry reaches a terminal status.
///
/// A WATCH, not a trigger. The scheduler owns dispatch, so nothing here makes a
/// queued run start — `ingest schedule <id> --next now` is what moves the next
/// one forward. It settles on the FIRST run to finish because a recurring
/// ingest never stops producing them, and a wait with no end is a wait nobody
/// can put in a script.
fn wait_for_newest_run(
    client: &IngestClient,
    ingest_id: &str,
    filters: &[(&str, String)],
    timeout_secs: u64,
) -> crate::client::ingest::RunsResponse {
    let outcome = poll_until(
        "waiting for the newest run…",
        timeout_secs,
        || client.list_runs(ingest_id, filters),
        |resp| resp.runs.first().is_some_and(|r| is_terminal(&r.status)),
        |resp| match resp.runs.first() {
            Some(r) => {
                let (status, stage) = presented_run_status(&r.status, r.stage.as_deref());
                Some(stage.unwrap_or(status))
            }
            // The scheduler has not dispatched one yet, which is the state a
            // one-time ingest passes through in seconds and a scheduled one can
            // sit in for its whole interval.
            None => Some("no run dispatched yet".into()),
        },
    );
    match outcome {
        Ok(resp) => resp,
        Err(_) => wait_timed_out(&format!("hotdata ingest runs {ingest_id} --wait")),
    }
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
            "hotdata ingest create --source <name-or-id> --table <table> --database-id <db>"
        }
        "list-imports" => "hotdata ingest list",
        "status" => "hotdata run show <run-id>  (or: hotdata ingest runs <ingest-id>)",
        "raw-sql" => {
            "hotdata ingest create --source <name-or-id> --raw-sql \"SELECT …\" \
             --table <result-table> --database-id <db>"
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

pub fn removed(argv: &[String]) -> ! {
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

    /// CONTRACT TEST — the literal below is the payload the worker's
    /// `sql` selector model accepts, and the worker has a test pinning the
    /// same literal on its side of the wire. **The two must be changed
    /// together**, because neither suite can fail on its own when they
    /// disagree: the CLI once emitted a per-table object here and every test
    /// on both sides stayed green while `ingest create --sql` 422'd for every
    /// user.
    #[test]
    fn sql_selector_emits_exactly_what_the_worker_accepts() {
        let p = parse_select("SELECT id, status FROM public.orders WHERE x LIMIT 5").unwrap();
        assert_eq!(
            sql_selector(&p),
            serde_json::json!({
                "mode": "tables",
                "schema": "public",
                "tables": ["orders"],
                "columns": ["id", "status"],
                "where": "x",
                "limit": 5,
            })
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
                "schema": "public",
                "tables": ["orders"],
                "columns": ["id"],
                "where": "id > 5",
                "limit": 10,
            })
        );
        // The minimum form: an unqualified SELECT * carries no `schema`, no
        // `columns`, no `where`, no `limit` — an empty column list would mean
        // "project nothing".
        let star = sql_selector(&parse_select("SELECT * FROM orders").unwrap());
        assert_eq!(
            star,
            serde_json::json!({"mode": "tables", "tables": ["orders"]})
        );
    }

    // --- response rendering ---------------------------------------------------

    /// The listing and detail views must show a destination for the body the
    /// service actually returns, which carries it as one nested object.
    /// Rendering from top-level `destination_*` fields printed `-` for every
    /// real ingest while the mock fixtures — invented in that shape — passed.
    #[test]
    fn a_worker_shaped_ingest_response_renders_its_destination() {
        let resp: crate::client::ingest::IngestsResponse =
            serde_json::from_str(crate::client::ingest::WORKER_INGEST_LIST_BODY).unwrap();
        let ing = &resp.ingests[0];
        assert_eq!(
            destination_cell(ing.destination.as_ref()),
            "db_1.public.orders_raw"
        );
        // What it READS, which is the half that tells two ingests off one
        // datasource apart.
        assert_eq!(selector_cell(ing.selector.as_ref()), "public.orders");
    }

    #[test]
    fn the_listing_summarises_every_familys_selector() {
        let cell = |v: serde_json::Value| selector_cell(Some(&v));
        // The bucket families select by position: prefix and glob together are
        // the pattern that was matched.
        assert_eq!(
            cell(
                serde_json::json!({"prefix": "orders/", "glob": "**/*.parquet",
                                    "file_format": "parquet"})
            ),
            "orders/**/*.parquet"
        );
        // A prefix with no glob still says which files.
        assert_eq!(
            cell(serde_json::json!({"prefix": "orders/", "file_format": "jsonl"})),
            "orders/*.jsonl"
        );
        // Several tables read as the list; a single one takes its schema.
        assert_eq!(
            cell(serde_json::json!({"tables": ["orders", "customers"]})),
            "orders, customers"
        );
        assert_eq!(
            cell(serde_json::json!({"topics": ["events"]})),
            "topics: events"
        );
        assert_eq!(
            cell(serde_json::json!({"table_path": "delta/orders"})),
            "delta/orders"
        );
        // A REST resource may be an object; its name is what a reader knows it
        // by.
        assert_eq!(
            cell(serde_json::json!({"resources": [{"name": "teams", "endpoint": {}}]})),
            "teams"
        );
        // Nothing to summarise renders as a dash, never blank.
        assert_eq!(selector_cell(None), "-");
        assert_eq!(cell(serde_json::json!({})), "-");
        // A shape this build does not recognise still says something about an
        // ingest that is running.
        assert_eq!(
            cell(serde_json::json!({"streams": ["a"]})),
            r#"{"streams":["a"]}"#
        );
    }

    #[test]
    fn a_source_native_query_is_summarised_by_its_text() {
        let selector = serde_json::json!({
            "mode": "query",
            "sql": "SELECT customer_id, sum(amount)\n  FROM orders\n  GROUP BY 1",
        });
        let rendered = selector_cell(Some(&selector));
        // One line, and inside a column. The whole statement is one
        // `hotdata ingest show` away.
        assert!(
            rendered.starts_with("query: SELECT customer_id"),
            "{rendered}"
        );
        assert!(!rendered.contains('\n'), "{rendered}");
        assert!(rendered.chars().count() <= 40, "{rendered}");
    }

    #[test]
    fn a_run_detail_is_one_cell_not_a_screenful() {
        // The shape a failed load reports: a sentence, a blank line, then an
        // indented trace. Untreated, every one of those lines is a table row.
        let detail = "Pipeline orders load step failed\n\n  \
                      LoadClientJobRetry: could not set lock on file\n  \
                      at /var/lib/pipelines/orders/load/completed_jobs/x.parquet\n";
        let rendered = detail_cell(Some(detail));
        assert!(!rendered.contains('\n'), "{rendered}");
        assert!(rendered.chars().count() <= 40, "{rendered}");
        assert!(
            rendered.starts_with("Pipeline orders load step"),
            "{rendered}"
        );
        // Short details are untouched, and a run with nothing to say gets a
        // dash rather than a blank cell.
        assert_eq!(detail_cell(Some("6 rows loaded")), "6 rows loaded");
        assert_eq!(detail_cell(None), "-");
        assert_eq!(detail_cell(Some("   ")), "-");
    }

    // --- request construction -------------------------------------------------

    /// Named so a test that is not about source tables reads as not being
    /// about them.
    const NO_TABLES: &[String] = &[];

    fn plan<'a>(datasource_id: &'a str, kind: &'a str) -> CreatePlan<'a> {
        CreatePlan {
            datasource_id,
            family: None,
            kind,
            sql: None,
            raw_sql: None,
            all: false,
            tables: NO_TABLES,
            schema: None,
            format: None,
            glob: None,
            record_shape: None,
            limit: None,
            selector: None,
            destination: None,
            database_id: None,
            dest_schema: None,
            dest_table: None,
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
        assert_eq!(req.selector["tables"][0], "orders");
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
        assert!(build_create(p).unwrap_err().contains("--table"));

        // Selector but no destination.
        p = plan("ds_1", "one-time");
        p.selector = Some(serde_json::json!({"mode": "tables"}));
        assert!(build_create(p).unwrap_err().contains("--destination"));

        // A database with no table for it to land in, and no selector naming
        // one for it to inherit.
        p = plan("ds_1", "one-time");
        p.selector = Some(serde_json::json!({"mode": "tables"}));
        p.database_id = Some("db_1");
        assert!(build_create(p).unwrap_err().contains("--dest-table"));
    }

    #[test]
    fn schedule_rules_follow_the_ingest_type() {
        let orders = ["orders".to_string()];
        // scheduled/continuous need one.
        let mut p = plan("ds_1", "continuous");
        p.tables = &orders;
        p.database_id = Some("db_1");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("--every"), "{err}");

        // one-time must not have one.
        let mut p = plan("ds_1", "one-time");
        p.tables = &orders;
        p.database_id = Some("db_1");
        p.every = Some("5m");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("one-time ingest has no schedule"), "{err}");
    }

    // --- the selector shorthands ---------------------------------------------

    /// CONTRACT TEST — the literal below is what `--table`/`--schema` build for
    /// a SQL datasource, and it is what the service's sql selector model
    /// accepts. **Change it only with the service.**
    ///
    /// `mode` is present even though the member declares `tables` as its
    /// default: the selector is a UNION discriminated on `mode`, and a
    /// discriminator has to be read before there is a member whose defaults
    /// could fill it in. Omitting it is a 422 on every request, which is
    /// precisely the failure this test exists to hold still.
    #[test]
    fn table_and_schema_build_exactly_what_the_worker_accepts_for_sql() {
        let tables = ["orders".to_string(), "customers".to_string()];
        let mut p = plan("ds_pg", "one-time");
        p.family = Some("sql");
        p.tables = &tables;
        p.schema = Some("public");
        p.limit = Some(100);
        p.database_id = Some("db_1");
        p.dest_table = Some("orders_raw");
        let req = build_create(p).unwrap();
        assert_eq!(
            req.selector,
            serde_json::json!({
                "mode": "tables",
                "schema": "public",
                "tables": ["orders", "customers"],
                "limit": 100,
            })
        );
    }

    /// CONTRACT TEST — the same shorthand for the families whose selector is
    /// NOT a union. They forbid unknown keys, so the `mode` that SQL cannot do
    /// without is the one word that makes their request invalid.
    #[test]
    fn table_builds_exactly_what_the_worker_accepts_for_a_catalog_family() {
        let tables = ["orders".to_string(), "customers".to_string()];
        for family in ["iceberg", "ducklake"] {
            let mut p = plan("ds_cat", "one-time");
            p.family = Some(family);
            p.tables = &tables;
            p.limit = Some(100);
            p.database_id = Some("db_1");
            p.dest_table = Some("orders_raw");
            let req = build_create(p).unwrap();
            assert_eq!(
                req.selector,
                serde_json::json!({"tables": ["orders", "customers"], "limit": 100}),
                "{family}"
            );
        }
        // A family this build has never seen gets the shape without the SQL
        // discriminator, which is the one that fits a plain object model.
        let mut p = plan("ds_new", "one-time");
        p.family = Some("something-new");
        p.tables = &tables;
        p.database_id = Some("db_1");
        p.dest_table = Some("t");
        assert!(build_create(p).unwrap().selector.get("mode").is_none());
    }

    #[test]
    fn a_single_source_table_names_the_destination_table() {
        // Loading one table must not mean typing its name twice.
        let orders = ["orders".to_string()];
        let mut p = plan("ds_pg", "one-time");
        p.family = Some("sql");
        p.tables = &orders;
        p.database_id = Some("db_1");
        let req = build_create(p).unwrap();
        assert_eq!(
            req.selector,
            serde_json::json!({"mode": "tables", "tables": ["orders"]})
        );
        assert_eq!(
            req.destination,
            serde_json::json!({
                "database_id": "db_1",
                "schema": "public",
                "table": "orders",
                "write_mode": "replace",
            })
        );
        // Several tables name no single destination, so one must be given.
        let two = ["orders".to_string(), "customers".to_string()];
        let mut p = plan("ds_pg", "one-time");
        p.tables = &two;
        p.database_id = Some("db_1");
        assert!(build_create(p).unwrap_err().contains("--dest-table"));
    }

    /// CONTRACT TEST — the selector `--format`, `--glob` and `--record-shape`
    /// build, as the service's filesystem selector model accepts it.
    #[test]
    fn the_bucket_shorthands_build_exactly_what_the_worker_accepts() {
        let mut p = plan("ds_s3", "continuous");
        p.format = Some("jsonl");
        p.glob = Some("orders/**/*.jsonl");
        p.record_shape = Some("otel_traces");
        p.database_id = Some("db_1");
        p.dest_table = Some("traces");
        p.every = Some("5m");
        let req = build_create(p).unwrap();
        assert_eq!(
            req.selector,
            serde_json::json!({
                "file_format": "jsonl",
                "glob": "orders/**/*.jsonl",
                "record_shape": "otel_traces",
            })
        );
    }

    /// CONTRACT TEST — the `mode=query` selector `--raw-sql` builds. This is
    /// the capability that used to be its own endpoint: the statement runs
    /// verbatim in the source engine's dialect and only the result transfers.
    #[test]
    fn raw_sql_builds_exactly_the_query_selector_the_worker_accepts() {
        let result = ["order_totals".to_string()];
        let mut p = plan("ds_pg", "one-time");
        p.raw_sql = Some("SELECT customer_id, sum(amount) FROM orders GROUP BY 1");
        p.limit = Some(1000);
        p.tables = &result;
        p.database_id = Some("db_1");
        let req = build_create(p).unwrap();
        assert_eq!(
            req.selector,
            serde_json::json!({
                "mode": "query",
                "sql": "SELECT customer_id, sum(amount) FROM orders GROUP BY 1",
                "limit": 1000,
            })
        );
        // A query has no source table, so --table named where the result lands
        // — the same thing it named before the split.
        assert_eq!(req.destination["table"], "order_totals");
    }

    #[test]
    fn raw_sql_lands_one_table() {
        let two = ["a".to_string(), "b".to_string()];
        let mut p = plan("ds_pg", "one-time");
        p.raw_sql = Some("SELECT 1");
        p.tables = &two;
        p.database_id = Some("db_1");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("one result set"), "{err}");
    }

    /// CONTRACT TEST — the selector `--all` builds for a bucket source.
    #[test]
    fn all_builds_the_whole_root_selector_the_worker_accepts() {
        let mut p = plan("ds_s3", "one-time");
        p.all = true;
        p.format = Some("parquet");
        p.database_id = Some("db_1");
        p.dest_table = Some("events");
        let req = build_create(p).unwrap();
        assert_eq!(
            req.selector,
            serde_json::json!({"prefix": "", "glob": "**", "file_format": "parquet"})
        );
    }

    #[test]
    fn all_says_which_flag_names_what_to_load_where_it_cannot_be_expressed() {
        // A database or a catalog selector requires the list; "everything" is
        // not a request those families have. Saying so beats sending a body
        // that comes back 422 for a field the user never saw.
        let mut p = plan("ds_pg", "one-time");
        p.all = true;
        p.database_id = Some("db_1");
        p.dest_table = Some("t");
        let err = build_create(p).unwrap_err();
        assert!(err.contains("--format"), "{err}");
        assert!(err.contains("--table"), "{err}");
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
            // Not "--selector" any more: the capability has its own flag back,
            // and a pointer at the escape hatch would be the long way round.
            ("raw-sql", "--raw-sql"),
        ] {
            let msg = removal_message(verb).unwrap_or_else(|| panic!("{verb} unmapped"));
            assert!(msg.contains(needle), "{verb}: {msg}");
        }
        // A genuine typo is not claimed to be a removed verb.
        assert!(removal_message("crate").is_none());
    }
}
