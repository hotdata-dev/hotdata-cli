use crate::config;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Deserialize;
use serde_json::json;
use std::path::Path;

#[derive(Deserialize)]
struct Dataset {
    id: String,
    label: String,
    table_name: String,
}

struct FileType {
    content_type: &'static str,
    format: &'static str,
}

fn detect_from_bytes(bytes: &[u8]) -> FileType {
    if bytes.starts_with(b"PAR1") {
        return FileType { content_type: "application/octet-stream", format: "parquet" };
    }
    let first = bytes.iter().find(|&&b| !b.is_ascii_whitespace()).copied();
    if matches!(first, Some(b'{') | Some(b'[')) {
        return FileType { content_type: "application/json", format: "json" };
    }
    FileType { content_type: "text/csv", format: "csv" }
}

fn detect_from_path(path: &str) -> Option<FileType> {
    match Path::new(path).extension().and_then(|e| e.to_str()) {
        Some("csv") => Some(FileType { content_type: "text/csv", format: "csv" }),
        Some("json") => Some(FileType { content_type: "application/json", format: "json" }),
        Some("parquet") => Some(FileType { content_type: "application/octet-stream", format: "parquet" }),
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
        use std::os::unix::io::AsRawFd;
        use nix::fcntl::{fcntl, FcntlArg};
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

fn api_error(body: String) -> String {
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or(body)
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
    client: &reqwest::blocking::Client,
    api_key: &str,
    workspace_id: &str,
    api_url: &str,
    content_type: &str,
    reader: R,
    pb: ProgressBar,
) -> String {
    let url = format!("{api_url}/files");

    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .header("Content-Type", content_type)
        .body(reqwest::blocking::Body::new(reader))
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            pb.finish_and_clear();
            eprintln!("error uploading: {e}");
            std::process::exit(1);
        }
    };

    pb.finish_and_clear();

    if !resp.status().is_success() {
        use crossterm::style::Stylize;
        eprintln!("{}", api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let body: serde_json::Value = match resp.json() {
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
fn upload_from_file(
    client: &reqwest::blocking::Client,
    api_key: &str,
    workspace_id: &str,
    api_url: &str,
    path: &str,
) -> (String, &'static str) {
    let f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("error opening file '{path}': {e}");
            std::process::exit(1);
        }
    };

    let ft = detect_from_path(path).unwrap_or_else(|| {
        use std::io::Read;
        let mut probe = [0u8; 512];
        let n = match std::fs::File::open(path) {
            Ok(mut f2) => f2.read(&mut probe).unwrap_or(0),
            Err(_) => 0,
        };
        detect_from_bytes(&probe[..n])
    });

    let file_size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let pb = make_progress_bar(file_size);
    let reader = pb.wrap_read(f);

    let id = do_upload(client, api_key, workspace_id, api_url, ft.content_type, reader, pb);
    (id, ft.format)
}

// Returns (upload_id, format)
fn upload_from_stdin(
    client: &reqwest::blocking::Client,
    api_key: &str,
    workspace_id: &str,
    api_url: &str,
) -> (String, &'static str) {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
        eprintln!("error reading stdin: {e}");
        std::process::exit(1);
    }

    let ft = detect_from_bytes(&buf);
    let total = buf.len() as u64;
    let pb = make_progress_bar(total);
    let reader = pb.wrap_read(std::io::Cursor::new(buf));

    let id = do_upload(client, api_key, workspace_id, api_url, ft.content_type, reader, pb);
    (id, ft.format)
}

pub fn create(
    workspace_id: &str,
    label: Option<&str>,
    table_name: Option<&str>,
    file: Option<&str>,
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
            eprintln!("error: not authenticated. Run 'hotdata auth login' to log in.");
            std::process::exit(1);
        }
    };

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
            None => match stdin_redirect_filename() {
                Some(name) => {
                    label_derived = name;
                    &label_derived
                }
                None => {
                    eprintln!("error: no label provided. Use --label to name the dataset.");
                    std::process::exit(1);
                }
            },
        },
    };

    let client = reqwest::blocking::Client::new();

    let (upload_id, format) = match file {
        Some(path) => upload_from_file(&client, &api_key, workspace_id, &profile_config.api_url, path),
        None => {
            use std::io::IsTerminal;
            if std::io::stdin().is_terminal() {
                eprintln!("error: no input data. Use --file <path> or pipe data via stdin.");
                std::process::exit(1);
            }
            upload_from_stdin(&client, &api_key, workspace_id, &profile_config.api_url)
        }
    };

    let source = json!({ "upload_id": upload_id, "format": format });
    let mut body = json!({ "label": label, "source": source });
    if let Some(tn) = table_name {
        body["table_name"] = json!(tn);
    }

    let url = format!("{}/datasets", profile_config.api_url);

    let resp = match client
        .post(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("X-Workspace-Id", workspace_id)
        .json(&body)
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
        eprintln!("{}", api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let dataset: Dataset = match resp.json() {
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
    println!("table_name: {}", dataset.table_name);
}
