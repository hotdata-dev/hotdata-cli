use crate::api::ApiClient;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Deserialize, Serialize)]
struct Dataset {
    id: String,
    label: String,
    #[serde(default = "default_schema")]
    schema_name: String,
    table_name: String,
    created_at: String,
    updated_at: String,
}

fn default_schema() -> String {
    "main".to_string()
}

#[derive(Deserialize)]
struct CreateResponse {
    id: String,
    label: String,
    #[serde(default = "default_schema")]
    schema_name: String,
    table_name: String,
}

#[derive(Deserialize)]
struct ListResponse {
    datasets: Vec<Dataset>,
    count: u64,
    has_more: bool,
}

#[derive(Deserialize, Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Deserialize, Serialize)]
struct DatasetDetail {
    id: String,
    label: String,
    schema_name: String,
    table_name: String,
    source_type: String,
    created_at: String,
    updated_at: String,
    columns: Vec<Column>,
}

#[derive(Deserialize, Serialize)]
struct UpdateResponse {
    id: String,
    label: String,
    // Not currently in runtimedb's UpdateDatasetResponse; kept Optional so we
    // print `full_name` only when the server actually returns the schema.
    // Synthesizing "main" is wrong for sandbox-scoped datasets where
    // schema_name == sandbox_id.
    #[serde(default)]
    schema_name: Option<String>,
    table_name: String,
    #[serde(default)]
    latest_version: Option<i32>,
    #[serde(default)]
    pinned_version: Option<i32>,
    updated_at: String,
}

fn create_dataset(
    api: &ApiClient,
    description: Option<&str>,
    name: &str,
    source: serde_json::Value,
) {
    let mut body = json!({ "table_name": name, "source": source });
    if let Some(desc) = description {
        body["label"] = json!(desc);
    }

    let (status, resp_body) = api.post_raw("/datasets", &body);

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let dataset: CreateResponse = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    use crossterm::style::Stylize;
    println!("{}", "Dataset created".green());
    println!("id:         {}", dataset.id);
    println!("label:      {}", dataset.label);
    println!(
        "full_name:  datasets.{}.{}",
        dataset.schema_name, dataset.table_name
    );
}

pub fn create_from_query(workspace_id: &str, sql: &str, description: Option<&str>, name: &str) {
    let api = ApiClient::new(Some(workspace_id));
    create_dataset(&api, description, name, json!({ "sql": sql }));
}

pub fn create_from_saved_query(
    workspace_id: &str,
    query_id: &str,
    description: Option<&str>,
    name: &str,
) {
    let api = ApiClient::new(Some(workspace_id));
    create_dataset(&api, description, name, json!({ "saved_query_id": query_id }));
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let params = [
        ("limit", limit.map(|l| l.to_string())),
        ("offset", offset.map(|o| o.to_string())),
    ];
    let body: ListResponse = api.get_with_params("/datasets", &params);

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.datasets).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.datasets).unwrap()),
        "table" => {
            if body.datasets.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No datasets found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body
                    .datasets
                    .iter()
                    .map(|d| {
                        vec![
                            d.id.clone(),
                            d.label.clone(),
                            format!("datasets.{}.{}", d.schema_name, d.table_name),
                            crate::util::format_date(&d.created_at),
                        ]
                    })
                    .collect();
                crate::table::print(&["ID", "LABEL", "FULL NAME", "CREATED AT"], &rows);
            }
            if body.has_more {
                let next = offset.unwrap_or(0) + body.count as u32;
                use crossterm::style::Stylize;
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

pub fn get(dataset_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));

    let d: DatasetDetail = api.get(&format!("/datasets/{dataset_id}"));

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&d).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&d).unwrap()),
        "table" => {
            let created_at = crate::util::format_date(&d.created_at);
            let updated_at = crate::util::format_date(&d.updated_at);
            println!("id:          {}", d.id);
            println!("label:       {}", d.label);
            println!("full_name:   datasets.main.{}", d.table_name);
            println!("source_type: {}", d.source_type);
            println!("created_at:  {created_at}");
            println!("updated_at:  {updated_at}");
            if !d.columns.is_empty() {
                println!();
                let rows: Vec<Vec<String>> = d
                    .columns
                    .iter()
                    .map(|col| {
                        vec![
                            col.name.clone(),
                            col.data_type.clone(),
                            col.nullable.to_string(),
                        ]
                    })
                    .collect();
                crate::table::print(&["COLUMN", "DATA TYPE", "NULLABLE"], &rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn update(
    dataset_id: &str,
    workspace_id: &str,
    label: Option<&str>,
    table_name: Option<&str>,
    format: &str,
) {
    if label.is_none() && table_name.is_none() {
        eprintln!("error: provide at least one of --description or --name.");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));

    let mut body = json!({});
    if let Some(l) = label {
        body["label"] = json!(l);
    }
    if let Some(tn) = table_name {
        body["table_name"] = json!(tn);
    }

    let d: UpdateResponse = api.put(&format!("/datasets/{dataset_id}"), &body);

    use crossterm::style::Stylize;
    eprintln!("{}", "Dataset updated".green());
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&d).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&d).unwrap()),
        "table" => {
            println!("id:          {}", d.id);
            println!("label:       {}", d.label);
            match &d.schema_name {
                Some(schema) => {
                    println!("full_name:   datasets.{}.{}", schema, d.table_name);
                }
                None => {
                    println!("table_name:  {}", d.table_name);
                    eprintln!(
                        "{}",
                        format!(
                            "(run `hotdata datasets {}` to see the qualified name)",
                            d.id
                        )
                        .dark_grey()
                    );
                }
            }
            println!("updated_at:  {}", crate::util::format_date(&d.updated_at));
        }
        _ => unreachable!(),
    }
}

