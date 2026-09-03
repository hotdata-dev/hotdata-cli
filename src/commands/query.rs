use crate::client::sdk::{Api, ApiError};
#[cfg(test)]
use arrow::datatypes::FieldRef;
use arrow::error::ArrowError;
use arrow::json::writer::{EncoderOptions, NullableEncoder, make_encoder};
use serde::Serialize;
use serde_json::Value;
use std::sync::LazyLock;

/// Subcommands for `hotdata query`.
#[derive(clap::Subcommand)]
pub enum QueryCommands {
    /// Check the status of a running query and retrieve results.
    /// Exit codes: 0 = succeeded, 1 = failed, 2 = still running (poll again),
    /// 3 = succeeded but the result is an incomplete/truncated preview
    Status {
        /// Query run ID
        id: String,
    },
}

#[derive(Serialize)]
pub struct QueryResponse {
    /// ID of the query run that produced this result. Surfaced so a user can
    /// look up run-level metadata (e.g. bytes/rows scanned) with
    /// `hotdata queries <id>`. `None` on paths that fetch a persisted result by
    /// `result_id` alone and never learn the originating run.
    pub query_run_id: Option<String>,
    pub result_id: Option<String>,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    /// Rows actually carried here (`rows.len()`). For a complete result this is
    /// the whole result; for an incomplete preview (`truncated`) it's a bounded
    /// subset.
    pub row_count: u64,
    /// Grand total rows in the full result when the server reported it. `None`
    /// when unknown — e.g. a truncated result whose persistence never started.
    pub total_row_count: Option<u64>,
    /// True when `rows` is an *incomplete* subset the CLI could not complete:
    /// the server truncated the result and it was either never persisted (no
    /// `result_id`) or the follow-up fetch failed. A truncated result the CLI
    /// successfully follows to the full set is `false` — the rows held are the
    /// whole result. Drives the fail-closed exit in [`print_result`].
    pub truncated: bool,
    pub execution_time_ms: Option<u64>,
    /// A human-facing completeness notice (e.g. truncation). Printed to stderr by
    /// [`print_result`], never the stdout body — a JSON consumer reads the same
    /// fact from the typed `truncated` / `total_row_count` fields.
    #[serde(skip)]
    pub warning: Option<String>,
}

/// Exit code emitted when the CLI prints an incomplete preview (a truncated
/// result it could not complete). Distinct from the generic `1`/`2` so pipelines
/// can tell "partial data" apart from a hard failure and break rather than
/// silently ingest a subset.
pub const EXIT_INCOMPLETE_RESULT: i32 = 3;

/// Convert the SDK's inline `QueryResponse` (200 path) into the CLI's display
/// model. The async path decodes Arrow instead (see `fetch_arrow_result`).
///
/// `row_count` is derived from `rows.len()` — the rows actually carried in this
/// body — rather than the deprecated SDK `row_count` field. This path only ever
/// renders the rows it holds: a non-truncated response carries the whole result,
/// and the truncated-without-`result_id` fallback keeps just the preview (with a
/// warning). A truncated result that *can* be fetched never reaches here — it's
/// followed to the full set via Arrow in `resolve_inline`. So counting the held
/// rows can never overstate or understate what the user sees.
///
/// `truncated` and `total_row_count` are carried through verbatim so structured
/// output (`--output json`) exposes whether the rows are a partial preview and,
/// when known, the grand total. For a complete (non-truncated) result the server
/// reports `total_row_count == preview`; if it's absent we backfill it from the
/// held rows, which for a complete result is the whole truth.
fn query_response_from_sdk(resp: hotdata::models::QueryResponse) -> QueryResponse {
    let row_count = resp.rows.len() as u64;
    // A negative total is impossible per the API contract; treat one as absent
    // rather than clamping to 0, which would emit a contradictory
    // `preview_row_count > total_row_count`.
    let total_row_count = resp
        .total_row_count
        .flatten()
        .and_then(|t| u64::try_from(t).ok())
        .or(if resp.truncated {
            None
        } else {
            Some(row_count)
        });
    QueryResponse {
        query_run_id: (!resp.query_run_id.is_empty()).then_some(resp.query_run_id),
        result_id: resp.result_id.flatten(),
        columns: resp.columns,
        row_count,
        total_row_count,
        truncated: resp.truncated,
        rows: resp.rows,
        execution_time_ms: Some(resp.execution_time_ms.max(0) as u64),
        warning: resp.warning.flatten(),
    }
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(arr) => {
            let (formatted, count) = crate::output::table::truncate_array(arr);
            match count {
                Some(n) => format!("{formatted} ({n} items)"),
                None => formatted,
            }
        }
        Value::Object(_) => v.to_string(),
    }
}

/// Encoder options, matched to the service's inline path.
///
/// `explicit_nulls` keeps null fields inside struct values (`{"a":1,"b":null}`
/// rather than dropping `b`). It has to agree with the service or the same
/// query would describe a struct differently depending on which side rendered
/// it — the divergence this module exists to remove.
///
/// Caveat worth knowing: this is the same *encoder* as the service's, not the
/// same *build* of it. The service is on a later arrow than the SDK this links
/// against, so the two agree because they are observed to, not because a
/// version pins them together. Aligning them needs an SDK release first.
static ENCODER_OPTIONS: LazyLock<EncoderOptions> =
    LazyLock::new(|| EncoderOptions::default().with_explicit_nulls(true));

/// Encode one already-prepared cell to a `serde_json::Value`.
///
/// The service encodes with the same arrow-json encoder but writes its bytes
/// straight to the response and never builds a `Value`. This has to, because
/// it also draws tables and CSV, which need the cells individually. That extra
/// step is the only asymmetry left between the two, and it cannot change the
/// rendering — the text being parsed here is exactly what the service emits.
///
/// `buf` is reused across cells so a wide result does not allocate per value.
fn encode_cell(
    enc: &mut NullableEncoder<'_>,
    row: usize,
    buf: &mut Vec<u8>,
) -> Result<Value, ArrowError> {
    // A null the service really did send. Distinct from a value we could not
    // render, which is an error below and never a null.
    if enc.is_null(row) {
        return Ok(Value::Null);
    }
    buf.clear();
    enc.encode(row, buf);

    serde_json::from_slice(buf).map_err(|e| {
        ArrowError::JsonError(format!(
            "arrow-json produced text that is not valid JSON ({e}): {}",
            String::from_utf8_lossy(buf)
        ))
    })
}

/// Render one cell of an Arrow array, building an encoder for it.
///
/// The batch path builds encoders once per column; this is for a single cell,
/// and shares [`encode_cell`] with it so the two cannot diverge.
#[cfg(test)]
fn arrow_cell(
    col: &dyn arrow::array::Array,
    field: &FieldRef,
    row: usize,
) -> Result<Value, ArrowError> {
    let mut enc = make_encoder(field, col, &ENCODER_OPTIONS)?;
    let mut buf = Vec::new();
    encode_cell(&mut enc, row, &mut buf)
}

