use crate::api::ApiClient;
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

#[derive(Deserialize)]
struct ListResponse {
    jobs: Vec<Job>,
}

pub fn get(job_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let job: Job = api.get(&format!("/jobs/{job_id}"));

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

fn fetch_jobs(
    api: &ApiClient,
    job_type: Option<&str>,
    status: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Vec<Job> {
    let params = [
        ("job_type", job_type.map(String::from)),
        ("status", status.map(String::from)),
        ("limit", limit.map(|l| l.to_string())),
        ("offset", offset.map(|o| o.to_string())),
    ];
    let resp: ListResponse = api.get_with_params("/jobs", &params);
    resp.jobs
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
    let api = ApiClient::new(Some(workspace_id));

    let jobs = if !all && status.is_none() {
        // Default: show only active jobs (pending + running)
        fetch_jobs(&api, job_type, Some("pending,running"), limit, offset)
    } else {
        fetch_jobs(&api, job_type, status, limit, offset)
    };

    let body = ListResponse { jobs };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.jobs).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.jobs).unwrap()),
        "table" => {
            if body.jobs.is_empty() {
                use crossterm::style::Stylize;
                let msg = if !all && status.is_none() {
                    "No active jobs found."
                } else {
                    "No jobs found."
                };
                eprintln!("{}", msg.dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body
                    .jobs
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
