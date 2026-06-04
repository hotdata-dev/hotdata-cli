use crate::sdk::Api;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct ResultEntry {
    id: String,
    status: String,
    created_at: String,
    #[serde(default)]
    row_count: Option<u64>,
    #[serde(default)]
    query_run_id: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse {
    results: Vec<ResultEntry>,
    count: u64,
    has_more: bool,
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
    let api = Api::new(Some(workspace_id));

    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(l) = limit {
        params.push(("limit", l.to_string()));
    }
    if let Some(o) = offset {
        params.push(("offset", o.to_string()));
    }
    // The SDK's typed `results().list()` model drops `row_count`,
    // `query_run_id`, and `expires_at` (the columns the CLI conditionally
    // shows), so deserialize into the CLI's own `ListResponse` via the seam's
    // `get_json` to preserve output byte-for-byte.
    let body: ListResponse = api
        .get_json("/results", &params)
        .unwrap_or_else(|e| e.exit());

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.results).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.results).unwrap()),
        "table" => {
            if body.results.is_empty() {
                eprintln!("{}", "No results found.".dark_grey());
            } else {
                let has_rows = body.results.iter().any(|r| r.row_count.is_some());
                let has_query_run = body.results.iter().any(|r| r.query_run_id.is_some());
                let has_expires = body.results.iter().any(|r| r.expires_at.is_some());

                let rows: Vec<Vec<String>> = body
                    .results
                    .iter()
                    .map(|r| {
                        let mut row = vec![
                            r.id.clone(),
                            crate::util::color_status(&r.status),
                            crate::util::format_date(&r.created_at),
                        ];
                        if has_rows {
                            row.push(r.row_count.map(|n| n.to_string()).unwrap_or_else(|| "-".to_string()));
                        }
                        if has_query_run {
                            row.push(r.query_run_id.as_deref().unwrap_or("-").to_string());
                        }
                        if has_expires {
                            row.push(
                                r.expires_at
                                    .as_deref()
                                    .map(|s| crate::util::format_date(s))
                                    .unwrap_or_else(|| "-".to_string()),
                            );
                        }
                        row
                    })
                    .collect();

                let mut headers = vec!["ID", "STATUS", "CREATED"];
                if has_rows { headers.push("ROWS"); }
                if has_query_run { headers.push("QUERY_RUN_ID"); }
                if has_expires { headers.push("EXPIRES"); }

                crate::table::print(&headers, &rows);
            }
            if body.has_more {
                let next = offset.unwrap_or(0) + body.count as u32;
                eprintln!(
                    "{}",
                    format!(
                        "showing {} results — use --offset {next} for more",
                        body.count
                    )
                    .dark_grey()
                );
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(result_id: &str, workspace_id: &str, format: &str) {
    let api = Api::new(Some(workspace_id));
    let result = crate::query::fetch_arrow_result(&api, result_id);
    crate::query::print_result(&result, format);
}
