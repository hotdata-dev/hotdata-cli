use crate::api::ApiClient;
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
    let api = ApiClient::new(Some(workspace_id));

    let mut params: Vec<(&str, Option<String>)> = Vec::new();
    if let Some(id) = connection_id {
        params.push(("connection_id", Some(id.to_string())));
        params.push(("include_columns", Some("true".to_string())));
    }
    if let Some(s) = schema {
        params.push(("schema", Some(s.to_string())));
    }
    if let Some(t) = table_filter {
        params.push(("table", Some(t.to_string())));
    }
    if let Some(l) = limit {
        params.push(("limit", Some(l.to_string())));
    }
    if let Some(c) = cursor {
        params.push(("cursor", Some(c.to_string())));
    }

    let body: ListResponse = api.get_with_params("/information_schema", &params);

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
                if out.is_empty() {
                    use crossterm::style::Stylize;
                    eprintln!("{}", "No tables found.".dark_grey());
                } else {
                    let rows: Vec<Vec<String>> = out.iter().flat_map(|t| {
                        t.columns.iter().map(|col| vec![
                            t.table.clone(), col.name.clone(), col.data_type.clone(), col.nullable.to_string(),
                        ])
                    }).collect();
                    crate::table::print(&["TABLE", "COLUMN", "DATA_TYPE", "NULLABLE"], &rows);
                }
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
                if out.is_empty() {
                    use crossterm::style::Stylize;
                    eprintln!("{}", "No tables found.".dark_grey());
                } else {
                    let rows: Vec<Vec<String>> = out.iter().map(|r| vec![
                        r.table.clone(),
                        r.synced.to_string(),
                        r.last_sync.as_deref().map(crate::util::format_date).unwrap_or_else(|| "-".to_string()),
                    ]).collect();
                    crate::table::print(&["TABLE", "SYNCED", "LAST_SYNC"], &rows);
                }
            }
            _ => unreachable!(),
        }
    }

    if has_more {
        use crossterm::style::Stylize;
        eprintln!("{}", format!("More results available. Use --cursor {} to fetch the next page.", next_cursor.as_deref().unwrap_or("")).dark_grey());
    }
}
