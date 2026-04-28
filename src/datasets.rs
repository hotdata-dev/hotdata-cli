use crate::api::ApiClient;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::Path;

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

struct FileType {
    content_type: &'static str,
    format: &'static str,
}

fn detect_from_bytes(bytes: &[u8]) -> FileType {
    if bytes.starts_with(b"PAR1") {
        return FileType {
            content_type: "application/octet-stream",
            format: "parquet",
        };
    }
    let first = bytes.iter().find(|&&b| !b.is_ascii_whitespace()).copied();
    if matches!(first, Some(b'{') | Some(b'[')) {
        return FileType {
            content_type: "application/json",
            format: "json",
        };
    }
    FileType {
        content_type: "text/csv",
        format: "csv",
    }
}

fn detect_from_path(path: &str) -> Option<FileType> {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("csv") => Some(FileType {
            content_type: "text/csv",
            format: "csv",
        }),
        Some("json") => Some(FileType {
            content_type: "application/json",
            format: "json",
        }),
        Some("parquet") => Some(FileType {
            content_type: "application/octet-stream",
            format: "parquet",
        }),
        _ => None,
    }
}

/// Try to resolve the filename of the file redirected into stdin.
/// Works for `cmd < file.csv` but not for pipes (`cat file.csv | cmd`).
fn stdin_redirect_filename() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_link("/proc/self/fd/0")
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
    }
    #[cfg(target_os = "macos")]
    {
        use nix::fcntl::{FcntlArg, fcntl};
        use std::os::unix::io::AsRawFd;
        let fd = std::io::stdin().as_raw_fd();
        let mut path = std::path::PathBuf::new();
        match fcntl(fd, FcntlArg::F_GETPATH(&mut path)) {
            Ok(_) => path.file_stem().map(|s| s.to_string_lossy().into_owned()),
            Err(_) => None,
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn make_progress_bar(total: u64) -> ProgressBar {
    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("=>-"),
    );
    pb
}

fn do_upload<R: std::io::Read + Send + 'static>(
    api: &ApiClient,
    content_type: &str,
    reader: R,
    pb: ProgressBar,
    content_length: Option<u64>,
) -> String {
    let (status, resp_body) = api.post_body("/files", content_type, reader, content_length);

    pb.finish_and_clear();

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        std::process::exit(1);
    }

    let body: serde_json::Value = match serde_json::from_str(&resp_body) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing upload response: {e}");
            std::process::exit(1);
        }
    };

    match body["id"].as_str() {
        Some(id) => id.to_string(),
        None => {
            eprintln!("error: upload response missing id");
            std::process::exit(1);
        }
    }
}

// Returns (upload_id, format)
fn upload_from_file(api: &ApiClient, path: &str) -> (String, &'static str) {
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error opening file '{path}': {e}");
            std::process::exit(1);
        }
    };

    let ft = detect_from_path(path).unwrap_or_else(|| {
        use std::io::{Read, Seek};
        let mut probe = [0u8; 512];
        let n = f.read(&mut probe).unwrap_or(0);
        let _ = f.seek(std::io::SeekFrom::Start(0));
        detect_from_bytes(&probe[..n])
    });

    let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let pb = make_progress_bar(file_size);
    let reader = pb.wrap_read(f);

    let id = do_upload(api, ft.content_type, reader, pb, Some(file_size));
    (id, ft.format)
}

// Returns (upload_id, format)
fn upload_from_stdin(api: &ApiClient) -> (String, &'static str) {
    use std::io::Read;
    let mut probe = [0u8; 512];
    let n = std::io::stdin().read(&mut probe).unwrap_or(0);
    let ft = detect_from_bytes(&probe[..n]);

    let reader = std::io::Cursor::new(probe[..n].to_vec()).chain(std::io::stdin());

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {bytes} uploaded ({elapsed})").unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    let reader = pb.wrap_read(reader);

    let id = do_upload(api, ft.content_type, reader, pb, None);
    (id, ft.format)
}

