//! `hotdata run` — one execution attempt of an ingest.
//!
//! Runs are append-only: a scheduled ingest accumulates them, and each one
//! records the datasource config version, selector, destination, and schedule
//! it used — so a run stays explainable after the datasource has been
//! reconfigured or the schedule changed. That is what `run show` prints.
//!
//! Not to be confused with `hotdata databases run <cmd>` (which launches a
//! child process with database-scoped credentials) or `hotdata jobs` (platform
//! background jobs). This noun is ingest execution only.
//!
//! **Script-friendly exit codes**, matching `query status`: 0 succeeded,
//! 1 failed or cancelled, 2 still in flight. `-o json` still lands on stdout in
//! every case, so a non-zero exit never costs the caller the detail.

use crate::client::ingest::IngestClient;
use crate::commands::ingest_common::{
    cell, field, hint, presented_run_status, render, run_exit_code, run_status_cell,
};
use crate::util;

#[derive(clap::Subcommand)]
pub enum RunCommands {
    /// Show one run: status, the snapshots it used, and its timings
    ///
    /// Exits 0 when the run succeeded, 1 when it failed or was cancelled, and
    /// 2 while it is still queued or running.
    Show {
        /// Run id (from `hotdata ingest runs <ingest-id>`)
        run_id: String,
    },
}

/// Entry point from `main`. Keeps `main.rs` thin — one call per group.
pub fn dispatch(workspace_id: &str, output: &str, command: RunCommands) {
    match command {
        RunCommands::Show { run_id } => show(workspace_id, output, &run_id),
    }
}

fn show(workspace_id: &str, output: &str, run_id: &str) {
    let client = IngestClient::new(workspace_id);
    let run = client.get_run(run_id).unwrap_or_else(|e| e.exit());

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

fn compact_json(v: &serde_json::Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "-".into())
}
