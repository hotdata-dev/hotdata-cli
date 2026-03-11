use crate::config;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Deserialize)]
struct Table {
    connection: String,
    schema: String,
    table: String,
    synced: bool,
    last_sync: Option<String>,
    #[serde(default)]
    columns: Vec<Column>,
}

impl Table {
    fn full_name(&self) -> String {
        format!("{}.{}.{}", self.connection, self.schema, self.table)
    }
}

#[derive(Deserialize)]
struct ListResponse {
    tables: Vec<Table>,
    has_more: bool,
    next_cursor: Option<String>,
}

#[derive(Serialize)]
struct TableRow {
    table: String,
    synced: bool,
    last_sync: Option<String>,
}

#[derive(Serialize)]
struct TableWithColumns {
    table: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    columns: Vec<Column>,
}

pub fn list(
    workspace_id: &str,
    connection_id: Option<&str>,
    schema: Option<&str>,
    table_filter: Option<&str>,
    limit: Option<u32>,
    cursor: Option<&str>,
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
            eprintln!("error: not authenticated. Run 'hotdata auth login' to log in.");
            std::process::exit(1);
        }
    };

    let mut params: Vec<String> = Vec::new();
    if let Some(id) = connection_id {
        params.push(format!("connection_id={id}"));
        params.push("include_columns=true".to_string());
    }
    if let Some(s) = schema {
        params.push(format!("schema={s}"));
    }
    if let Some(t) = table_filter {
        params.push(format!("table={t}"));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(c) = cursor {
        params.push(format!("cursor={c}"));
    }

    let mut url = format!("{}/information_schema", profile_config.api_url);
    if !params.is_empty() {
        url.push_str(&format!("?{}", params.join("&")));
    }

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
        eprintln!("error: HTTP {}", resp.status());
        std::process::exit(1);
    }

    let body: ListResponse = match resp.json() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("error parsing response: {e}");
            std::process::exit(1);
        }
    };

    let has_more = body.has_more;
    let next_cursor = body.next_cursor.clone();

    if connection_id.is_some() {
        let out: Vec<TableWithColumns> = body.tables.into_iter()
            .map(|t| TableWithColumns { table: t.full_name(), columns: t.columns })
            .collect();
        match format {
            "json" => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
            "yaml" => print!("{}", serde_yaml::to_string(&out).unwrap()),
            "table" => {
                let mut table = crate::util::make_table();
                table.set_header(["TABLE", "COLUMN", "DATA_TYPE", "NULLABLE"]);
                for t in &out {
                    for col in &t.columns {
                        table.add_row([&t.table, &col.name, &col.data_type, &col.nullable.to_string()]);
                    }
                }
                println!("{table}");
            }
            _ => unreachable!(),
        }
    } else {
        let mut out: Vec<TableRow> = body.tables.iter()
            .map(|t| TableRow { table: t.full_name(), synced: t.synced, last_sync: t.last_sync.clone() })
            .collect();
        out.sort_by(|a, b| a.table.cmp(&b.table));
        match format {
            "json" => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
            "yaml" => print!("{}", serde_yaml::to_string(&out).unwrap()),
            "table" => {
                let mut table = crate::util::make_table();
                table.set_header(["TABLE", "SYNCED", "LAST_SYNC"]);
                for r in &out {
                    table.add_row([&r.table, &r.synced.to_string(), r.last_sync.as_deref().unwrap_or("-")]);
                }
                println!("{table}");
            }
            _ => unreachable!(),
        }
    }

    if has_more {
        use crossterm::style::Stylize;
        eprintln!("{}", format!("More results available. Use --cursor {} to fetch the next page.", next_cursor.as_deref().unwrap_or("")).dark_grey());
    }
}