/// Describe a rendering failure, naming the column when one is known.
///
/// A value that cannot be rendered is a client-side failure, not absent data.
/// Reporting it as `null` would invent an absence the service never sent, and
/// nothing downstream could tell the two apart — so this becomes an error.
fn render_failure(
    schema: &arrow::datatypes::Schema,
    col: Option<usize>,
    err: &ArrowError,
) -> ApiError {
    match col.and_then(|c| schema.fields().get(c)) {
        Some(f) => ApiError::Transport(format!(
            "could not render column '{}' of type {} from the fetched result: {err}. \
             The value is present in the result — refusing to report it as null.",
            f.name(),
            f.data_type()
        )),
        None => ApiError::Transport(format!(
            "could not build a renderer for the fetched result: {err}"
        )),
    }
}

/// Convert an SDK-decoded [`hotdata::ArrowResult`] into a `QueryResponse`
/// suitable for display.
fn arrow_result_to_query_response(
    result: hotdata::ArrowResult,
    result_id: String,
) -> Result<QueryResponse, ApiError> {
    let columns: Vec<String> = result
        .schema
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let mut rows: Vec<Vec<Value>> = Vec::new();

    let mut buf: Vec<u8> = Vec::new();
    for batch in &result.batches {
        let schema = batch.schema();
        // One encoder per column per batch, as the service does — building one
        // per cell would repeat the type dispatch for every value.
        let mut encoders: Vec<NullableEncoder<'_>> = batch
            .columns()
            .iter()
            .zip(schema.fields())
            .map(|(col, field)| make_encoder(field, col.as_ref(), &ENCODER_OPTIONS))
            .collect::<Result<_, _>>()
            .map_err(|e| render_failure(&schema, None, &e))?;

        for row in 0..batch.num_rows() {
            let mut cells: Vec<Value> = Vec::with_capacity(encoders.len());
            for (c, enc) in encoders.iter_mut().enumerate() {
                cells.push(
                    encode_cell(enc, row, &mut buf)
                        .map_err(|e| render_failure(&schema, Some(c), &e))?,
                );
            }
            rows.push(cells);
        }
    }

    let row_count = rows.len() as u64;
    // The fetched Arrow result is the full persisted result, so it's complete:
    // `truncated` is false and the total is the authoritative `X-Total-Row-Count`
    // the SDK parsed (falling back to the rows we hold).
    let total_row_count = result
        .total_row_count
        .and_then(|t| u64::try_from(t).ok())
        .or(Some(row_count));
    Ok(QueryResponse {
        // The Arrow fetch is keyed by result_id and carries no run id; the
        // async/poll callers that know it stamp it after this returns.
        query_run_id: None,
        result_id: Some(result_id),
        columns,
        rows,
        row_count,
        total_row_count,
        truncated: false,
        execution_time_ms: None,
        warning: None,
    })
}

/// Fetch `/results/{result_id}` as Arrow and return a `QueryResponse`, returning
/// the error instead of exiting.
///
/// Both transport and decode are owned by the SDK's `get_result_arrow` (via the
/// [`Api::get_result_arrow`] seam), so the CLI shares one `arrow` major version
/// with the SDK.
///
/// The fallible form lets callers that hold a fallback (e.g. an inline preview)
/// degrade gracefully rather than terminate the process; [`fetch_arrow_result`]
/// is the exiting wrapper for callers with nothing else to show.
pub(crate) fn try_fetch_arrow_result(
    api: &Api,
    result_id: &str,
) -> Result<QueryResponse, ApiError> {
    let result = api.get_result_arrow(result_id)?;
    arrow_result_to_query_response(result, result_id.to_owned())
}

/// Fetch `/results/{result_id}` as Arrow, exiting the process on failure.
pub(crate) fn fetch_arrow_result(api: &Api, result_id: &str) -> QueryResponse {
    try_fetch_arrow_result(api, result_id).unwrap_or_else(|e| e.exit())
}

/// Convert a query run's wire `execution_time_ms` (a double-option: outer = field
/// presence, inner = JSON null) into the display value. A reported time clamps
/// negatives to 0 (mirroring the inline path); an absent/null time stays `None`
/// so the display shows an em dash rather than a fabricated 0.
fn run_execution_time_ms(raw: Option<Option<i64>>) -> Option<u64> {
    raw.flatten().map(|ms| ms.max(0) as u64)
}

/// Fetch a succeeded run's persisted Arrow result and stamp it with the run's own
/// `execution_time_ms`. The Arrow result body carries no timing
/// (`arrow_result_to_query_response` hardcodes `None`), so the async/poll display
/// paths would otherwise report `execution_time_ms: null` for every query slow
/// enough to fall back to async — exactly the queries you most want timed (#183).
fn fetch_arrow_result_with_timing(
    api: &Api,
    result_id: &str,
    execution_time_ms: Option<Option<i64>>,
) -> QueryResponse {
    let mut result = fetch_arrow_result(api, result_id);
    result.execution_time_ms = run_execution_time_ms(execution_time_ms);
    result
}

/// Resolve an inline (HTTP 200) query response for display.
///
/// A non-truncated response carries the whole result in `rows`, so it's shown
/// as-is. A truncated one (#640) carries only a bounded preview — the full set
/// is persisted under `result_id` — so follow it to the full result via Arrow,
/// the same path the async (202) branch uses. Truncation rides on result *size*
/// while `async_after_ms` gates on *time*, so a fast-completing but large query
/// returns a truncated inline 200; without this follow the CLI would silently
/// print only the preview rows.
///
/// If a truncated response has no `result_id` (persistence could not be
/// initiated — see the SDK's `warning` field), or the follow-up Arrow fetch
/// fails, the full result is unreachable. Rather than exiting and discarding the
/// preview the inline body already carries, fall back to that preview, mark it
/// incomplete (`truncated`), and surface a warning. `print_result` then fails
/// closed (non-zero exit) so the partial data can't be silently consumed.
fn resolve_inline(api: &Api, resp: hotdata::models::QueryResponse) -> QueryResponse {
    if !resp.truncated {
        return query_response_from_sdk(resp);
    }
    match resp.result_id.clone().flatten() {
        Some(result_id) => match try_fetch_arrow_result(api, &result_id) {
            // The Arrow fetch returns only schema + rows; carry the query-level
            // warning, execution time, and run id the inline response reported,
            // which `arrow_result_to_query_response` otherwise hardcodes to None.
            Ok(mut full) => {
                full.warning = resp.warning.flatten();
                full.execution_time_ms = Some(resp.execution_time_ms.max(0) as u64);
                full.query_run_id = (!resp.query_run_id.is_empty()).then_some(resp.query_run_id);
                full
            }
            // The full result is persisted but the follow-up fetch failed (e.g.
            // transport error, persistence still in progress). Degrade to the
            // preview instead of hard-exiting and losing the rows in hand.
            Err(e) => incomplete_preview(
                resp,
                &format!("could not fetch full result ({})", e.message()),
            ),
        },
        None => incomplete_preview(resp, "full result unavailable (persistence not initiated)"),
    }
}

