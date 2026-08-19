//! `hotdata ingest run` — one execution attempt of an ingest.
//!
//! Runs are append-only: a scheduled ingest accumulates them, and each one
//! records the datasource config version, selector, destination, and schedule
//! it used — so a run stays explainable after the datasource has been
//! reconfigured or the schedule changed. That is what this prints.
//!
//! It sits under `ingest` rather than at the top level because "run" on its own
//! is not this subsystem's word to take: `hotdata jobs` is platform background
//! jobs and `hotdata queries` is query run history. A bare `hotdata run` would
//! have to be read as "which of those three?" — and it would spend a name whose
//! obvious eventual meaning is runs of every kind, not ingest's alone.
//!
//! `ingest runs <ingest-id>` lists; `ingest run <run-id>` shows one.
//!
//! **Script-friendly exit codes**, matching `query status`: 0 succeeded,
//! 1 failed or cancelled, 2 still in flight. `-o json` still lands on stdout in
//! every case, so a non-zero exit never costs the caller the detail.

use crate::client::ingest::{IngestClient, Run};
use crate::commands::ingest_common::{
    cell, field, hint, is_terminal, poll_until, presented_run_status, render, run_exit_code,
    run_status_cell, wait_timed_out,
};
use crate::util;

/// Called from `ingest::dispatch` for `IngestCommands::Run`.
pub fn show(workspace_id: &str, output: &str, run_id: &str, wait: bool, wait_timeout: u64) {
    let client = IngestClient::new(workspace_id);
    let run = if wait {
        watch(&client, run_id, wait_timeout)
    } else {
        client.get_run(run_id).unwrap_or_else(|e| e.exit())
    };

    render(output, &run, || {
        field("run id:", &run.run_id);
        field("ingest id:", &cell(run.ingest_id.as_deref()));
        field("datasource id:", &cell(run.datasource_id.as_deref()));
        field(
            "status:",
            &run_status_cell(&run.status, run.stage.as_deref()),
        );
        if let Some(a) = run.attempt {
            field("attempt:", &a.to_string());
        }
        if let Some(d) = run.detail.as_deref().filter(|d| !d.trim().is_empty()) {
            field("detail:", d);
        }
        if let Some(e) = run.error.as_ref().filter(|e| !e.is_null()) {
            field("error:", &compact_json(e));
        }
        // The snapshots are the point of the noun: what this attempt actually
        // used, regardless of what the ingest or datasource says today.
        if let Some(v) = run.config_version_id.as_deref() {
            field("config version:", v);
        }
        if let Some(d) = run.destination_snapshot.as_ref() {
            field("destination:", &compact_json(d));
        }
        if let Some(s) = run.selector_snapshot.as_ref() {
            field("selector:", &compact_json(s));
        }
        for (label, ts) in [
            ("queued:", run.queued_at.as_deref()),
            ("started:", run.started_at.as_deref()),
            ("finished:", run.finished_at.as_deref()),
        ] {
            if let Some(t) = ts {
                field(label, &util::format_date(t));
            }
        }
        if let Some(j) = run.job_name.as_deref() {
            field("job:", j);
        }
        if run_exit_code(&run.status) == 2 {
            let (_, stage) = presented_run_status(&run.status, run.stage.as_deref());
            match stage {
                Some(s) => hint(&format!("Still {s}. Re-run this command to check again.")),
                None => hint("Still in flight. Re-run this command to check again."),
            }
        }
    });
    std::process::exit(run_exit_code(&run.status));
}

/// Re-read the run until it reaches a terminal status.
///
/// Watching, not driving: a queued run is waiting on the scheduler, and no
/// request the CLI can send moves it up the queue. The live stage goes into the
/// spinner so a long load shows what it is doing rather than only that it is
/// still going.
fn watch(client: &IngestClient, run_id: &str, timeout_secs: u64) -> Run {
    let outcome = poll_until(
        "watching the run…",
        timeout_secs,
        || client.get_run(run_id),
        |run| is_terminal(&run.status),
        |run| {
            let (status, stage) = presented_run_status(&run.status, run.stage.as_deref());
            Some(stage.unwrap_or(status))
        },
    );
    match outcome {
        Ok(run) => run,
        Err(_) => wait_timed_out(&format!("hotdata ingest run {run_id} --wait")),
    }
}

fn compact_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "-".into())
}