fn create_dataset(
    api: &ApiClient,
    label: &str,
    table_name: Option<&str>,
    source: serde_json::Value,
    on_failure: Option<Box<dyn FnOnce()>>,
) {
    let mut body = json!({ "label": label, "source": source });
    if let Some(tn) = table_name {
        body["table_name"] = json!(tn);
    }

    let (status, resp_body) = api.post_raw("/datasets", &body);

    if !status.is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", crate::util::api_error(resp_body).red());
        if let Some(f) = on_failure {
            f();
        }
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

pub fn create_from_upload(
    workspace_id: &str,
    label: Option<&str>,
    table_name: Option<&str>,
    file: Option<&str>,
    upload_id: Option<&str>,
    source_format: &str,
) {
    let api = ApiClient::new(Some(workspace_id));

    let label_derived;
    let label: &str = match label {
        Some(l) => l,
        None => match file {
            Some(path) => {
                label_derived = Path::new(path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("dataset")
                    .to_string();
                &label_derived
            }
            None => {
                if upload_id.is_some() {
                    eprintln!("error: no label provided. Use --label to name the dataset.");
                    std::process::exit(1);
                }
                match stdin_redirect_filename() {
                    Some(name) => {
                        label_derived = name;
                        &label_derived
                    }
                    None => {
                        eprintln!("error: no label provided. Use --label to name the dataset.");
                        std::process::exit(1);
                    }
                }
            }
        },
    };

    let (upload_id, format, upload_id_was_uploaded): (String, &str, bool) = if let Some(id) =
        upload_id
    {
        (id.to_string(), source_format, false)
    } else {
        let (id, fmt) = match file {
            Some(path) => upload_from_file(&api, path),
            None => {
                use std::io::IsTerminal;
                if std::io::stdin().is_terminal() {
                    eprintln!(
                        "error: no input data. Use --file <path>, --upload-id <id>, or pipe data via stdin."
                    );
                    std::process::exit(1);
                }
                upload_from_stdin(&api)
            }
        };
        (id, fmt, true)
    };

    let source = json!({ "upload_id": upload_id, "format": format });

    let on_failure: Option<Box<dyn FnOnce()>> = if upload_id_was_uploaded {
        let uid = upload_id.clone();
        Some(Box::new(move || {
            use crossterm::style::Stylize;
            eprintln!(
                "{}",
                format!(
                    "Resume dataset creation without re-uploading by passing --upload-id {uid}"
                )
                .yellow()
            );
        }))
    } else {
        None
    };

    create_dataset(&api, label, table_name, source, on_failure);
}

pub fn create_from_url(
    workspace_id: &str,
    url: &str,
    label: Option<&str>,
    table_name: Option<&str>,
) {
    let label = match label {
        Some(l) => l,
        None => {
            eprintln!("error: --label is required when using --url");
            std::process::exit(1);
        }
    };
    let api = ApiClient::new(Some(workspace_id));
    create_dataset(&api, label, table_name, json!({ "url": url }), None);
}

pub fn create_from_query(
    workspace_id: &str,
    sql: &str,
    label: Option<&str>,
    table_name: Option<&str>,
) {
    let label = match label {
        Some(l) => l,
        None => {
            eprintln!("error: --label is required when using --sql");
            std::process::exit(1);
        }
    };
    let api = ApiClient::new(Some(workspace_id));
    create_dataset(&api, label, table_name, json!({ "sql": sql }), None);
}

pub fn create_from_saved_query(
    workspace_id: &str,
    query_id: &str,
    label: Option<&str>,
    table_name: Option<&str>,
) {
    let label = match label {
        Some(l) => l,
        None => {
            eprintln!("error: --label is required when using --query-id");
            std::process::exit(1);
        }
    };
    let api = ApiClient::new(Some(workspace_id));
    create_dataset(
        &api,
        label,
        table_name,
        json!({ "saved_query_id": query_id }),
        None,
    );
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
        eprintln!("error: provide at least one of --label or --table-name.");
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
                    use crossterm::style::Stylize;
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