/// Keep a truncated inline response's preview rows, mark it incomplete, and fold
/// `note` into the warning (preserving any SDK-provided warning that explains
/// *why* the full result is unreachable).
fn incomplete_preview(resp: hotdata::models::QueryResponse, note: &str) -> QueryResponse {
    let mut preview = query_response_from_sdk(resp);
    let note = format!("result truncated to a preview; {note}");
    preview.warning = Some(match preview.warning {
        Some(w) => format!("{w}; {note}"),
        None => note,
    });
    preview
}

/// When a query fails because it has no database context or references a
/// catalog that isn't in scope, return a one-line hint pointing at
/// `databases set` / `databases attach`. Pure string inspection of the server
/// error so it's unit-testable and adds no network round-trip on success.
///
/// A `query` runs inside exactly one instant database; that context exposes the
/// database's own catalog plus any *attached* connection catalogs. The two
/// failure modes a user hits when they don't know this are "a database is
/// required" (no context set) and "table '<catalog>.<schema>.<table>' not
/// found" (the catalog isn't attached) — both resolved by attaching.
fn cross_source_hint(error_msg: &str) -> Option<String> {
    let lower = error_msg.to_lowercase();
    if lower.contains("a database is required") {
        return Some(
            "Tip: a query runs inside one instant database. Set one with `hotdata databases \
             use <id>`, then attach any catalog whose tables you need: `hotdata databases \
             attach <catalog>`. See available catalogs and tables with `hotdata databases \
             tables list`."
                .to_string(),
        );
    }
    // "table 'catalog.schema.table' not found" — surface the catalog so the user
    // can attach it if it's a catalog simply outside this database's scope.
    if lower.contains("not found")
        && let Some(quoted) = error_msg.split('\'').nth(1)
        && quoted.contains('.')
        && let Some(catalog) = quoted.split('.').next().filter(|c| !c.is_empty())
    {
        return Some(format!(
            "Tip: '{catalog}' isn't in the current database's scope. If it's a catalog, \
             attach it to query across catalogs: `hotdata databases attach {catalog}`."
        ));
    }
    None
}

/// Print the API error, append the cross-source hint when one applies, then
/// exit non-zero. Used on both query failure paths (submit error and async
/// `failed`) so the hint shows after the error regardless of which one fires.
fn fail_query(err: &ApiError, error_msg: &str) -> ! {
    err.print();
    if let Some(tip) = cross_source_hint(error_msg) {
        use crossterm::style::Stylize;
        eprintln!("{}", tip.dark_grey());
    }
    std::process::exit(1);
}

/// Print a failed query run's `query failed: <err>` line, append the cross-source
/// hint when applicable, then exit non-zero. Shared by both terminal-failure
/// sites — `execute`'s poll loop and the `query status` (`poll`) command — so the
/// hint surfaces identically whether the failure is seen inline or on a later
/// poll.
fn fail_run(error_msg: &str) -> ! {
    use crossterm::style::Stylize;
    eprintln!("{}", format!("query failed: {error_msg}").red());
    if let Some(tip) = cross_source_hint(error_msg) {
        eprintln!("{}", tip.dark_grey());
    }
    std::process::exit(1);
}

