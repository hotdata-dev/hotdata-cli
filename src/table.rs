use tabled::settings::{
    Color, Modify, Style,
    object::{Rows, Segment},
    style::BorderColor,
    width::Width,
};

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
}

fn style_table(table: &mut tabled::Table, _ncols: usize) {
    let tw = term_width();

    table
        .with(Style::modern_rounded())
        .with(Width::wrap(tw).keep_words(true))
        .with(Modify::new(Segment::all()).with(BorderColor::filled(Color::FG_BRIGHT_BLACK)))
        .with(Modify::new(Rows::first()).with(Color::FG_GREEN));
}

/// Print a table with string data. Headers are &str slices, rows are Vec<String>.
pub fn print(headers: &[&str], rows: &[Vec<String>]) {
    let ncols = headers.len();
    let mut builder = tabled::builder::Builder::new();
    builder.push_record(headers.iter().map(|h| h.to_string()));
    for row in rows {
        builder.push_record(row.iter().map(|c| c.to_string()));
    }
    let mut table = builder.build();
    style_table(&mut table, ncols);
    println!("{table}");
}

/// Print a table with JSON-typed data. Numbers, bools, and nulls get per-cell coloring.
pub fn print_json(headers: &[String], rows: &[Vec<serde_json::Value>]) {
    use tabled::settings::object::Cell;

    let ncols = headers.len();
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
    style_table(&mut table, ncols);

    for (r, c, color) in colored_cells {
        table.with(Modify::new(Cell::new(r, c)).with(color));
    }

    println!("{table}");
}
