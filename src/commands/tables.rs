use crate::client::sdk::{Api, block_with_wakeup};
use serde::Serialize;

#[derive(Serialize)]
struct Column {
    name: String,
    data_type: String,
    nullable: bool,
}

#[derive(Serialize)]
struct TableWithColumns {
    table: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    columns: Vec<Column>,
}

pub fn show(workspace_id: &str, table_ref: &str, format: &str) {
    let api = Api::new(Some(workspace_id));

    // Accept "schema.table" (active database) or "catalog.schema.table".
    let parts: Vec<&str> = table_ref.splitn(3, '.').collect();
    let (connection_id, display_catalog, schema, table_name) = match parts.as_slice() {
        [schema, table] => {
            // Two-part: resolve active database's connection.
            let db_id = crate::config::load_current_database("default", workspace_id)
                .unwrap_or_else(|| {
                    use crossterm::style::Stylize;
                    eprintln!(
                        "{}",
                        "error: use catalog.schema.table, or set an active database with \
                         `hotdata databases use <id>`."
                            .red()
                    );
                    std::process::exit(1);
                });
            let db =
                crate::commands::databases::get_database(&api, &db_id).unwrap_or_else(|e| e.exit());
            let catalog = db
                .default_catalog
                .unwrap_or_else(|| db.name.unwrap_or_else(|| "default".to_string()));
            (
                db.default_connection_id,
                catalog,
                schema.to_string(),
                table.to_string(),
            )
        }
        [catalog, schema, table] => {
            // Three-part: resolve the catalog/name as a database or connection.
            let conn_id = crate::commands::connections::resolve_connection_id(&api, catalog);
            (
                conn_id,
                catalog.to_string(),
                schema.to_string(),
                table.to_string(),
            )
        }
        _ => {
            use crossterm::style::Stylize;
            eprintln!(
                "{}",
                "error: table must be specified as schema.table or catalog.schema.table".red()
            );
            std::process::exit(1);
        }
    };

    let body = block_with_wakeup(
        &api,
        "Loading table…",
        api.client().information_schema().get(
            Some(&connection_id),
            Some(&schema),
            Some(&table_name),
            Some(true),
            None,
            None,
        ),
    )
    .unwrap_or_else(|e| e.exit());

    let t = body
        .tables
        .into_iter()
        .find(|t| t.table == table_name)
        .unwrap_or_else(|| {
            use crossterm::style::Stylize;
            eprintln!("{}", format!("Table '{table_ref}' not found.").red());
            std::process::exit(1);
        });

    let out = TableWithColumns {
        table: format!("{display_catalog}.{}.{}", t.schema, t.table),
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
    };

    match format {
        "json" => println!("{}", serde_json::to_string_pretty(&out).unwrap()),
        "yaml" => print!("{}", serde_yaml::to_string(&out).unwrap()),
        "table" => {
            if out.columns.is_empty() {
                use crossterm::style::Stylize;
                eprintln!("{}", "No columns found.".dark_grey());
            } else {
                let rows: Vec<Vec<String>> = out
                    .columns
                    .iter()
                    .map(|c| {
                        vec![
                            out.table.clone(),
                            c.name.clone(),
                            c.data_type.clone(),
                            c.nullable.to_string(),
                        ]
                    })
                    .collect();
                crate::output::table::print(&["TABLE", "COLUMN", "DATA_TYPE", "NULLABLE"], &rows);
            }
        }
        _ => unreachable!(),
    }
}
