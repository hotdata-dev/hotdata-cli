use crate::config;
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
    let profile_config = match config::load("default") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => {
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };

    let url = format!("{}/jobs/{job_id}", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let job: Job = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

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
            println!("{}{}", label("attempts:"), job.attempts.to_string().dark_cyan());
            println!("{}{}", label("created:"), crate::util::format_date(&job.created_at));
            println!("{}{}", label("completed:"), job.completed_at.as_deref().map(crate::util::format_date).unwrap_or_else(|| "-".dark_grey().to_string()));
            if let Some(err) = &job.error_message {
                println!("{}{}", label("error:"), err.as_str().red());
            }
            if let Some(result) = &job.result {
                if !result.is_null() {
                    println!("{}{}", label("result:"), serde_json::to_string_pretty(result).unwrap());
                }
            }
        }
        _ => unreachable!(),
    }
}

fn fetch_jobs(
    client: &reqwest::blocking::Client,
    api_key: &str,
    api_url: &str,
    workspace_id: &str,
    job_type: Option<&str>,
    status: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
) -> Vec<Job> {
    let mut params = vec![];
    if let Some(jt) = job_type { params.push(format!("job_type={jt}")); }
    if let Some(s) = status { params.push(format!("status={s}")); }
    if let Some(l) = limit { params.push(format!("limit={l}")); }
    if let Some(o) = offset { params.push(format!("offset={o}")); }

    let mut url = format!("{api_url}/jobs");
    if !params.is_empty() { url = format!("{url}?{}", params.join("&")); }

    let resp = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    };

    if !resp.status().is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    match resp.json::<ListResponse>() {
        Ok(v) => v.jobs,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    }
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
    let profile_config = match config::load("default") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => {
            eprintln!("error: not authenticated. Run 'hotdata auth' to log in.");
            std::process::exit(1);
        }
    };

    let client = reqwest::blocking::Client::new();
    let api_url = profile_config.api_url.to_string();

    let jobs = if !all && status.is_none() {
        // Default: show only active jobs (pending + running)
        fetch_jobs(&client, &api_key, &api_url, workspace_id, job_type, Some("pending,running"), limit, offset)
    } else {
        fetch_jobs(&client, &api_key, &api_url, workspace_id, job_type, status, limit, offset)
    };

    let body = ListResponse { jobs };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.jobs).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.jobs).unwrap()),
        "table" => {
            if body.jobs.is_empty() {
                use crossterm::style::Stylize;
                let msg = if !all && status.is_none() { "No active jobs found." } else { "No jobs found." };
                eprintln!("{}", msg.dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.jobs.iter().map(|j| vec![
                    j.id.clone(),
                    j.job_type.clone(),
                    j.status.clone(),
                    j.attempts.to_string(),
                    crate::util::format_date(&j.created_at),
                    j.completed_at.as_deref().map(crate::util::format_date).unwrap_or_else(|| "-".to_string()),
                ]).collect();
                crate::table::print(&["ID", "TYPE", "STATUS", "ATTEMPTS", "CREATED", "COMPLETED"], &rows);
            }
        }
        _ => unreachable!(),
    }
}
