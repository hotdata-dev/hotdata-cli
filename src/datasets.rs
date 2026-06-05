use crate::sdk::Api;
use hotdata::models::{
    CreateDatasetRequest, CreateDatasetResponse, DatasetSource, DatasetSourceOneOf1,
    DatasetSourceOneOf2, GetDatasetResponse, RefreshRequest, RefreshResponse, UpdateDatasetRequest,
    UpdateDatasetResponse,
};
use serde::Serialize;

/// Output shape for `create`, preserving the CLI's field order for json/yaml.
#[derive(Serialize)]
struct CreateView {
    id: String,
    label: String,
    schema_name: String,
    table_name: String,
}

impl From<CreateDatasetResponse> for CreateView {
    fn from(r: CreateDatasetResponse) -> Self {
        CreateView {
            id: r.id,
            label: r.label,
            schema_name: r.schema_name,
            table_name: r.table_name,
        }
    }
}

/// Output shape for `list` rows.
#[derive(Serialize)]
struct DatasetView {
    id: String,
    label: String,
    schema_name: String,
    table_name: String,
    created_at: String,
    updated_at: String,
}

/// Output shape for `get`.
#[derive(Serialize)]
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

#[derive(Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

/// Output shape for `update`, preserving the CLI's field order and optional
/// `schema_name`. runtimedb's `UpdateDatasetResponse` does not currently send
/// `schema_name`, so we don't synthesize one — sandbox-scoped datasets live
/// under `datasets.<sandbox_id>.<table>`, not `datasets.main.*`.
#[derive(Serialize)]
struct UpdateView {
    id: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema_name: Option<String>,
    table_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    latest_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pinned_version: Option<i32>,
    updated_at: String,
}

impl From<UpdateDatasetResponse> for UpdateView {
    fn from(r: UpdateDatasetResponse) -> Self {
        UpdateView {
            id: r.id,
            label: r.label,
            // The SDK model carries no schema_name; keep None so we print the
            // unqualified table_name + the "see qualified name" hint.
            schema_name: None,
            table_name: r.table_name,
            latest_version: Some(r.latest_version),
            pinned_version: r.pinned_version.flatten(),
            updated_at: r.updated_at,
        }
    }
}

fn create_dataset(
    api: &Api,
    description: Option<&str>,
    name: &str,
    source: DatasetSource,
    format: &str,
) {
    let label = description.unwrap_or(name).to_string();
    let mut request = CreateDatasetRequest::new(label, source);
    request.table_name = Some(Some(name.to_string()));

    let resp = match crate::sdk::block(api.client().datasets().create(request, api.database_id())) {
        Ok(r) => r,
        Err(e) => e.exit(),
    };
    let dataset = CreateView::from(resp);

    use crossterm::style::Stylize;
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&dataset).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&dataset).unwrap()),
        "table" => {
            eprintln!("{}", "Dataset created".green());
            println!("id:         {}", dataset.id);
            println!("label:      {}", dataset.label);
            println!(
                "full_name:  datasets.{}.{}",
                dataset.schema_name, dataset.table_name
            );
        }
        _ => unreachable!(),
    }
}

pub fn create_from_query(
    workspace_id: &str,
    sql: &str,
    description: Option<&str>,
    name: &str,
    format: &str,
) {
    let api = Api::new(Some(workspace_id));
    let source = DatasetSource::DatasetSourceOneOf2(Box::new(DatasetSourceOneOf2::new(
        sql.to_string(),
        hotdata::models::dataset_source_one_of_2::Type::SqlQuery,
    )));
    create_dataset(&api, description, name, source, format);
}