pub fn execute(sql: &str, workspace_id: &str, database: Option<&str>, format: &str, dialect: &str) {
    // Scope to the explicit --database flag, else the active database resolved
    // at construction (HOTDATA_DATABASE / current database). The scoped `Api`
    // carries the database into submit_query's `X-Database-Id` header and into
    // the database-scoped follow-up fetches (query-run poll, Arrow result).
    let api = Api::new(Some(workspace_id)).scoped_to_database_opt(database);
    let database = api.database_id();

    let mut request = hotdata::models::QueryRequest::new(sql.to_string());
    request.r#async = Some(true);
    request.async_after_ms = Some(Some(1000));
    // `hotsql` is the server default and planned as-is; only send `dialect` for a
    // non-default dialect so ordinary queries keep their existing request shape.
    if dialect != "hotsql" {
        request.dialect = Some(Some(dialect.to_string()));
    }

    let outcome = crate::client::sdk::block_with_wakeup(
        &api,
        "running query...",
        api.client().submit_query(request, database),
    )
    .unwrap_or_else(|e| {
        let msg = e.message();
        fail_query(&e, &msg)
    });

    let async_resp = match outcome {
        // Completed within async_after_ms — inline results. A large result can
        // come back truncated to a preview even on this fast path, so follow it
        // to the full set (resolve_inline) rather than printing the preview.
        hotdata::QueryOutcome::Inline(resp) => {
            print_result(&resolve_inline(&api, resp), format);
            return;
        }
        // Still running — poll the query run, then fetch the result as Arrow.
        hotdata::QueryOutcome::Submitted(async_resp) => async_resp,
        // QueryOutcome is #[non_exhaustive]; guard against future variants.
        _ => {
            eprintln!("unexpected query response from server");
            std::process::exit(1);
        }
    };

    let run_id = &async_resp.query_run_id;
    let spinner = crate::util::spinner("waiting for query...");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);

    loop {
        // Drive the poll loop ourselves to preserve the 5-minute deadline and
        // 500ms cadence (NOT the SDK's PollConfig defaults).
        let run = crate::client::sdk::block(
            api.client()
                .query_runs()
                .get(run_id, api.require_database()),
        )
        .unwrap_or_else(|e| e.exit());
        match run.status.as_str() {
            "succeeded" => {
                spinner.finish_and_clear();
                let execution_time_ms = run.execution_time_ms;
                match run.result_id.flatten() {
                    Some(ref result_id) => {
                        let mut result =
                            fetch_arrow_result_with_timing(&api, result_id, execution_time_ms);
                        result.query_run_id = Some(run_id.clone());
                        print_result(&result, format);
                    }
                    None => {
                        use crossterm::style::Stylize;
                        println!("{}", "Query succeeded but no result available.".yellow());
                    }
                }
                return;
            }
            "failed" => {
                spinner.finish_and_clear();
                let err = run
                    .error_message
                    .flatten()
                    .unwrap_or_else(|| "unknown error".to_string());
                fail_run(&err);
            }
            "running" | "queued" | "pending" => {}
            status => {
                spinner.finish_and_clear();
                use crossterm::style::Stylize;
                eprintln!("{}", format!("query status: {status}").yellow());
                eprintln!(
                    "{}",
                    format!("Check status with: hotdata query status {run_id}").dark_grey()
                );
                std::process::exit(2);
            }
        }
        if std::time::Instant::now() > deadline {
            spinner.finish_and_clear();
            use crossterm::style::Stylize;
            eprintln!("{}", "query timed out after 5 minutes".red());
            eprintln!(
                "{}",
                format!("Check status with: hotdata query status {run_id}").dark_grey()
            );
            std::process::exit(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

/// Poll a query run by ID. If succeeded and has a result_id, fetch and display the result.
pub fn poll(query_run_id: &str, workspace_id: &str, database: Option<&str>, format: &str) {
    let api = Api::new(Some(workspace_id)).scoped_to_database_opt(database);

    let run = crate::client::sdk::block(
        api.client()
            .query_runs()
            .get(query_run_id, api.require_database()),
    )
    .unwrap_or_else(|e| e.exit());

    match run.status.as_str() {
        "succeeded" => {
            let execution_time_ms = run.execution_time_ms;
            match run.result_id.flatten() {
                Some(ref result_id) => {
                    let mut result =
                        fetch_arrow_result_with_timing(&api, result_id, execution_time_ms);
                    result.query_run_id = Some(run.id.clone());
                    print_result(&result, format);
                }
                None => {
                    use crossterm::style::Stylize;
                    println!("{}", "Query succeeded but no result available.".yellow());
                }
            }
        }
        "failed" => {
            let err = run
                .error_message
                .flatten()
                .unwrap_or_else(|| "unknown error".to_string());
            fail_run(&err);
        }
        status => {
            use crossterm::style::Stylize;
            eprintln!("{}", format!("query status: {status}").yellow());
            eprintln!("query_run_id: {}", run.id);
            eprintln!(
                "{}",
                format!("Poll again with: hotdata query status {}", run.id).dark_grey()
            );
            std::process::exit(2);
        }
    }
}

/// Process exit code after rendering a result: [`EXIT_INCOMPLETE_RESULT`] when
/// the rows are an incomplete preview (fail closed so pipelines break), else `0`.
fn result_exit_code(result: &QueryResponse) -> i32 {
    if result.truncated {
        EXIT_INCOMPLETE_RESULT
    } else {
        0
    }
}

/// The unstyled summary line printed under a `table` result.
///
/// A complete result reads `N rows (time) [result-id]`. An incomplete preview is
/// loud — `N of TOTAL rows — INCOMPLETE PREVIEW (...)` — with `?` standing in for
/// a total the server didn't report. The caller colours it (red vs grey).
fn table_footer(result: &QueryResponse) -> String {
    let run_part = result
        .query_run_id
        .as_deref()
        .map(|id| format!(" [run: {id}]"))
        .unwrap_or_default();
    let id_part = result
        .result_id
        .as_deref()
        .map(|id| format!(" [result-id: {id}]"))
        .unwrap_or_default();
    let time_part = match result.execution_time_ms {
        Some(ms) => format!("{ms} ms"),
        None => "\u{2014}".to_string(), // em dash
    };
    if result.truncated {
        let total = result
            .total_row_count
            .map(|t| t.to_string())
            .unwrap_or_else(|| "?".to_string());
        format!(
            "{} of {} rows — INCOMPLETE PREVIEW ({}){}{}",
            result.row_count, total, time_part, run_part, id_part
        )
    } else {
        format!(
            "{} row{} ({}){}{}",
            result.row_count,
            if result.row_count == 1 { "" } else { "s" },
            time_part,
            run_part,
            id_part
        )
    }
}

pub fn print_result(result: &QueryResponse, format: &str) {
    if let Some(ref warning) = result.warning {
        eprintln!("warning: {warning}");
    }

    match format {
        "json" => {
            // Serialize the display struct directly; `warning` is `#[serde(skip)]`
            // (stderr-only), the rest is the JSON body.
            println!("{}", serde_json::to_string_pretty(result).unwrap());
        }
        "csv" => {
            println!("{}", result.columns.join(","));
            for row in &result.rows {
                let cells: Vec<String> = row
                    .iter()
                    .map(|v| {
                        let s = value_to_string(v);
                        if s.contains(',') || s.contains('"') || s.contains('\n') {
                            format!("\"{}\"", s.replace('"', "\"\""))
                        } else {
                            s
                        }
                    })
                    .collect();
                println!("{}", cells.join(","));
            }
        }
        "table" => {
            crate::output::table::print_json(&result.columns, &result.rows);
            use crossterm::style::Stylize;
            let footer = table_footer(result);
            // Loud (red) when the preview is incomplete so it can't be mistaken
            // for a complete result; quiet (grey) otherwise.
            if result.truncated {
                eprintln!("\n{}", footer.red());
            } else {
                eprintln!("\n{}", footer.dark_grey());
            }
        }
        _ => unreachable!(),
    }

    // Fail closed: an incomplete preview was just printed (with a stderr
    // warning). Exit non-zero so a pipeline consuming the output breaks rather
    // than silently ingesting a subset of the result as if it were complete.
    let code = result_exit_code(result);
    if code != 0 {
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::sdk::Api;
    use std::sync::Arc;

    /// A truncated inline 200: one preview row standing in for a larger result.
    /// `result_id` uses the wire double-option (`Some(None)` = field present but
    /// null, i.e. persistence not initiated).
    fn truncated_preview(result_id: Option<&str>) -> hotdata::models::QueryResponse {
        let mut resp = hotdata::models::QueryResponse::new(
            vec!["id".to_string()],           // columns
            5,                                // execution_time_ms
            vec![false],                      // nullable
            1,                                // preview_row_count
            "qrun_1".to_string(),             // query_run_id
            1,                                // row_count (deprecated, == preview)
            vec![vec![serde_json::json!(1)]], // rows (preview only)
            true,                             // truncated
        );
        resp.result_id = Some(result_id.map(|s| s.to_string()));
        resp
    }

    /// A field describing `col`, for the single-cell helper.
    fn field_for(col: &dyn arrow::array::Array) -> FieldRef {
        use arrow::datatypes::Field;
        Arc::new(Field::new("v", col.data_type().clone(), true))
    }

    /// Rendering a cell must produce the same JSON the service produces for the
    /// inline path — a list as a JSON array, a struct as a JSON object, not
    /// arrow's human-readable debug text.
    ///
    /// This is the invariant that failed before: the service encodes with
    /// arrow-json while this rendered with the display formatter, so the same
    /// query returned `[1,2,3]` under one second and the *string* `"[1, 2, 3]"`
    /// over it. Anything doing `.v[0]` on the output broke on a slow day.
    #[test]
    fn nested_types_render_as_json_not_display_text() {
        use arrow::array::{ArrayRef, Int64Builder, ListBuilder, StructArray};
        use arrow::datatypes::{DataType, Field};

        let mut lb = ListBuilder::new(Int64Builder::new());
        lb.values().append_value(1);
        lb.values().append_value(2);
        lb.append(true);
        let list: ArrayRef = Arc::new(lb.finish());
        let got = arrow_cell(list.as_ref(), &field_for(list.as_ref()), 0).expect("list renders");
        assert_eq!(
            got,
            serde_json::json!([1, 2]),
            "list must be a JSON array, got {got:?}"
        );

        let st: ArrayRef = Arc::new(StructArray::from(vec![
            (
                Arc::new(Field::new("a", DataType::Int64, true)),
                Arc::new(arrow::array::Int64Array::from(vec![1])) as ArrayRef,
            ),
            (
                Arc::new(Field::new("b", DataType::Utf8, true)),
                Arc::new(arrow::array::StringArray::from(vec!["x"])) as ArrayRef,
            ),
        ]));
        let got = arrow_cell(st.as_ref(), &field_for(st.as_ref()), 0).expect("struct renders");
        assert_eq!(
            got,
            serde_json::json!({"a": 1, "b": "x"}),
            "struct must be a JSON object, got {got:?}"
        );
    }

    /// The production path — `arrow_result_to_query_response` — rather than the
    /// single-cell helper the other tests use.
    ///
    /// It owns work the helper does not: encoders built once per column per
    /// batch, row assembly, and the column naming carried by a failure. Those
    /// were previously exercised only by hand against a live service, which CI
    /// does not do, so a break in them would have shipped.
    #[test]
    fn the_batch_path_renders_a_whole_result() {
        use arrow::array::TimestampMicrosecondArray;
        use arrow::array::{ArrayRef, Int64Array, Int64Builder, ListBuilder, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};

        // Two rows, and a nested column so shape is actually asserted.
        let mut lb = ListBuilder::new(Int64Builder::new());
        lb.values().append_value(7);
        lb.values().append_value(8);
        lb.append(true);
        lb.values().append_value(9);
        lb.append(true);
        let list: ArrayRef = Arc::new(lb.finish());

        let micros = 1_767_268_800_000_000i64;
        let ts: ArrayRef = Arc::new(
            TimestampMicrosecondArray::from(vec![micros, micros]).with_timezone("Asia/Kolkata"),
        );
        let ints: ArrayRef = Arc::new(Int64Array::from(vec![1, 2]));

        let schema = Arc::new(Schema::new(vec![
            Field::new("n", DataType::Int64, false),
            Field::new("l", list.data_type().clone(), true),
            Field::new("t", ts.data_type().clone(), true),
        ]));
        let batch = RecordBatch::try_new(schema.clone(), vec![ints, list, ts]).expect("batch");

        let result = hotdata::ArrowResult {
            batches: vec![batch],
            schema,
            total_row_count: Some(2),
            next_link: None,
        };
        let resp = arrow_result_to_query_response(result, "rslt_test".to_string())
            .expect("the batch path renders");

        assert_eq!(resp.columns, vec!["n", "l", "t"]);
        assert_eq!(resp.row_count, 2);
        assert_eq!(resp.total_row_count, Some(2));
        assert_eq!(resp.result_id.as_deref(), Some("rslt_test"));
        assert_eq!(resp.rows.len(), 2);

        // The nested column must be a JSON array, not display text — the
        // invariant that failed before, asserted through the real path.
        assert_eq!(resp.rows[0][0], serde_json::json!(1));
        assert_eq!(resp.rows[0][1], serde_json::json!([7, 8]));
        assert_eq!(resp.rows[1][1], serde_json::json!([9]));

        // And the named zone resolves rather than nulling.
        let t = resp.rows[0][2].as_str().expect("timestamp is a string");
        assert!(t.contains("+05:30"), "expected a resolved offset, got {t}");
    }

    /// A timestamp carrying a *named* IANA zone must render, not come back as
    /// null. Rendering it needs a timezone database compiled in; without one
    /// Arrow's formatter errors, and this cell used to swallow that error and
    /// report the value as null — silently, and only for named zones, so a
    /// fixed-offset or zone-less timestamp in the same row looked fine.
    #[test]
    fn named_timezone_timestamps_render_instead_of_nulling() {
        use arrow::array::TimestampMicrosecondArray;

        // 2026-01-01T12:00:00Z
        let micros = 1_767_268_800_000_000i64;

        for zone in ["UTC", "America/New_York", "Europe/London"] {
            let col = TimestampMicrosecondArray::from(vec![micros]).with_timezone(zone);
            let cell = arrow_cell(&col, &field_for(&col), 0)
                .unwrap_or_else(|e| panic!("zone {zone} failed: {e}"));
            assert!(
                cell.is_string(),
                "zone {zone} rendered as {cell:?}, expected a formatted string"
            );
        }

        // The two forms that worked even without a timezone database, kept here
        // so a regression narrows to the named-zone case rather than all timestamps.
        let offset = TimestampMicrosecondArray::from(vec![micros]).with_timezone("+00:00");
        assert!(
            arrow_cell(&offset, &field_for(&offset), 0)
                .unwrap()
                .is_string()
        );
        let naive = TimestampMicrosecondArray::from(vec![micros]);
        assert!(
            arrow_cell(&naive, &field_for(&naive), 0)
                .unwrap()
                .is_string()
        );

        // A genuine null is still a null — that one the service really did send.
        let with_null = TimestampMicrosecondArray::from(vec![None::<i64>]).with_timezone("UTC");
        assert_eq!(
            arrow_cell(&with_null, &field_for(&with_null), 0).unwrap(),
            Value::Null
        );
    }

    /// No column type we can be handed may turn a present value into `null`.
    ///
    /// The timezone case above is one instance of a wider hazard: every type
    /// without an explicit arm falls through to Arrow's formatter, and a
    /// formatter that cannot handle the type used to be reported as a null
    /// cell. This walks the temporal and decimal types a query can return and
    /// asserts each renders, so a future dependency or feature change that
    /// breaks one of them fails here instead of silently blanking a column.
    #[test]
    fn no_column_type_silently_renders_as_null() {
        use arrow::array::{
            Date32Array, Date64Array, Decimal128Array, DurationMicrosecondArray, Float32Array,
            Float64Array, Time32SecondArray, Time64MicrosecondArray, TimestampMicrosecondArray,
            TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray,
        };

        let micros = 1_767_268_800_000_000i64;
        let cases: Vec<(&str, arrow::array::ArrayRef)> = vec![
            ("Date32", Arc::new(Date32Array::from(vec![20454]))),
            (
                "Date64",
                Arc::new(Date64Array::from(vec![1_767_268_800_000])),
            ),
            ("Time32(s)", Arc::new(Time32SecondArray::from(vec![43200]))),
            (
                "Time64(µs)",
                Arc::new(Time64MicrosecondArray::from(vec![43_200_000_000])),
            ),
            (
                "Duration(µs)",
                Arc::new(DurationMicrosecondArray::from(vec![1_000_000])),
            ),
            (
                "Decimal128",
                Arc::new(
                    Decimal128Array::from(vec![123_456i128])
                        .with_precision_and_scale(10, 3)
                        .expect("valid decimal"),
                ),
            ),
            (
                "Timestamp(s, naive)",
                Arc::new(TimestampSecondArray::from(vec![1_767_268_800])),
            ),
            (
                "Timestamp(ms, naive)",
                Arc::new(TimestampMillisecondArray::from(vec![1_767_268_800_000])),
            ),
            (
                "Timestamp(µs, naive)",
                Arc::new(TimestampMicrosecondArray::from(vec![micros])),
            ),
            (
                "Timestamp(ns, naive)",
                Arc::new(TimestampNanosecondArray::from(vec![micros * 1_000])),
            ),
            (
                "Timestamp(s, UTC)",
                Arc::new(TimestampSecondArray::from(vec![1_767_268_800]).with_timezone("UTC")),
            ),
            (
                "Timestamp(ms, UTC)",
                Arc::new(
                    TimestampMillisecondArray::from(vec![1_767_268_800_000]).with_timezone("UTC"),
                ),
            ),
            (
                "Timestamp(µs, UTC)",
                Arc::new(TimestampMicrosecondArray::from(vec![micros]).with_timezone("UTC")),
            ),
            (
                "Timestamp(ns, UTC)",
                Arc::new(TimestampNanosecondArray::from(vec![micros * 1_000]).with_timezone("UTC")),
            ),
            (
                "Timestamp(µs, +05:30)",
                Arc::new(TimestampMicrosecondArray::from(vec![micros]).with_timezone("+05:30")),
            ),
            (
                "Timestamp(µs, Asia/Kolkata)",
                Arc::new(
                    TimestampMicrosecondArray::from(vec![micros]).with_timezone("Asia/Kolkata"),
                ),
            ),
            // Floats belong in this sweep because arrow-json emits an unquoted
            // token for them: were it ever to emit `NaN` or `inf` — neither of
            // which is valid JSON — the parse would fail and one bad cell would
            // abort a whole result. It emits `null` today, matching the service.
            ("Float64 finite", Arc::new(Float64Array::from(vec![1.5]))),
            ("Float32 finite", Arc::new(Float32Array::from(vec![1.5f32]))),
        ];

        for (label, col) in cases {
            let cell = arrow_cell(col.as_ref(), &field_for(col.as_ref()), 0).unwrap_or_else(|e| {
                panic!("{label} has a value at row 0 but failed to render: {e}")
            });
            assert_ne!(
                cell,
                Value::Null,
                "{label} has a value at row 0 but rendered as null"
            );
        }
    }

    /// A value that cannot be converted is rendered the same way the service
    /// renders it — never as a `null`.
    ///
    /// `null` is the outcome this must not produce: it would invent an absence
    /// the service never sent, indistinguishable downstream from a real null.
    /// What arrow-json actually does with an out-of-range timestamp is write
    /// `ERROR: <msg>` into the cell and report success, which is its own
    /// silent-wrong-value problem — but it is *upstream* behaviour and the
    /// service exhibits it identically, so correcting it only here would
    /// recreate the client/service divergence this renderer exists to remove.
    /// Pinned so a future change to it is a deliberate one, made on both sides.
    #[test]
    fn an_unconvertible_value_matches_the_service_and_is_never_null() {
        use arrow::array::TimestampSecondArray;

        // Far outside the range a second-resolution timestamp can represent.
        let col = TimestampSecondArray::from(vec![i64::MAX]);
        let cell = arrow_cell(&col, &field_for(&col), 0).expect("arrow-json reports success here");

        match &cell {
            Value::Null => panic!("a present value was reported as null"),
            Value::String(s) => assert!(
                s.starts_with("ERROR:"),
                "expected arrow-json's error-in-cell text, got {s:?}"
            ),
            other => panic!("unexpected rendering: {other:?}"),
        }
    }

    /// A non-finite float renders as `null`, the way the service renders it.
    ///
    /// The service nullifies non-finite floats before encoding, because JSON
    /// has no `NaN` or `Infinity`. arrow-json independently emits `null` for
    /// them, so both sides agree — but if it ever emitted the bare tokens they
    /// would not be valid JSON, the parse would fail, and a single bad cell
    /// would abort an entire result. Pinned so that change is caught here.
    #[test]
    fn non_finite_floats_render_as_null_like_the_service() {
        use arrow::array::{ArrayRef, Float32Array, Float64Array};

        let cases: Vec<(&str, ArrayRef)> = vec![
            ("f64 NaN", Arc::new(Float64Array::from(vec![f64::NAN]))),
            (
                "f64 +inf",
                Arc::new(Float64Array::from(vec![f64::INFINITY])),
            ),
            (
                "f64 -inf",
                Arc::new(Float64Array::from(vec![f64::NEG_INFINITY])),
            ),
            ("f32 NaN", Arc::new(Float32Array::from(vec![f32::NAN]))),
        ];
        for (label, col) in cases {
            let got = arrow_cell(col.as_ref(), &field_for(col.as_ref()), 0)
                .unwrap_or_else(|e| panic!("{label} aborted the result: {e}"));
            assert_eq!(got, Value::Null, "{label} rendered as {got:?}");
        }
    }

    /// A decimal wider than an `f64` is rounded here. Known limitation, pinned
    /// so it is a discovered fact rather than a surprise.
    ///
    /// arrow-json writes a decimal as an unquoted JSON number at full width and
    /// the service passes those bytes through untouched, so a `DECIMAL(38,2)`
    /// reaches this side with all its digits and then loses everything past the
    /// 17th when parsed into a `Value`.
    ///
    /// `serde_json`'s `arbitrary_precision` would fix it and must not be used:
    /// the feature is crate-wide, and with it on a `Number` serializes as the
    /// private struct `$serde_json::private::Number`, which serde_yaml renders
    /// as a nested mapping — corrupting `-o yaml` for commands that have
    /// nothing to do with rendering results. The honest fix is to stop routing
    /// the json output path through `Value` at all, as the service does; that
    /// is a change to the output layer, not to this function.
    #[test]
    fn a_wide_decimal_is_rounded_a_known_limitation() {
        use arrow::array::Decimal128Array;

        let col = Decimal128Array::from(vec![123456789012345678901234567890123456i128])
            .with_precision_and_scale(38, 2)
            .expect("valid decimal");
        let got = arrow_cell(&col, &field_for(&col), 0).expect("decimal renders");

        // Full precision would be 1234567890123456789012345678901234.56.
        assert_eq!(got.to_string(), "1.2345678901234568e+33");
    }

    /// `-o yaml` must render a JSON number as a number.
    ///
    /// This exists to fail loudly if `serde_json`'s `arbitrary_precision` is
    /// ever enabled: the feature is crate-wide, and serde_yaml does not know
    /// about the private struct a `Number` then serializes as, so every yaml
    /// output carrying a `Value` number becomes a nested mapping. Nothing else
    /// in the suite covers yaml, so without this the breakage would ship.
    #[test]
    fn yaml_renders_a_json_number_as_a_number() {
        let v: Value = serde_json::json!({"dimensions": 1536});
        let yaml = serde_yaml::to_string(&v).expect("yaml");
        assert_eq!(yaml.trim(), "dimensions: 1536", "yaml corrupted: {yaml}");
        assert!(
            !yaml.contains("private::Number"),
            "serde_json's arbitrary_precision leaked into yaml: {yaml}"
        );
    }

    #[test]
    fn hint_for_missing_database_context() {
        let tip = cross_source_hint(
            "a database is required: set the X-Database-Id header or the database_id body field",
        )
        .expect("missing-database error should produce a hint");
        assert!(tip.contains("hotdata databases use"), "tip: {tip}");
        assert!(tip.contains("hotdata databases attach"), "tip: {tip}");
    }

    #[test]
    fn hint_for_unattached_catalog_names_the_catalog() {
        let tip = cross_source_hint("table 'github.github.issues' not found")
            .expect("qualified not-found should produce a hint");
        // The catalog (first dotted segment), not the schema/table, drives the hint.
        assert!(tip.contains("'github'"), "tip: {tip}");
        assert!(
            tip.contains("hotdata databases attach github"),
            "tip: {tip}"
        );
    }

    #[test]
    fn no_hint_for_unqualified_not_found() {
        // A bare name (no catalog prefix) isn't an attach problem — don't guess.
        assert!(cross_source_hint("table 'orders' not found").is_none());
    }

    #[test]
    fn no_hint_for_unrelated_error() {
        assert!(cross_source_hint("syntax error at or near \"SELCT\"").is_none());
        assert!(cross_source_hint("429: OVERLOADED").is_none());
    }

    #[test]
    fn resolve_inline_follows_truncated_result_to_full_arrow() {
        use arrow::array::{Int64Array, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::ipc::writer::StreamWriter;

        // Full result has 3 rows — more than the 1-row inline preview.
        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let mut ipc: Vec<u8> = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut ipc, &schema).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }

        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/vnd.apache.arrow.stream")
            .with_body(ipc)
            .create();

        // The inline response carries a query-level warning and execution time
        // (execution_time_ms=5 from `truncated_preview`) that must survive the
        // Arrow follow, which otherwise hardcodes them to None.
        let mut resp = truncated_preview(Some("res_1"));
        resp.warning = Some(Some("approximate aggregate".to_string()));

        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));
        let resolved = resolve_inline(&api, resp);

        // Followed the truncated preview to the full 3-row result.
        assert_eq!(resolved.row_count, 3);
        assert_eq!(resolved.rows.len(), 3);
        assert_eq!(resolved.result_id.as_deref(), Some("res_1"));
        // The held rows are now the whole result: complete, not a preview.
        assert!(!resolved.truncated);
        assert_eq!(resolved.total_row_count, Some(3));
        // Inline warning + timing carried through, not dropped by the fetch.
        assert_eq!(resolved.warning.as_deref(), Some("approximate aggregate"));
        assert_eq!(resolved.execution_time_ms, Some(5));
        // The run id from the inline response must survive the Arrow follow — the
        // Arrow path itself has no run id, so the follow branch must stamp it.
        assert_eq!(resolved.query_run_id.as_deref(), Some("qrun_1"));
        m.assert();
    }

    #[test]
    fn resolve_inline_falls_back_to_preview_when_follow_fetch_fails() {
        // Truncated with a result_id, but the follow-up Arrow fetch fails (500).
        // The CLI must NOT hard-exit (which would also discard the preview it
        // already holds) — it degrades to the preview, marks it incomplete, and
        // explains. A returning call here is itself the assertion that no exit
        // happened.
        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .with_status(500)
            .with_body("boom")
            .create();

        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));
        let resolved = resolve_inline(&api, truncated_preview(Some("res_1")));

        // Preview kept, flagged incomplete so print_result fails closed.
        assert!(resolved.truncated);
        assert_eq!(resolved.row_count, 1);
        assert_eq!(resolved.rows.len(), 1);
        // The fixture reports no total, so the table renders "1 of ? rows".
        assert_eq!(resolved.total_row_count, None);
        let warning = resolved.warning.as_deref().unwrap_or("");
        assert!(warning.contains("truncated"), "warning: {warning:?}");
        assert!(
            warning.contains("could not fetch full result"),
            "warning: {warning:?}"
        );
        m.assert();
    }

    #[test]
    fn resolve_inline_returns_untruncated_preview_without_fetching() {
        // truncated=false short-circuits before any network call; point the Api
        // at a server with no mocks so an erroneous fetch would fail loudly.
        let server = mockito::Server::new();
        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));

        let mut resp = hotdata::models::QueryResponse::new(
            vec!["x".to_string()],
            5,
            vec![false],
            2,
            "qrun_2".to_string(),
            2,
            vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
            false, // not truncated
        );
        resp.result_id = Some(Some("res_2".to_string()));

        let resolved = resolve_inline(&api, resp);
        assert_eq!(resolved.row_count, 2);
        assert_eq!(
            resolved.rows,
            vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]]
        );
        assert_eq!(resolved.result_id.as_deref(), Some("res_2"));
        // Complete result: not a preview, total backfilled from held rows.
        assert!(!resolved.truncated);
        assert_eq!(resolved.total_row_count, Some(2));
    }

    #[test]
    fn resolve_inline_truncated_without_result_id_warns_and_keeps_preview() {
        // Truncated but persistence never started (result_id is null): the full
        // result is unfetchable, so keep the preview and surface a warning.
        let server = mockito::Server::new();
        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));

        // The server reports the grand total even though it couldn't persist;
        // it must survive onto the preview so structured output exposes it.
        let mut resp = truncated_preview(None);
        resp.total_row_count = Some(Some(100));

        let resolved = resolve_inline(&api, resp);
        assert!(resolved.truncated);
        assert_eq!(resolved.row_count, 1);
        assert_eq!(resolved.rows.len(), 1);
        assert_eq!(resolved.total_row_count, Some(100));
        assert!(
            resolved
                .warning
                .as_deref()
                .unwrap_or("")
                .contains("truncated")
        );
    }

    #[test]
    fn json_body_exposes_truncation_for_incomplete_preview() {
        // A JSON consumer must be able to detect a partial result from the body
        // alone — not only from a stderr warning. truncated=true, row_count <
        // total, both present. The stderr-only `warning` must NOT leak in.
        let result = QueryResponse {
            query_run_id: None,
            result_id: None,
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            total_row_count: Some(100),
            truncated: true,
            execution_time_ms: Some(5),
            warning: Some("result truncated to a preview".to_string()),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["truncated"], serde_json::json!(true));
        assert_eq!(json["row_count"], serde_json::json!(1));
        assert_eq!(json["total_row_count"], serde_json::json!(100));
        assert!(json.get("warning").is_none(), "warning leaked into JSON");
    }

    #[test]
    fn json_body_marks_complete_result_not_truncated() {
        let result = QueryResponse {
            query_run_id: Some("qrun_9".to_string()),
            result_id: Some("res_9".to_string()),
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)], vec![serde_json::json!(2)]],
            row_count: 2,
            total_row_count: Some(2),
            truncated: false,
            execution_time_ms: Some(5),
            warning: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["truncated"], serde_json::json!(false));
        assert_eq!(json["row_count"], serde_json::json!(2));
        assert_eq!(json["total_row_count"], serde_json::json!(2));
        // A complete result exits 0 — pipelines proceed.
        assert_eq!(result_exit_code(&result), 0);
    }

    #[test]
    fn incomplete_preview_fails_closed_with_distinct_exit_code() {
        // The fail-closed contract: an incomplete preview maps to a non-zero,
        // non-generic exit code so a pipeline breaks instead of ingesting a
        // partial result that exited 0.
        let result = QueryResponse {
            query_run_id: None,
            result_id: None,
            columns: vec!["id".to_string()],
            rows: vec![vec![serde_json::json!(1)]],
            row_count: 1,
            total_row_count: Some(100),
            truncated: true,
            execution_time_ms: Some(5),
            warning: Some("result truncated to a preview".to_string()),
        };
        assert_eq!(result_exit_code(&result), EXIT_INCOMPLETE_RESULT);
        assert_ne!(EXIT_INCOMPLETE_RESULT, 0);
        // Distinct from the generic failure codes used elsewhere in this module.
        assert_ne!(EXIT_INCOMPLETE_RESULT, 1);
        assert_ne!(EXIT_INCOMPLETE_RESULT, 2);
    }

    /// Build a minimal display `QueryResponse` for footer rendering tests.
    fn display_result(
        row_count: u64,
        total_row_count: Option<u64>,
        truncated: bool,
    ) -> QueryResponse {
        QueryResponse {
            query_run_id: None,
            result_id: None,
            columns: vec!["id".to_string()],
            rows: Vec::new(),
            row_count,
            total_row_count,
            truncated,
            execution_time_ms: Some(5),
            warning: None,
        }
    }

    #[test]
    fn table_footer_marks_incomplete_preview_loudly() {
        let footer = table_footer(&display_result(1, Some(100), true));
        assert!(footer.contains("INCOMPLETE PREVIEW"), "footer: {footer}");
        assert!(footer.contains("1 of 100 rows"), "footer: {footer}");
    }

    #[test]
    fn table_footer_renders_question_mark_for_unknown_total() {
        // Truncated with no server-reported total → "1 of ? rows".
        let footer = table_footer(&display_result(1, None, true));
        assert!(footer.contains("1 of ? rows"), "footer: {footer}");
        assert!(footer.contains("INCOMPLETE PREVIEW"), "footer: {footer}");
    }

    #[test]
    fn table_footer_for_complete_result_is_plain() {
        let footer = table_footer(&display_result(2, Some(2), false));
        assert!(!footer.contains("INCOMPLETE"), "footer: {footer}");
        assert!(footer.starts_with("2 rows"), "footer: {footer}");
    }

    #[test]
    fn table_footer_includes_run_id_when_present() {
        // The run id lets a user follow up with `hotdata queries <id>` for
        // run-level metadata (bytes/rows scanned). It precedes the result-id.
        let mut result = display_result(2, Some(2), false);
        result.query_run_id = Some("qrun_7b3e04".to_string());
        result.result_id = Some("res_9f2a1c".to_string());
        let footer = table_footer(&result);
        assert!(footer.contains("[run: qrun_7b3e04]"), "footer: {footer}");
        let run_at = footer.find("[run:").unwrap();
        let res_at = footer.find("[result-id:").unwrap();
        assert!(run_at < res_at, "run id should precede result id: {footer}");
    }

    #[test]
    fn table_footer_omits_run_id_when_absent() {
        // The Arrow-only fetch path has no run id; the footer must not render an
        // empty `[run: ]` tag.
        let footer = table_footer(&display_result(2, Some(2), false));
        assert!(!footer.contains("[run:"), "footer: {footer}");
    }

    #[test]
    fn json_body_exposes_query_run_id() {
        let mut result = display_result(2, Some(2), false);
        result.query_run_id = Some("qrun_7b3e04".to_string());
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["query_run_id"], serde_json::json!("qrun_7b3e04"));
    }

    #[test]
    fn inline_response_carries_query_run_id_from_sdk() {
        // The SDK inline response carries the run id; the CLI must not drop it.
        let resolved = query_response_from_sdk(truncated_preview(Some("res_1")));
        assert_eq!(resolved.query_run_id.as_deref(), Some("qrun_1"));
    }

    #[test]
    fn resolve_inline_preserves_existing_warning_when_following_fails() {
        // A truncated response with no result_id often arrives with an SDK
        // warning explaining why persistence didn't start. The truncation note
        // is appended to it, not allowed to clobber it.
        let server = mockito::Server::new();
        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));

        let mut resp = truncated_preview(None);
        resp.warning = Some(Some(
            "result persistence could not be initiated".to_string(),
        ));

        let resolved = resolve_inline(&api, resp);
        let warning = resolved.warning.as_deref().unwrap_or("");
        assert!(
            warning.contains("result persistence could not be initiated"),
            "original warning dropped: {warning:?}"
        );
        assert!(
            warning.contains("truncated"),
            "truncation note missing: {warning:?}"
        );
    }

    #[test]
    fn run_execution_time_ms_maps_wire_double_option() {
        // A reported time survives; a null (`Some(None)`/`None`) becomes None so
        // the display shows an em dash rather than a bogus 0; a negative clamps to 0.
        assert_eq!(run_execution_time_ms(Some(Some(4200))), Some(4200));
        assert_eq!(run_execution_time_ms(Some(None)), None);
        assert_eq!(run_execution_time_ms(None), None);
        assert_eq!(run_execution_time_ms(Some(Some(-1))), Some(0));
    }

    #[test]
    fn fetch_arrow_result_with_timing_carries_run_execution_time() {
        // Regression for #183: a query that falls back to async fetches its result
        // via Arrow (which carries no timing) and must be stamped with the run's
        // own `execution_time_ms`, not the hardcoded None.
        use arrow::array::{Int64Array, RecordBatch};
        use arrow::datatypes::{DataType, Field, Schema};
        use arrow::ipc::writer::StreamWriter;

        let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int64Array::from(vec![1, 2, 3]))],
        )
        .unwrap();
        let mut ipc: Vec<u8> = Vec::new();
        {
            let mut writer = StreamWriter::try_new(&mut ipc, &schema).unwrap();
            writer.write(&batch).unwrap();
            writer.finish().unwrap();
        }

        let mut server = mockito::Server::new();
        let m = server
            .mock("GET", "/v1/results/res_1")
            .match_query(mockito::Matcher::UrlEncoded(
                "format".into(),
                "arrow".into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/vnd.apache.arrow.stream")
            .with_body(ipc)
            .create();

        let api = Api::test_new_scoped(&server.url(), "test-jwt", Some("ws-1"), Some("db-1"));
        // The async poll response reported the run took 4200ms.
        let result = fetch_arrow_result_with_timing(&api, "res_1", Some(Some(4200)));

        assert_eq!(result.row_count, 3);
        // The slow query's timing is preserved, not dropped to null (#183).
        assert_eq!(result.execution_time_ms, Some(4200));
        m.assert();
    }
}
