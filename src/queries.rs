use crate::config;
use crossterm::style::Stylize;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "FROM", "WHERE", "AND", "OR", "NOT", "IN", "IS", "NULL", "AS",
    "ON", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "FULL", "CROSS",
    "ORDER", "BY", "GROUP", "HAVING", "LIMIT", "OFFSET", "UNION", "ALL",
    "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "CREATE", "DROP",
    "ALTER", "TABLE", "INDEX", "VIEW", "WITH", "DISTINCT", "BETWEEN", "LIKE",
    "CASE", "WHEN", "THEN", "ELSE", "END", "EXISTS", "ASC", "DESC", "TRUE", "FALSE",
    "COUNT", "SUM", "AVG", "MIN", "MAX", "CAST", "COALESCE", "NULLIF",
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
            if i + 1 < len { i += 2; }
            let comment: String = chars[start..i].iter().collect();
            result.push_str(&comment.dark_grey().to_string());
            continue;
        }

        // String literal
        if ch == '\'' {
            let start = i;
            i += 1;
            while i < len && chars[i] != '\'' {
                if chars[i] == '\'' && i + 1 < len && chars[i + 1] == '\'' {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if i < len { i += 1; }
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
struct SavedQuery {
    id: String,
    name: String,
    description: String,
    tags: Vec<String>,
    latest_version: u64,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize, Serialize)]
struct SavedQueryDetail {
    id: String,
    name: String,
    description: String,
    sql: String,
    sql_hash: String,
    tags: Vec<String>,
    latest_version: u64,
    #[serde(default)]
    category: Value,
    #[serde(default)]
    has_aggregation: Value,
    #[serde(default)]
    has_group_by: Value,
    #[serde(default)]
    has_join: Value,
    #[serde(default)]
    has_limit: Value,
    #[serde(default)]
    has_order_by: Value,
    #[serde(default)]
    has_predicate: Value,
    #[serde(default)]
    num_tables: Value,
    #[serde(default)]
    table_size: Value,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct ListResponse {
    queries: Vec<SavedQuery>,
    count: u64,
    has_more: bool,
}

pub fn list(workspace_id: &str, limit: Option<u32>, offset: Option<u32>, format: &str) {
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

    let mut url = format!("{}/queries", profile_config.api_url);
    let mut params = vec![];
    if let Some(l) = limit { params.push(format!("limit={l}")); }
    if let Some(o) = offset { params.push(format!("offset={o}")); }
    if !params.is_empty() { url = format!("{url}?{}", params.join("&")); }

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

    let body: ListResponse = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&body.queries).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&body.queries).unwrap()),
        "table" => {
            if body.queries.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No saved queries found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = body.queries.iter().map(|q| vec![
                    q.id.clone(),
                    q.name.clone(),
                    q.description.clone(),
                    q.tags.join(", "),
                    q.latest_version.to_string(),
                    crate::util::format_date(&q.updated_at),
                ]).collect();
                crate::table::print(&["ID", "NAME", "DESCRIPTION", "TAGS", "VERSION", "UPDATED"], &rows);
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

pub fn get(query_id: &str, workspace_id: &str, format: &str) {
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

    let url = format!("{}/queries/{query_id}", profile_config.api_url);
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

    let q: SavedQueryDetail = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    print_detail(&q, format);
}

fn print_detail(q: &SavedQueryDetail, format: &str) {
    match format {
        "json" => println!("{}", serde_json::to_string_pretty(q).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(q).unwrap()),
        "table" => {
            let label = |l: &str| format!("{:<12}", l).dark_grey().to_string();
            println!("{}{}", label("id:"), q.id);
            println!("{}{}", label("name:"), q.name);
            println!("{}{}", label("description:"), q.description);
            println!("{}{}", label("version:"), q.latest_version);
            if !q.tags.is_empty() {
                println!("{}{}", label("tags:"), q.tags.join(", "));
            }
            println!("{}{}", label("created:"), crate::util::format_date(&q.created_at));
            println!("{}{}", label("updated:"), crate::util::format_date(&q.updated_at));
            println!();
            println!("{}", "SQL:".dark_grey());
            let formatted = sqlformat::format(
                &q.sql,
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

fn parse_tags(tags: Option<&str>) -> Option<Vec<&str>> {
    tags.map(|t| t.split(',').map(str::trim).collect())
}

pub fn create(
    workspace_id: &str,
    name: &str,
    sql: &str,
    description: Option<&str>,
    tags: Option<&str>,
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

    let mut body = serde_json::json!({ "name": name, "sql": sql });
    if let Some(d) = description { body["description"] = serde_json::json!(d); }
    if let Some(tags) = parse_tags(tags) { body["tags"] = serde_json::json!(tags); }

    let url = format!("{}/queries", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

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
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let q: SavedQueryDetail = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    println!("{}", "Query created".green());
    print_detail(&q, format);
}

pub fn update(
    workspace_id: &str,
    id: &str,
    name: Option<&str>,
    sql: Option<&str>,
    description: Option<&str>,
    tags: Option<&str>,
    category: Option<&str>,
    table_size: Option<&str>,
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

    let mut body = serde_json::json!({});
    if let Some(n) = name { body["name"] = serde_json::json!(n); }
    if let Some(s) = sql { body["sql"] = serde_json::json!(s); }
    if let Some(d) = description { body["description"] = serde_json::json!(d); }
    if let Some(tags) = parse_tags(tags) { body["tags"] = serde_json::json!(tags); }
    match category {
        Some("") => { body["category_override"] = serde_json::json!(null); }
        Some(c) => { body["category_override"] = serde_json::json!(c); }
        None => {}
    }
    match table_size {
        Some("") => { body["table_size_override"] = serde_json::json!(null); }
        Some(ts) => { body["table_size_override"] = serde_json::json!(ts); }
        None => {}
    }

    let url = format!("{}/queries/{id}", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .put(&url)
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
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let q: SavedQueryDetail = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    println!("{}", "Query updated".green());
    print_detail(&q, format);
}

pub fn run(query_id: &str, workspace_id: &str, format: &str) {
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

    let url = format!("{}/queries/{query_id}/execute", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    let resp = match client
        .post(&url)
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
        eprintln!("{}", crate::util::api_error(resp.text().unwrap_or_default()).red());
        std::process::exit(1);
    }

    let result: crate::query::QueryResponse = match resp.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    crate::query::print_result(&result, format);
}
