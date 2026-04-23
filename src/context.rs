//! Workspace context: `/v1/context` sync with `./{NAME}.md` in the current directory.

use crate::api::ApiClient;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::LazyLock;

/// Matches runtimedb `MAX_TABLE_NAME_LENGTH` / `validate_table_name` rules for context keys.
pub const MAX_CONTEXT_NAME_LEN: usize = 128;

/// Matches runtimedb workspace context content cap.
pub const MAX_CONTEXT_CONTENT_CHARS: usize = 512_000;

static RESERVED_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "select", "from", "where", "insert", "update", "delete", "create", "drop", "alter",
        "table", "index", "view", "and", "or", "not", "null", "true", "false", "in", "is", "like",
        "between", "join", "on", "as", "order", "by", "group", "having", "limit", "offset",
        "union", "all", "distinct", "case", "when", "then", "else", "end", "exists", "any", "some",
    ]
    .into_iter()
    .collect()
});

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceContextEntry {
    name: String,
    content: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    contexts: Vec<WorkspaceContextEntry>,
}

#[derive(Deserialize)]
struct GetResponse {
    context: WorkspaceContextEntry,
}

#[derive(Deserialize)]
struct UpsertResponse {
    context: WorkspaceContextEntry,
}

/// Validates a context stem (API `name` and basename before `.md`).
/// Same rules as runtimedb `validate_table_name`.
pub fn validate_context_stem(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    if name.len() > MAX_CONTEXT_NAME_LEN {
        return Err(format!(
            "name exceeds maximum length of {} (got {})",
            MAX_CONTEXT_NAME_LEN,
            name.len()
        ));
    }

    let mut chars = name.chars();
    if let Some(first) = chars.next() {
        if !first.is_ascii_alphabetic() && first != '_' {
            return Err(format!(
                "name must start with a letter or underscore, got '{first}'"
            ));
        }
    }

    for c in chars {
        if !c.is_ascii_alphanumeric() && c != '_' {
            return Err(format!("name contains invalid character '{c}'"));
        }
    }

    if RESERVED_WORDS.contains(name.to_lowercase().as_str()) {
        return Err(format!(
            "'{name}' is a SQL reserved word and cannot be used as a context name"
        ));
    }

    Ok(())
}

fn local_md_path(name: &str) -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|e| {
            eprintln!("error: could not read current directory: {e}");
            std::process::exit(1);
        })
        .join(format!("{name}.md"))
}

fn fetch_context(api: &ApiClient, name: &str) -> Result<WorkspaceContextEntry, reqwest::StatusCode> {
    let path = format!("/context/{name}");
    let (status, body) = api.get_raw(&path);
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(status);
    }
    if !status.is_success() {
        eprintln!("{}", format!("error: HTTP {status}").red());
        eprintln!("{body}");
        std::process::exit(1);
    }
    let parsed: GetResponse = serde_json::from_str(&body).unwrap_or_else(|e| {
        eprintln!("error parsing response: {e}");
        std::process::exit(1);
    });
    Ok(parsed.context)
}

pub fn list(workspace_id: &str, prefix: Option<&str>, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let body: ListResponse = api.get("/context");

    let mut rows: Vec<WorkspaceContextEntry> = body.contexts;
    if let Some(p) = prefix {
        rows.retain(|c| c.name.starts_with(p));
    }

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&rows).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&rows).unwrap()),
        "table" => {
            if rows.is_empty() {
                eprintln!("{}", "No contexts found.".dark_grey());
            } else {
                let table_rows: Vec<Vec<String>> = rows
                    .iter()
                    .map(|c| {
                        vec![
                            c.name.clone(),
                            crate::util::format_date(&c.updated_at),
                            c.content.chars().count().to_string(),
                        ]
                    })
                    .collect();
                crate::table::print(&["NAME", "UPDATED", "CHARS"], &table_rows);
            }
        }
        _ => unreachable!(),
    }
}

