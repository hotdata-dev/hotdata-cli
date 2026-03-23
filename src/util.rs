/// Returns a dark-grey styled header cell.
pub fn hcell(label: &str) -> comfy_table::Cell {
    comfy_table::Cell::new(label).fg(comfy_table::Color::Green)
}

/// Returns a cell colored for a numeric value.
pub fn num_cell(value: impl std::fmt::Display) -> comfy_table::Cell {
    comfy_table::Cell::new(value.to_string()).fg(comfy_table::Color::DarkCyan)
}

/// Returns a cell styled based on a serde_json::Value type (numbers get colored).
pub fn json_cell(v: &serde_json::Value) -> comfy_table::Cell {
    match v {
        serde_json::Value::Number(n) => num_cell(n),
        serde_json::Value::Null => comfy_table::Cell::new("").fg(comfy_table::Color::DarkGrey),
        serde_json::Value::Bool(b) => comfy_table::Cell::new(b.to_string()).fg(comfy_table::Color::DarkYellow),
        _ => {
            let s = v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string());
            comfy_table::Cell::new(s)
        }
    }
}

/// Format an ISO date string compactly: "2024-03-15 14:23" (no seconds, no timezone).
pub fn format_date(s: &str) -> String {
    let s = s.split('.').next().unwrap_or(s).trim_end_matches('Z');
    let s = s.replace('T', " ");
    s.chars().take(16).collect()
}

pub fn api_error(body: String) -> String {
    serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .unwrap_or(body)
}

pub fn make_table() -> comfy_table::Table {
    use comfy_table::TableComponent::*;
    let mut table = comfy_table::Table::new();
    // Start from the condensed preset (solid vertical lines, no double-line header)
    table.load_preset(comfy_table::presets::UTF8_FULL_CONDENSED);
    // Replace double-line header separator with single-line
    table.set_style(LeftHeaderIntersection, '├');
    table.set_style(HeaderLines, '─');
    table.set_style(MiddleHeaderIntersections, '┼');
    table.set_style(RightHeaderIntersection, '┤');
    // Replace dashed inner vertical (┆) with solid vertical (│)
    table.set_style(VerticalLines, '│');
    // Add row dividers between data rows
    table.set_style(HorizontalLines, '─');
    table.set_style(MiddleIntersections, '┼');
    table.set_style(LeftBorderIntersections, '├');
    table.set_style(RightBorderIntersections, '┤');
    // Rounded corners
    table.set_style(TopLeftCorner, '╭');
    table.set_style(TopRightCorner, '╮');
    table.set_style(BottomLeftCorner, '╰');
    table.set_style(BottomRightCorner, '╯');
    table.set_content_arrangement(comfy_table::ContentArrangement::Dynamic);
    if let Ok((width, _)) = crossterm::terminal::size() {
        table.set_width(width);
    }
    table
}

/// Print a table with all border characters rendered in dark grey.
pub fn print_table(table: &comfy_table::Table) {
    use crossterm::style::Stylize;
    let dark_pipe = "│".dark_grey().to_string();
    for line in table.to_string().lines() {
        let first = line.chars().next();
        if matches!(first, Some('─' | '═' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼' | '╭' | '╮' | '╰' | '╯')) {
            println!("{}", line.dark_grey());
        } else {
            println!("{}", line.replace('│', &dark_pipe));
        }
    }
}

/// Call after `set_header` to prevent any column from wrapping to a second line.
/// Distributes available width equally across all columns using UpperBoundary percentage constraints.
pub fn no_wrap(table: &mut comfy_table::Table) {
    let n = table.column_iter().count();
    if n == 0 { return; }
    let pct = (100u16 / n as u16).max(1);
    let constraints: Vec<_> = (0..n)
        .map(|_| comfy_table::ColumnConstraint::UpperBoundary(comfy_table::Width::Percentage(pct)))
        .collect();
    table.set_constraints(constraints);
}