pub fn refresh(workspace_id: &str, dataset_id: &str, async_mode: bool) {
    use crossterm::style::Stylize;

    let mut body = json!({
        "dataset_id": dataset_id,
    });
    if async_mode {
        body["async"] = json!(true);
    }

    let api = ApiClient::new(Some(workspace_id));
    let (status, resp_body) = api.post_raw("/refresh", &body);

    if !status.is_success() {
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let parsed: serde_json::Value = serde_json::from_str(&resp_body).unwrap_or_default();

    if async_mode {
        let job_id = parsed["id"].as_str().unwrap_or("unknown");
        println!("{}", "Dataset refresh submitted.".green());
        println!("job_id: {}", job_id);
        println!(
            "{}",
            format!("Use 'hotdata jobs {}' to check status.", job_id).dark_grey()
        );
        return;
    }

    let id = parsed["id"].as_str().unwrap_or("unknown");
    let version = parsed["version"].as_i64().unwrap_or(0);
    let dataset_status = parsed["status"].as_str().unwrap_or("");
    println!("{}", "Dataset refresh completed.".green());
    println!(
        "{}",
        format!("  id: {id}, version: {version}, status: {dataset_status}").dark_grey()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirrors runtimedb's `UpdateDatasetResponse` (see runtimedb/src/http/models.rs).
    /// The CLI must deserialize this exact shape — schema_name, source_type,
    /// created_at, and columns are NOT in the response. If runtimedb's response
    /// gains or loses fields, update this fixture in lockstep.
    #[test]
    fn update_response_deserializes_runtimedb_payload() {
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "url_test",
            "table_name": "url_test",
            "latest_version": 3,
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.id, "ds_abc123");
        assert_eq!(resp.label, "url_test");
        assert_eq!(resp.table_name, "url_test");
        // The server doesn't currently send schema_name, so we don't synthesize
        // one — sandbox-scoped datasets live under datasets.<sandbox_id>.<table>,
        // not datasets.main.*, and a fabricated "main" would mislead users.
        assert!(resp.schema_name.is_none());
        assert_eq!(resp.latest_version, Some(3));
        assert!(resp.pinned_version.is_none());
    }

    #[test]
    fn update_response_uses_schema_name_when_server_supplies_it() {
        // Forward-compat: if runtimedb later includes schema_name, we use it.
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "x",
            "schema_name": "sandbox_xyz",
            "table_name": "x",
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.schema_name.as_deref(), Some("sandbox_xyz"));
    }

    #[test]
    fn update_response_handles_pinned_version() {
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "x",
            "table_name": "x",
            "latest_version": 5,
            "pinned_version": 2,
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateResponse = serde_json::from_value(body).unwrap();
        assert_eq!(resp.pinned_version, Some(2));
    }

    #[test]
    fn update_response_tolerates_missing_latest_version() {
        // Defensive: treat latest_version as optional in case the server omits it.
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "x",
            "table_name": "x",
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateResponse = serde_json::from_value(body).unwrap();
        assert!(resp.latest_version.is_none());
    }
}