pub fn show(workspace_id: &str, name: &str) {
    if let Err(e) = validate_context_stem(name) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));
    match fetch_context(&api, name) {
        Ok(ctx) => {
            print!("{}", ctx.content);
            if !ctx.content.ends_with('\n') {
                println!();
            }
        }
        Err(reqwest::StatusCode::NOT_FOUND) => {
            eprintln!(
                "{}",
                format!("error: no context named '{name}' in this workspace.").red()
            );
            eprintln!(
                "{}",
                format!("Create ./{name}.md locally, then run: hotdata context push {name}")
                    .dark_grey()
            );
            std::process::exit(1);
        }
        Err(status) => panic!("unexpected error status from fetch_context: {status}"),
    }
}

pub fn pull(workspace_id: &str, name: &str, force: bool, dry_run: bool) {
    if let Err(e) = validate_context_stem(name) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    let path = local_md_path(name);

    if !dry_run && !force && path.exists() {
        eprintln!(
            "{}",
            format!("error: {} already exists (use --force to overwrite)", path.display()).red()
        );
        std::process::exit(1);
    }

    let api = ApiClient::new(Some(workspace_id));
    let ctx = match fetch_context(&api, name) {
        Ok(c) => c,
        Err(reqwest::StatusCode::NOT_FOUND) => {
            eprintln!(
                "{}",
                format!("error: no context named '{name}' in this workspace.").red()
            );
            std::process::exit(1);
        }
        Err(status) => panic!("unexpected error status from fetch_context: {status}"),
    };

    let n_chars = ctx.content.chars().count();
    if dry_run {
        eprintln!(
            "{}",
            format!("would write {} chars to {}", n_chars, path.display()).dark_grey()
        );
        return;
    }

    let mut f = fs::File::create(&path).unwrap_or_else(|e| {
        eprintln!("error: could not create {}: {e}", path.display());
        std::process::exit(1);
    });
    if let Err(e) = f.write_all(ctx.content.as_bytes()) {
        eprintln!("error: could not write {}: {e}", path.display());
        std::process::exit(1);
    }

    println!(
        "{}",
        format!("wrote {} (updated {})", path.display(), crate::util::format_date(&ctx.updated_at))
            .green()
    );
}

pub fn push(workspace_id: &str, name: &str, dry_run: bool) {
    if let Err(e) = validate_context_stem(name) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }

    let path = local_md_path(name);
    if !path.is_file() {
        eprintln!(
            "{}",
            format!("error: {} does not exist or is not a file", path.display()).red()
        );
        std::process::exit(1);
    }

    let content = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("error: could not read {}: {e}", path.display());
        std::process::exit(1);
    });

    let n_chars = content.chars().count();
    if n_chars > MAX_CONTEXT_CONTENT_CHARS {
        eprintln!(
            "error: file is {} characters; maximum allowed is {}",
            n_chars, MAX_CONTEXT_CONTENT_CHARS
        );
        std::process::exit(1);
    }

    if dry_run {
        eprintln!(
            "{}",
            format!("would POST {} characters as context '{name}'", n_chars).dark_grey()
        );
        return;
    }

    let api = ApiClient::new(Some(workspace_id));
    let body = json!({ "name": name, "content": content });
    let resp: UpsertResponse = api.post("/context", &body);

    println!(
        "{}",
        format!(
            "pushed '{}' (updated {})",
            name,
            crate::util::format_date(&resp.context.updated_at)
        )
        .green()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_datamodel() {
        validate_context_stem("DATAMODEL").unwrap();
    }

    #[test]
    fn validate_rejects_reserved() {
        assert!(validate_context_stem("select").is_err());
    }

    #[test]
    fn validate_rejects_dot() {
        assert!(validate_context_stem("foo.md").is_err());
    }

    #[test]
    fn validate_rejects_leading_digit() {
        assert!(validate_context_stem("1abc").is_err());
    }

    #[test]
    fn validate_accepts_leading_underscore() {
        validate_context_stem("_private").unwrap();
    }

    #[test]
    fn validate_accepts_max_length() {
        let s = format!("a{}", "b".repeat(127));
        assert_eq!(s.len(), 128);
        validate_context_stem(&s).unwrap();
    }

    #[test]
    fn validate_rejects_too_long() {
        let s = format!("a{}", "b".repeat(128));
        assert_eq!(s.len(), 129);
        assert!(validate_context_stem(&s).is_err());
    }

    #[test]
    fn validate_rejects_reserved_uppercase() {
        assert!(validate_context_stem("SELECT").is_err());
    }
}
