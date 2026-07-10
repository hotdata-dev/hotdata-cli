use crate::client::sdk::{Api, block_with_wakeup};
use hotdata::models::TableInfo;
use serde::Serialize;

/// Subcommands for `hotdata tables`.
#[derive(clap::Subcommand)]
pub enum TablesCommands {
    /// Show column definitions for a specific table (catalog.schema.table or schema.table)
    Show {
        /// Table as catalog.schema.table (or schema.table when a database is active)
        table: String,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },

    /// List tables in the active database, or all workspace tables if none is set
    List {
        /// Workspace ID (defaults to first workspace from login)
        #[arg(long, short = 'w')]
        workspace_id: Option<String>,

        /// Filter by schema name (supports % wildcards)
        #[arg(long)]
        schema: Option<String>,

        /// Filter by table name (supports % wildcards)
        #[arg(long)]
        table: Option<String>,

        /// Maximum number of results to return
        #[arg(long)]
        limit: Option<u32>,

        /// Pagination cursor from a previous response
        #[arg(long)]
        cursor: Option<String>,

        /// Output format
        #[arg(long = "output", short = 'o', default_value = "table", value_parser = ["table", "json", "yaml"])]
        output: String,
    },
}

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
    schema: Option<&str>,
    table_filter: Option<&str>,
    limit: Option<u32>,
    cursor: Option<&str>,
    format: &str,
) {
    let api = Api::new(Some(workspace_id));

    let body = block_with_wakeup(
        &api,
        "Loading tables…",
        api.client().information_schema().get(
            None,
            schema,
            table_filter,
            None,
            limit.map(|l| l as i32),
            cursor,
        ),
    )
    .unwrap_or_else(|e| e.exit());

    let has_more = body.has_more;
    let next_cursor = body.next_cursor.flatten();

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
                crate::output::table::print(&["TABLE", "SYNCED", "LAST_SYNC"], &rows);
            }
        }
        _ => unreachable!(),
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

pub fn show(workspace_id: &str, table_ref: &str, format: &str) {
    let api = Api::new(Some(workspace_id));

    // Accept "schema.table" (active database) or "catalog.schema.table".
    let parts: Vec<&str> = table_ref.splitn(3, '.').collect();
    let (connection_id, schema, table_name) = match parts.as_slice() {
        [schema, table] => {
            // Two-part: resolve active database's connection.
            let db_id = crate::config::load_current_database("default", workspace_id)
                .unwrap_or_else(|| {
                    use crossterm::style::Stylize;
                    eprintln!(
                        "{}",
                        "error: use catalog.schema.table, or set an active database with \
                         `hotdata databases set <id>`."
                            .red()
                    );
                    std::process::exit(1);
                });
            let db = crate::commands::databases::get_database(&api, &db_id)
                .unwrap_or_else(|e| e.exit());
            (db.default_connection_id, schema.to_string(), table.to_string())
        }
        [catalog, schema, table] => {
            // Three-part: resolve the catalog/name as a database or connection.
            let conn_id =
                crate::commands::connections::resolve_connection_id(&api, catalog);
            (conn_id, schema.to_string(), table.to_string())
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
