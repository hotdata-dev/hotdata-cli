use crate::client::sdk::Api;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};

/// Subcommands for `hotdata results`.
#[derive(clap::Subcommand)]
pub enum ResultsCommands {
    /// List stored query results
    List {
        /// Maximum number of results (default: 100, max: 1000)
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination offset
        #[arg(long)]
        offset: Option<u32>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

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

pub fn list(
    workspace_id: &str,
    database: Option<&str>,
    limit: Option<u32>,
    offset: Option<u32>,
    format: &str,
) {
    let api = scoped_api(workspace_id, database);
    // Results are database-scoped (the required `X-Database-Id` header the seam
    // sends from the active database). Fail early with a hint when none is set,
    // rather than surfacing the raw server error.
    api.require_database();

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
                            row.push(
                                r.row_count
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            );
                        }
                        if has_query_run {
                            row.push(r.query_run_id.as_deref().unwrap_or("-").to_string());
                        }
                        if has_expires {
                            row.push(
                                r.expires_at
                                    .as_deref()
                                    .map(crate::util::format_date)
                                    .unwrap_or_else(|| "-".to_string()),
                            );
                        }
                        row
                    })
                    .collect();

                let mut headers = vec!["ID", "STATUS", "CREATED"];
                if has_rows {
                    headers.push("ROWS");
                }
                if has_query_run {
                    headers.push("QUERY_RUN_ID");
                }
                if has_expires {
                    headers.push("EXPIRES");
                }

                crate::output::table::print(&headers, &rows);
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

pub fn get(result_id: &str, workspace_id: &str, database: Option<&str>, format: &str) {
    let api = scoped_api(workspace_id, database);
    let result = crate::commands::query::fetch_arrow_result(&api, result_id);
    crate::commands::query::print_result(&result, format);
}

/// Build an [`Api`] scoped to `database` when the caller passed `--database`,
/// otherwise the database resolved at construction (`HOTDATA_DATABASE` /
/// current database).
fn scoped_api(workspace_id: &str, database: Option<&str>) -> Api {
    let api = Api::new(Some(workspace_id));
    match database {
        Some(db) => api.scoped_to_database(db.to_string()),
        None => api,
    }
}
