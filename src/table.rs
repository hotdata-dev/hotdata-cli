use tabled::settings::{
    Color, Modify, Style,
    object::{Columns, Rows, Segment},
    style::BorderColor,
    width::Width,
};

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
}

/// Find column indices whose header ends with "ID" (case-insensitive).
fn id_column_indices(headers: &[impl AsRef<str>]) -> Vec<usize> {
    headers
        .iter()
        .enumerate()
        .filter(|(_, h)| {
            let h = h.as_ref().to_ascii_uppercase();
            h == "ID" || h.ends_with("_ID") || h.ends_with(" ID")
        })
        .map(|(i, _)| i)
        .collect()
}

/// Get the width of a column from the first row.
fn first_row_width(rows: &[Vec<String>], col: usize) -> usize {
    rows.first()
        .and_then(|r| r.get(col))
        .map(|s| s.len())
        .unwrap_or(0)
}

fn style_table(table: &mut tabled::Table, num_cols: usize, id_col_indices: &[usize], id_widths: &[usize]) {
    let tw = term_width();

    // Calculate how much space ID columns need (content + 3 for cell padding/borders)
    let id_total: usize = id_widths.iter().map(|w| w + 3).sum();
    // Borders: 1 for left edge + 3 per separator between columns + 1 for right edge => but simpler:
    // Each column takes width + 3 (padding + border), plus 1 for the final border
    let non_id_count = num_cols - id_col_indices.len();
    let overhead = 1; // final border character
    let remaining = tw.saturating_sub(id_total + overhead);
    let non_id_width = if non_id_count > 0 { remaining / non_id_count } else { 0 };

    table.with(Style::modern_rounded());

    // Wrap only non-ID columns to fit; leave ID columns at full width
    for col in 0..num_cols {
        if id_col_indices.contains(&col) {
            continue;
        }
        table.with(Modify::new(Columns::new(col..=col)).with(Width::wrap(non_id_width).keep_words(true)));
    }

    table
        .with(Modify::new(Segment::all()).with(BorderColor::filled(Color::FG_BRIGHT_BLACK)))
        .with(Modify::new(Rows::first()).with(Color::FG_GREEN));
}

/// Print a table with string data. Headers are &str slices, rows are Vec<String>.
pub fn print(headers: &[&str], rows: &[Vec<String>]) {
    let id_cols = id_column_indices(headers);
    let id_widths: Vec<usize> = id_cols.iter().map(|&i| first_row_width(rows, i)).collect();

    let mut builder = tabled::builder::Builder::new();
    builder.push_record(headers.iter().map(|h| h.to_string()));
    for row in rows {
        builder.push_record(row.iter().map(|c| c.to_string()));
    }
    let mut table = builder.build();
    style_table(&mut table, headers.len(), &id_cols, &id_widths);
    println!("{table}");
}

/// Print a table with JSON-typed data. Numbers, bools, and nulls get per-cell coloring.
/// Uses simple word-wrapping without ID column priority (for user-generated query results).
pub fn print_json(headers: &[String], rows: &[Vec<serde_json::Value>]) {
    use tabled::settings::object::Cell;

    let tw = term_width();

    let mut builder = tabled::builder::Builder::new();
    builder.push_record(headers.iter().map(|h| h.to_string()));

    // Track cells that need coloring: (row_index, col_index, color)
    let mut colored_cells: Vec<(usize, usize, Color)> = Vec::new();

    for (ri, row) in rows.iter().enumerate() {
        let string_row: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(ci, v)| {
                match v {
                    serde_json::Value::Number(n) => {
                        colored_cells.push((ri + 1, ci, Color::FG_CYAN));
                        n.to_string()
                    }
                    serde_json::Value::Null => {
                        colored_cells.push((ri + 1, ci, Color::FG_BRIGHT_BLACK));
                        String::new()
                    }
                    serde_json::Value::Bool(b) => {
                        colored_cells.push((ri + 1, ci, Color::FG_YELLOW));
                        b.to_string()
                    }
                    _ => v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()),
                }
            })
            .collect();
        builder.push_record(string_row);
    }

    let mut table = builder.build();
    table
        .with(Style::modern_rounded())
        .with(Width::wrap(tw).keep_words(true))
        .with(Modify::new(Segment::all()).with(BorderColor::filled(Color::FG_BRIGHT_BLACK)))
        .with(Modify::new(Rows::first()).with(Color::FG_GREEN));

    for (r, c, color) in colored_cells {
        table.with(Modify::new(Cell::new(r, c)).with(color));
    }

    println!("{table}");
}
