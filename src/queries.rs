use crate::api::ApiClient;
use crossterm::style::{Color, Stylize};
use serde::{Deserialize, Serialize};

const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "AS", "ON", "JOIN", "LEFT",
    "RIGHT", "INNER", "OUTER", "FULL", "CROSS", "ORDER", "BY", "GROUP", "HAVING", "LIMIT",
    "OFFSET", "UNION", "ALL", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE",
    "DROP", "ALTER", "TABLE", "INDEX", "VIEW", "WITH", "DISTINCT", "BETWEEN", "LIKE", "CASE",
    "WHEN", "THEN", "ELSE", "END", "EXISTS", "ASC", "DESC", "TRUE", "FALSE", "COUNT", "SUM", "AVG",
    "MIN", "MAX", "CAST", "COALESCE", "NULLIF",
];

fn highlight_sql(sql: &str) -> String {
    let mut result = String::with_capacity(sql.len() * 2);
    let chars: Vec<char> = sql.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let ch = chars[i];

        // Single-line comment
        if ch == '-' && i + 1 < len && chars[i + 1] == '-' {
            let start = i;
            while i < len && chars[i] != '\n' {
                i += 1;
            }
            let comment: String = chars[start..i].iter().collect();
            result.push_str(&comment.dark_grey().to_string());
            continue;
        }

        // Block comment
        if ch == '/' && i + 1 < len && chars[i + 1] == '*' {
            let start = i;
            i += 2;
            while i + 1 < len && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            if i + 1 < len {
                i += 2;
            }
            let comment: String = chars[start..i].iter().collect();
            result.push_str(&comment.dark_grey().to_string());
            continue;
        }

        // String literal (handles '' escaped quotes in SQL)
        if ch == '\'' {
            let start = i;
            i += 1;
            loop {
                if i >= len {
                    break;
                }
                if chars[i] == '\'' {
                    i += 1;
                    // '' is an escaped quote, continue the string
                    if i < len && chars[i] == '\'' {
                        i += 1;
                    } else {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
            let s: String = chars[start..i].iter().collect();
            result.push_str(&s.yellow().to_string());
            continue;
        }

        // Number
        if ch.is_ascii_digit() || (ch == '.' && i + 1 < len && chars[i + 1].is_ascii_digit()) {
            let start = i;
            while i < len && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            // Don't highlight if it's part of an identifier
            if start > 0 && (chars[start - 1].is_alphanumeric() || chars[start - 1] == '_') {
                let s: String = chars[start..i].iter().collect();
                result.push_str(&s);
            } else {
                let s: String = chars[start..i].iter().collect();
                result.push_str(&s.cyan().to_string());
            }
            continue;
        }

        // Word (keyword or identifier)
        if ch.is_alphanumeric() || ch == '_' {
            let start = i;
            while i < len && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            if SQL_KEYWORDS.contains(&word.to_uppercase().as_str()) {
                result.push_str(&word.blue().to_string());
            } else {
                result.push_str(&word);
            }
            continue;
        }

        result.push(ch);
        i += 1;
    }

    result
}

#[derive(Deserialize, Serialize)]
struct QueryRun {
    id: String,
    status: String,
    created_at: String,
    completed_at: Option<String>,
    execution_time_ms: Option<u64>,
    server_processing_ms: Option<u64>,
    row_count: Option<u64>,
    saved_query_id: Option<String>,
    saved_query_version: Option<u64>,
    snapshot_id: String,
    sql_hash: String,
    sql_text: String,
    result_id: Option<String>,
    error_message: Option<String>,
    warning_message: Option<String>,
    trace_id: Option<String>,
    user_public_id: Option<String>,
}

#[derive(Deserialize)]
struct ListResponse {
    query_runs: Vec<QueryRun>,
    count: u64,
    has_more: bool,
    next_cursor: Option<String>,
}

fn color_status(status: &str) -> String {
    let color = match status {
        "succeeded" => Color::Green,
        "failed" => Color::Red,
        "running" | "queued" | "pending" => Color::Yellow,
        _ => Color::Reset,
    };
    status.with(color).to_string()
}

fn truncate_sql(sql: &str, max: usize) -> String {
    let flat = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let prefix: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{prefix}…")
    }
}

pub fn list(
    workspace_id: &str,
    limit: Option<u32>,
    cursor: Option<&str>,
    status: Option<&str>,
    format: &str,
) {
    let api = ApiClient::new(Some(workspace_id));

    let params = [
        ("limit", limit.map(|l| l.to_string())),
        ("cursor", cursor.map(str::to_string)),
        ("status", status.map(str::to_string)),
    ];
    let body: ListResponse = api.get_with_params("/query-runs", &params);

    match format {
        "json" => println!(
            "{}",
            serde_json::to_string_pretty(&body.query_runs).unwrap()
        ),
        "yaml" => print!("{}", serde_yaml::to_string(&body.query_runs).unwrap()),
        "table" => {
            if body.query_runs.is_empty() {
                eprintln!("{}", "No query runs found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body
                    .query_runs
                    .iter()
                    .map(|r| {
                        vec![
                            r.id.clone(),
                            color_status(&r.status),
                            crate::util::format_date(&r.created_at),
                            r.execution_time_ms
                                .map(|ms| ms.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            r.row_count
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                            truncate_sql(&r.sql_text, 60),
                        ]
                    })
                    .collect();
                crate::table::print(
                    &["ID", "STATUS", "CREATED", "DURATION_MS", "ROWS", "SQL"],
                    &rows,
                );
            }
            if body.has_more {
                let next = body.next_cursor.as_deref().unwrap_or("");
                eprintln!(
                    "{}",
                    format!(
                        "showing {} results — use --cursor {next} for more",
                        body.count
                    )
                    .dark_grey()
                );
            }
        }
        _ => unreachable!(),
    }
}

pub fn get(query_run_id: &str, workspace_id: &str, format: &str) {
    let api = ApiClient::new(Some(workspace_id));
    let path = format!("/query-runs/{query_run_id}");
    let run: QueryRun = api.get(&path);
    print_detail(&run, format);
}

fn print_detail(r: &QueryRun, format: &str) {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(r).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(r).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<14}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), r.id);
            println!("{}{}", label("status:"), color_status(&r.status));
            println!(
                "{}{}",
                label("created:"),
                crate::util::format_date(&r.created_at)
            );
            if let Some(ref c) = r.completed_at {
                println!("{}{}", label("completed:"), crate::util::format_date(c));
            }
            if let Some(ms) = r.execution_time_ms {
                println!("{}{} ms", label("duration:"), ms);
            }
            if let Some(ms) = r.server_processing_ms {
                println!("{}{} ms", label("server time:"), ms);
            }
            if let Some(n) = r.row_count {
                println!("{}{}", label("rows:"), n);
            }
            if let Some(ref id) = r.result_id {
                println!("{}{}", label("result id:"), id);
            }
            if let Some(ref id) = r.saved_query_id {
                let version = r
                    .saved_query_version
                    .map(|v| format!(" (v{v})"))
                    .unwrap_or_default();
                println!("{}{}{}", label("saved query:"), id, version);
            }
            println!("{}{}", label("snapshot:"), r.snapshot_id);
            println!("{}{}", label("sql hash:"), r.sql_hash);
            if let Some(ref id) = r.trace_id {
                println!("{}{}", label("trace:"), id);
            }
            if let Some(ref id) = r.user_public_id {
                println!("{}{}", label("user:"), id);
            }
            if let Some(ref msg) = r.warning_message {
                println!("{}{}", label("warning:"), msg.as_str().yellow());
            }
            if let Some(ref msg) = r.error_message {
                println!("{}{}", label("error:"), msg.as_str().red());
            }
            println!();
            println!("{}", "SQL:".dark_grey());
            let formatted = sqlformat::format(
                &r.sql_text,
                &sqlformat::QueryParams::None,
                &sqlformat::FormatOptions {
                    indent: sqlformat::Indent::Spaces(2),
                    uppercase: Some(true),
                    lines_between_queries: 1,
                    ..Default::default()
                },
            );
            println!("{}", highlight_sql(&formatted));
        }
        _ => unreachable!(),
    }
}
