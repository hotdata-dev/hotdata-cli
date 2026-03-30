use crate::api::ApiClient;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct ResultEntry {
    id: String,
    status: String,
    created_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    results: Vec<ResultEntry>,
    count: u64,
    has_more: bool,
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let params = [
        ("limit", limit.map(|l| l.to_string())),
        ("offset", offset.map(|o| o.to_string())),
    ];
    let body: ListResponse = api.get_with_params("/results", &params);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.results).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.results).unwrap()),
        "table" => {
            if body.results.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No results found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.results.iter().map(|r| vec![
                    r.id.clone(),
                    r.status.clone(),
                    crate::util::format_date(&r.created_at),
                ]).collect();
                crate::table::print(&["ID", "STATUS", "CREATED AT"], &rows);
            }
            if body.has_more {
                let next = offset.unwrap_or(0) + body.count as u32;
                use crossterm::style::Stylize;
                eprintln!("{}", format!("showing {} results — use --offset {next} for more", body.count).dark_grey());
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(result_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let path = format!("/results/{result_id}");
    let result: crate::query::QueryResponse = api.get(&path);

    crate::query::print_result(&result, format);
}