pub fn create_from_saved_query(
    workspace_id: &str,
    query_id: &str,
    description: Option<&str>,
    name: &str,
    format: &str,
) {
    let api = Api::new(Some(workspace_id));
    let source = DatasetSource::DatasetSourceOneOf1(Box::new(DatasetSourceOneOf1::new(
        query_id.to_string(),
        hotdata::models::dataset_source_one_of_1::Type::SavedQuery,
    )));
    create_dataset(&api, description, name, source, format);
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
    let api = Api::new(Some(workspace_id));

    let body = crate::sdk::block(
        api.client()
            .datasets()
            .list(limit.map(|l| l as i32), offset.map(|o| o as i32)),
    )
    .unwrap_or_else(|e| e.exit());

    let datasets: Vec<DatasetView> = body
        .datasets
        .into_iter()
        .map(|d| DatasetView {
            id: d.id,
            label: d.label,
            schema_name: d.schema_name,
            table_name: d.table_name,
            created_at: d.created_at,
            updated_at: d.updated_at,
        })
        .collect();

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&datasets).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&datasets).unwrap()),
        "table" => {
            if datasets.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No datasets found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = datasets
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
                let next = offset.unwrap_or(0) + body.count.max(0) as u32;
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
    let api = Api::new(Some(workspace_id));

    let resp: GetDatasetResponse =
        crate::sdk::block(api.client().datasets().get(dataset_id)).unwrap_or_else(|e| e.exit());

    let d = DatasetDetail {
        id: resp.id,
        label: resp.label,
        schema_name: resp.schema_name,
        table_name: resp.table_name,
        source_type: resp.source_type,
        created_at: resp.created_at,
        updated_at: resp.updated_at,
        columns: resp
            .columns
            .into_iter()
            .map(|c| Column {
                name: c.name,
                data_type: c.data_type,
                nullable: c.nullable,
            })
            .collect(),
    };

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
    description: Option<&str>,
    name: Option<&str>,
    format: &str,
) {
    if description.is_none() && name.is_none() {
        eprintln!("error: provide at least one of --description or --name.");
        std::process::exit(1);
    }

    let api = Api::new(Some(workspace_id));

    let mut request = UpdateDatasetRequest::new();
    if let Some(d) = description {
        request.label = Some(Some(d.to_string()));
    }
    if let Some(n) = name {
        request.table_name = Some(Some(n.to_string()));
    }

    let resp: UpdateDatasetResponse =
        crate::sdk::block(api.client().datasets().update(dataset_id, request))
            .unwrap_or_else(|e| e.exit());
    let d = UpdateView::from(resp);

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

    let api = Api::new(Some(workspace_id));

    let mut request = RefreshRequest::new();
    request.dataset_id = Some(Some(dataset_id.to_string()));
    if async_mode {
        request.r#async = Some(true);
    }

    let resp =
        crate::sdk::block(api.client().refresh().refresh(request)).unwrap_or_else(|e| e.exit());

    if async_mode {
        let job_id = match &resp {
            RefreshResponse::SubmitJobResponse(j) => j.id.clone(),
            _ => "unknown".to_string(),
        };
        println!("{}", "Dataset refresh submitted.".green());
        println!("job_id: {}", job_id);
        println!(
            "{}",
            format!("Use 'hotdata jobs {}' to check status.", job_id).dark_grey()
        );
        return;
    }

    let (id, version, dataset_status) = match &resp {
        RefreshResponse::RefreshDatasetResponse(r) => {
            (r.id.clone(), r.version as i64, r.status.clone())
        }
        RefreshResponse::SubmitJobResponse(j) => (j.id.clone(), 0, j.status.to_string()),
        _ => ("unknown".to_string(), 0, String::new()),
    };
    println!("{}", "Dataset refresh completed.".green());
    println!(
        "{}",
        format!("  id: {id}, version: {version}, status: {dataset_status}").dark_grey()
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use hotdata::models::UpdateDatasetResponse;

    /// Mirrors runtimedb's `UpdateDatasetResponse` (see runtimedb/src/http/models.rs).
    /// The SDK deserializes this exact shape; here we assert the CLI's `UpdateView`
    /// conversion preserves the display contract: no synthesized schema_name, and
    /// latest/pinned versions surfaced when present.
    #[test]
    fn update_view_from_runtimedb_payload() {
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "url_test",
            "table_name": "url_test",
            "latest_version": 3,
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateDatasetResponse = serde_json::from_value(body).unwrap();
        let view = UpdateView::from(resp);
        assert_eq!(view.id, "ds_abc123");
        assert_eq!(view.label, "url_test");
        assert_eq!(view.table_name, "url_test");
        // The server doesn't send schema_name and we never synthesize "main",
        // so sandbox-scoped datasets aren't mislabeled.
        assert!(view.schema_name.is_none());
        assert_eq!(view.latest_version, Some(3));
        assert!(view.pinned_version.is_none());
    }

    #[test]
    fn update_view_handles_pinned_version() {
        let body = serde_json::json!({
            "id": "ds_abc123",
            "label": "x",
            "table_name": "x",
            "latest_version": 5,
            "pinned_version": 2,
            "updated_at": "2026-04-28T18:30:00Z",
        });
        let resp: UpdateDatasetResponse = serde_json::from_value(body).unwrap();
        let view = UpdateView::from(resp);
        assert_eq!(view.pinned_version, Some(2));
    }

    #[test]
    fn create_from_query_builds_sql_source() {
        let source = DatasetSource::DatasetSourceOneOf2(Box::new(DatasetSourceOneOf2::new(
            "SELECT 1".to_string(),
            hotdata::models::dataset_source_one_of_2::Type::SqlQuery,
        )));
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["type"], "sql_query");
        assert_eq!(json["sql"], "SELECT 1");
    }

    #[test]
    fn create_from_saved_query_builds_saved_query_source() {
        let source = DatasetSource::DatasetSourceOneOf1(Box::new(DatasetSourceOneOf1::new(
            "sq_123".to_string(),
            hotdata::models::dataset_source_one_of_1::Type::SavedQuery,
        )));
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json["type"], "saved_query");
        assert_eq!(json["saved_query_id"], "sq_123");
    }
}
