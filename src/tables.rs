use crate::sdk::Api;
use hotdata::models::TableInfo;
use serde::Serialize;

#[derive(Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

fn full_name(t: &TableInfo) -> String {
    format!("{}.{}.{}", t.connection, t.schema, t.table)
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
    let api = Api::new(Some(workspace_id));

    // Request columns only when a connection is specified
    // (include_columns=true iff connection_id is set).
    let include_columns = connection_id.map(|_| true);

    let body = crate::sdk::block(api.client().information_schema().get(
        connection_id,
        schema,
        table_filter,
        include_columns,
        limit.map(|l| l as i32),
        cursor,
    ))
    .unwrap_or_else(|e| e.exit());

    let has_more = body.has_more;
    let next_cursor = body.next_cursor.flatten();

    if connection_id.is_some() {
        let out: Vec<TableWithColumns> = body
            .tables
            .into_iter()
            .map(|t| TableWithColumns {
                table: full_name(&t),
                columns: t
                    .columns
                    .flatten()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|c| Column {
                        name: c.name,
                        data_type: c.data_type,
                        nullable: c.nullable,
                    })
                    .collect(),
            })
            .collect();
        match format {
            "json" => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
            "yaml" => print!("{}", serde_yaml::to_string(&out).unwrap()),
            "table" => {
                if out.is_empty() {
                    use crossterm::style::Stylize;
                    eprintln!("{}", "No tables found.".dark_grey());
                } else {
                    let rows: Vec<Vec<String>> = out
                        .iter()
                        .flat_map(|t| {
                            t.columns.iter().map(|col| {
                                vec![
                                    t.table.clone(),
                                    col.name.clone(),
                                    col.data_type.clone(),
                                    col.nullable.to_string(),
                                ]
                            })
                        })
                        .collect();
                    crate::table::print(&["TABLE", "COLUMN", "DATA_TYPE", "NULLABLE"], &rows);
                }
            }
            _ => unreachable!(),
        }
    } else {
        let mut out: Vec<TableRow> = body
            .tables
            .iter()
            .map(|t| TableRow {
                table: full_name(t),
                synced: t.synced,
                last_sync: t.last_sync.clone().flatten(),
            })
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
                    let rows: Vec<Vec<String>> = out
                        .iter()
                        .map(|r| {
                            vec![
                                r.table.clone(),
                                r.synced.to_string(),
                                r.last_sync
                                    .as_deref()
                                    .map(crate::util::format_date)
                                    .unwrap_or_else(|| "-".to_string()),
                            ]
                        })
                        .collect();
                    crate::table::print(&["TABLE", "SYNCED", "LAST_SYNC"], &rows);
                }
            }
            _ => unreachable!(),
        }
    }

    if has_more {
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!(
                "More results available. Use --cursor {} to fetch the next page.",
                next_cursor.as_deref().unwrap_or("")
            )
            .dark_grey()
        );
    }
}
