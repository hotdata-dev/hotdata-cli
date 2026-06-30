use crate::sdk::Api;
use hotdata::models::{JobStatusResponse, JobType};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Job {
    id: String,
    job_type: String,
    status: String,
    attempts: u64,
    created_at: String,
    completed_at: Option<String>,
    error_message: Option<String>,
    result: Option<serde_json::Value>,
}

impl From<JobStatusResponse> for Job {
    fn from(j: JobStatusResponse) -> Self {
        Job {
            id: j.id,
            job_type: j.job_type.to_string(),
            status: j.status.to_string(),
            attempts: j.attempts.max(0) as u64,
            created_at: j.created_at,
            completed_at: j.completed_at.flatten(),
            error_message: j.error_message.flatten(),
            result: j
                .result
                .flatten()
                .and_then(|r| serde_json::to_value(*r).ok()),
        }
    }
}

/// Map the clap-validated job-type string to the SDK enum. The CLI already
/// restricts `--type` to these values, so an unknown string is unreachable;
/// fall back to `None` (no filter) rather than panic if that ever changes.
fn parse_job_type(s: &str) -> Option<JobType> {
    match s {
        "noop" => Some(JobType::Noop),
        "data_refresh_table" => Some(JobType::DataRefreshTable),
        "data_refresh_connection" => Some(JobType::DataRefreshConnection),
        "create_index" => Some(JobType::CreateIndex),
        "managed_load" => Some(JobType::ManagedLoad),
        _ => None,
    }
}

pub fn get(job_id: &str, workspace_id: &str, format: &str) {
    let api = Api::new(Some(workspace_id));
    let job: Job =
        crate::sdk::block_with_wakeup(&api, "Loading job…", api.client().jobs().get(job_id))
            .unwrap_or_else(|e| e.exit())
            .into();

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&job).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&job).unwrap()),
        "table" => {
            use crossterm::style::Stylize;
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            let status_colored = match job.status.as_str() {
                "succeeded" => job.status.green().to_string(),
                "failed" => job.status.red().to_string(),
                "running" | "pending" => job.status.yellow().to_string(),
                "partially_succeeded" => job.status.dark_yellow().to_string(),
                _ => job.status.clone(),
            };
            println!("{}{}", label("id:"), job.id);
            println!("{}{}", label("type:"), job.job_type);
            println!("{}{}", label("status:"), status_colored);
            println!(
                "{}{}",
                label("attempts:"),
                job.attempts.to_string().dark_cyan()
            );
            println!(
                "{}{}",
                label("created:"),
                crate::util::format_date(&job.created_at)
            );
            println!(
                "{}{}",
                label("completed:"),
                job.completed_at
                    .as_deref()
                    .map(crate::util::format_date)
                    .unwrap_or_else(|| "-".dark_grey().to_string())
            );
            if let Some(err) = &job.error_message {
                println!("{}{}", label("error:"), err.as_str().red());
            }
            if let Some(result) = &job.result
                && !result.is_null()
            {
                println!(
                    "{}{}",
                    label("result:"),
                    serde_json::to_string_pretty(result).unwrap()
                );
            }
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotdata::models::ListJobsResponse;

    // Regression for #160: a `managed_load` row must deserialize, not blow up
    // the whole `jobs list` response. Proves SDK 0.6.0's JobType carries the
    // variant the server emits for `databases load`.
    #[test]
    fn list_response_with_managed_load_deserializes() {
        let body = r#"{
            "jobs": [
                {
                    "id": "job_1",
                    "job_type": "managed_load",
                    "status": "succeeded",
                    "attempts": 1,
                    "created_at": "2026-06-18T06:00:00Z"
                }
            ]
        }"#;
        let resp: ListJobsResponse = serde_json::from_str(body).expect("managed_load must parse");
        assert_eq!(resp.jobs[0].job_type, JobType::ManagedLoad);
    }

    #[test]
    fn parse_job_type_accepts_managed_load() {
        assert_eq!(parse_job_type("managed_load"), Some(JobType::ManagedLoad));
    }
}

fn fetch_jobs(
    api: &Api,
    job_type: Option<&str>,
    status: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Vec<Job> {
    let resp = crate::sdk::block(api.client().jobs().list(
        job_type.and_then(parse_job_type),
        status,
        limit.map(|l| l as i32),
        offset.map(|o| o as i32),
    ))
    .unwrap_or_else(|e| e.exit());
    resp.jobs.into_iter().map(Job::from).collect()
}

pub fn list(
    workspace_id: &str,
    job_type: Option<&str>,
    status: Option<&str>,
    all: bool,
    limit: Option<u32>,
    offset: Option<u32>,
    format: &str,
) {
    let api = Api::new(Some(workspace_id));

    let jobs = if !all && status.is_none() {
        // Default: show only active jobs (pending + running)
        fetch_jobs(&api, job_type, Some("pending,running"), limit, offset)
    } else {
        fetch_jobs(&api, job_type, status, limit, offset)
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&jobs).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&jobs).unwrap()),
        "table" => {
            if jobs.is_empty() {
                use crossterm::style::Stylize;
                let msg = if !all && status.is_none() {
                    "No active jobs found."
                } else {
                    "No jobs found."
                };
                eprintln!("{}", msg.dark_grey());
            } else {
                let rows: Vec<Vec<String>> = jobs
                    .iter()
                    .map(|j| {
                        vec![
                            j.id.clone(),
                            j.job_type.clone(),
                            j.status.clone(),
                            j.attempts.to_string(),
                            crate::util::format_date(&j.created_at),
                            j.completed_at
                                .as_deref()
                                .map(crate::util::format_date)
                                .unwrap_or_else(|| "-".to_string()),
                        ]
                    })
                    .collect();
                crate::table::print(
                    &["ID", "TYPE", "STATUS", "ATTEMPTS", "CREATED", "COMPLETED"],
                    &rows,
                );
            }
        }
        _ => unreachable!(),
    }
}
