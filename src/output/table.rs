use tabled::settings::{
    Color, Modify, Style,
    object::{Columns, Rows, Segment},
    style::BorderColor,
    width::Width,
};

/// Truncate arrays to first 3 + last 3 when over 6 elements.
/// Returns (formatted_values, total_count) where total_count is Some when truncated.
pub fn truncate_array(arr: &[serde_json::Value]) -> (String, Option<usize>) {
    if arr.len() > 6 {
        let head: Vec<String> = arr[..3].iter().map(|v| v.to_string()).collect();
        let tail: Vec<String> = arr[arr.len() - 3..].iter().map(|v| v.to_string()).collect();
        (
            format!("[{}, ..., {}]", head.join(", "), tail.join(", ")),
            Some(arr.len()),
        )
    } else {
        (
            format!(
                "[{}]",
                arr.iter()
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None,
        )
    }
}

/// Format an array for styled table output.
fn format_array(arr: &[serde_json::Value]) -> String {
    use crossterm::style::Stylize;
    let (formatted, count) = truncate_array(arr);
    match count {
        Some(n) => format!("{formatted} {}", format!("({n} items)").dark_grey()),
        None => formatted,
    }
}

fn term_width() -> usize {
    crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(120)
}

/// The narrowest a column can be and still read as a value. Below this a cell
/// stops being shorter and starts being taller: nine characters of state in a
/// two-character column is five rows of table saying "co/mp/le/te/d".
///
/// Sized to a formatted timestamp — "2026-08-13 15:37" — the narrowest *whole*
/// value these tables render, since a column that cannot show one wraps every
/// date it holds. A column whose content is naturally shorter is never held to
/// it; see [`needs`].
const MIN_COL_WIDTH: usize = 16;

/// What a column must be given to be worth showing: what it naturally wants,
/// or [`MIN_COL_WIDTH`] when it wants more than that.
fn needs(natural: usize) -> usize {
    natural.min(MIN_COL_WIDTH)
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

/// Printable width of a cell. The escape sequences that colour a status take
/// no space on screen, so counting their bytes is how a coloured column comes
/// to ask for twice the width it needs.
fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            width += 1;
            continue;
        }
        // CSI: ESC '[' then parameter bytes, then one final byte in @..~.
        // Consume the '[' before scanning, since '[' is itself in that range.
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    width
}

/// What each column would take if nothing were competing: the widest cell,
/// with the header allowed to widen it by up to 3 characters so a narrow
/// column still shows its name.
fn natural_widths(headers: &[impl AsRef<str>], rows: &[Vec<String>], ncols: usize) -> Vec<usize> {
    (0..ncols)
        .map(|i| {
            let content_w = rows
                .iter()
                .filter_map(|r| r.get(i))
                .map(|s| display_width(s))
                .max()
                .unwrap_or(1);
            let header_w = headers
                .get(i)
                .map(|h| display_width(h.as_ref()))
                .unwrap_or(0);
            content_w.max(header_w.min(content_w + 3))
        })
        .collect()
}

/// Column widths for a table of `ncols` columns at terminal width `tw`.
///
/// ID columns are kept whole whenever the rest of the table still clears
/// [`MIN_COL_WIDTH`] without them: an id is one opaque token and the only
/// handle the row can be looked up by, and a token wrapped across cell lines
/// cannot be copied in one selection. The exemption is conditional because
/// unconditionally honouring it is how a table spends two thirds of a terminal
/// on ids and leaves the columns a reader came for two characters each — so
/// when the ids cannot be afforded, they wrap like everything else and
/// [`print`] drops columns instead.
fn column_widths(
    headers: &[impl AsRef<str>],
    rows: &[Vec<String>],
    ncols: usize,
    tw: usize,
) -> Vec<usize> {
    if ncols == 0 {
        return vec![];
    }
    // Per column: a border, a leading pad and a trailing pad; plus the closing
    // border of the row.
    let available = tw.saturating_sub(ncols * 3 + 1);
    let natural = natural_widths(headers, rows, ncols);
    let id_cols = id_column_indices(&headers[..ncols]);

    let mut widths = vec![0usize; ncols];
    let mut unsettled: Vec<usize> = (0..ncols).collect();
    let mut remaining = available;

    let ids_whole: usize = id_cols.iter().map(|&i| natural[i]).sum();
    let others: usize = (0..ncols)
        .filter(|i| !id_cols.contains(i))
        .map(|i| needs(natural[i]))
        .sum();
    if available >= ids_whole + others {
        for &i in &id_cols {
            widths[i] = natural[i];
        }
        remaining -= ids_whole;
        unsettled.retain(|i| !id_cols.contains(i));
    }

    distribute(&natural, &mut unsettled, remaining, &mut widths);
    for w in &mut widths {
        *w = (*w).max(1);
    }
    widths
}

/// Hand `remaining` out among `unsettled`: a column that fits inside an equal
/// share takes only what it needs, and what it did not take is re-shared among
/// the columns that are still short. Columns that all exceed the share split
/// it evenly, which is the point where nothing more can be done for them.
fn distribute(
    natural: &[usize],
    unsettled: &mut Vec<usize>,
    mut remaining: usize,
    widths: &mut [usize],
) {
    while !unsettled.is_empty() {
        let share = remaining / unsettled.len();
        let mut settled = vec![];
        let mut used = 0;
        for &i in unsettled.iter() {
            if natural[i] <= share {
                widths[i] = natural[i];
                used += natural[i];
                settled.push(i);
            }
        }
        if settled.is_empty() {
            for &i in unsettled.iter() {
                widths[i] = share;
            }
            return;
        }
        remaining -= used;
        unsettled.retain(|i| !settled.contains(i));
    }
}

/// How many leading columns can be rendered at `tw` without starving any of
/// them or wrapping an id.
///
/// Answered by asking the allocator, so "does it fit" cannot drift from what
/// gets drawn. Callers therefore order columns by importance: the ones a
/// reader can do without go on the right, because that is the end a narrow
/// terminal takes them from.
fn visible_columns(headers: &[impl AsRef<str>], rows: &[Vec<String>], tw: usize) -> usize {
    let ncols = headers.len();
    (1..=ncols)
        .rev()
        .find(|&k| {
            let widths = column_widths(headers, rows, k, tw);
            let natural = natural_widths(headers, rows, k);
            id_column_indices(&headers[..k])
                .iter()
                .all(|&i| widths[i] == natural[i])
                && widths.iter().zip(&natural).all(|(&w, &n)| w >= needs(n))
        })
        // Nothing fits cleanly — draw the first column anyway and let it wrap,
        // rather than printing an empty table at a 20-column terminal.
        .unwrap_or(1)
}

/// Print a table with string data. Headers are &str slices, rows are Vec<String>.
///
/// Columns are given in order of importance; see [`visible_columns`].
pub fn print(headers: &[&str], rows: &[Vec<String>]) {
    let tw = term_width();
    let shown = visible_columns(headers, rows, tw);
    let widths = column_widths(headers, rows, shown, tw);

    let mut builder = tabled::builder::Builder::new();
    builder.push_record(headers[..shown].iter().map(|h| h.to_string()));
    for row in rows {
        builder.push_record(row.iter().take(shown).map(|c| c.to_string()));
    }
    let mut table = builder.build();

    table.with(Style::modern_rounded());
    for (i, &w) in widths.iter().enumerate() {
        table.with(Modify::new(Columns::new(i..=i)).with(Width::wrap(w).keep_words(true)));
    }
    table
        .with(Modify::new(Segment::all()).with(BorderColor::filled(Color::FG_BRIGHT_BLACK)))
        .with(Modify::new(Rows::first()).with(Color::FG_GREEN));

    println!("{table}");
    if shown < headers.len() {
        // On stderr, like every other note this module's callers print: a
        // `-o table` pipeline must see the table and nothing else.
        use crossterm::style::Stylize;
        eprintln!(
            "{}",
            format!(
                "{} more column{} at a wider terminal: {}. All of them: -o json.",
                headers.len() - shown,
                if headers.len() - shown == 1 { "" } else { "s" },
                headers[shown..].join(", ")
            )
            .dark_grey()
        );
    }
}

/// Print a table with JSON-typed data. Numbers, bools, and nulls get per-cell coloring.
/// Uses fair column width distribution (for user-generated query results).
pub fn print_json(headers: &[String], rows: &[Vec<serde_json::Value>]) {
    use tabled::settings::object::Cell;

    let tw = term_width();
    let ncols = headers.len();

    let mut builder = tabled::builder::Builder::new();
    builder.push_record(headers.iter().map(|h| h.to_string()));

    // Track cells that need coloring: (row_index, col_index, color)
    let mut colored_cells: Vec<(usize, usize, Color)> = Vec::new();

    let mut string_rows: Vec<Vec<String>> = Vec::with_capacity(rows.len());

    for (ri, row) in rows.iter().enumerate() {
        let string_row: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(ci, v)| match v {
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
                serde_json::Value::Array(arr) => format_array(arr),
                _ => v
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| v.to_string()),
            })
            .collect();
        builder.push_record(&string_row);
        string_rows.push(string_row);
    }

    // Every column is kept: these are the caller's own query results, and a
    // column missing from a result set is a different answer, not a narrower
    // one. Widths are shared out by need all the same.
    let col_widths = column_widths(headers, &string_rows, ncols, tw);

    let mut table = builder.build();
    table.with(Style::modern_rounded());

    for (i, &w) in col_widths.iter().enumerate() {
        table.with(Modify::new(Columns::new(i..=i)).with(Width::wrap(w)));
    }

    table
        .with(Modify::new(Segment::all()).with(BorderColor::filled(Color::FG_BRIGHT_BLACK)))
        .with(Modify::new(Rows::first()).with(Color::FG_GREEN));

    for (r, c, color) in colored_cells {
        table.with(Modify::new(Cell::new(r, c)).with(color));
    }

    println!("{table}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `hotdata ingest list` table, which is the one that went unreadable:
    /// a 30-character id per row and seven attributes competing for what is
    /// left of an 80-column terminal.
    const INGEST_LIST: &[&str] = &[
        "INGEST ID",
        "TYPE",
        "STATE",
        "DESTINATION",
        "SCHEDULE",
        "READS",
        "CREATED",
        "DATASOURCE ID",
    ];

    fn ingest_rows() -> Vec<Vec<String>> {
        vec![
            vec![
                "ing_01KZXX8S84FARPZZPAP8MJXHD5".into(),
                "continuous".into(),
                "\u{1b}[38;5;3mactive\u{1b}[39m".into(),
                "db_e2e_local.public.s1_events".into(),
                "every 1m (next 2026-08-13 18:00)".into(),
                "evt/**/*.parquet".into(),
                "2026-08-13 15:53".into(),
                "ds_01KZXX802KXD93YGR6586D5PJX".into(),
            ],
            vec![
                "ing_01KZXWBRW65H2Z84WYT566XT3Z".into(),
                "one_time".into(),
                "\u{1b}[38;5;10mcompleted\u{1b}[39m".into(),
                "db_e2e_local.public.orders".into(),
                "-".into(),
                "orders/**/*.parquet".into(),
                "2026-08-13 15:37".into(),
                "ds_01KZXWAXH88NB3NEW7PKR35RPJ".into(),
            ],
        ]
    }

    /// What the drawn table will measure: every column plus its borders and
    /// padding.
    fn drawn_width(widths: &[usize]) -> usize {
        widths.iter().map(|w| w + 3).sum::<usize>() + 1
    }

    #[test]
    fn colour_codes_do_not_count_toward_a_column_width() {
        assert_eq!(display_width("\u{1b}[38;5;10mcompleted\u{1b}[39m"), 9);
        assert_eq!(display_width("completed"), 9);
        // A cell the CLI truncated is one character per glyph, not per byte.
        assert_eq!(display_width("orders…"), 7);
    }

    #[test]
    fn a_narrow_terminal_drops_columns_rather_than_starving_them() {
        let rows = ingest_rows();
        // 80 columns cannot hold a 30-character id and seven more values, so
        // the ones on the right are given up — the alternative, seen in the
        // field, is eight columns of two characters each.
        let shown = visible_columns(INGEST_LIST, &rows, 80);
        assert!(
            (2..INGEST_LIST.len()).contains(&shown),
            "expected some but not all columns at 80, got {shown}"
        );
        // Width earns columns back, up to all of them.
        assert!(visible_columns(INGEST_LIST, &rows, 120) > shown);
        assert_eq!(visible_columns(INGEST_LIST, &rows, 200), INGEST_LIST.len());
    }

    #[test]
    fn every_shown_column_is_wide_enough_to_read_and_the_table_fits() {
        let rows = ingest_rows();
        for tw in [80, 100, 120, 160, 200] {
            let shown = visible_columns(INGEST_LIST, &rows, tw);
            let widths = column_widths(INGEST_LIST, &rows, shown, tw);
            let natural = natural_widths(INGEST_LIST, &rows, shown);
            assert!(
                drawn_width(&widths) <= tw,
                "table of {widths:?} overflows {tw} columns"
            );
            for (i, (&w, &n)) in widths.iter().zip(&natural).enumerate() {
                assert!(
                    w >= needs(n),
                    "column {} got {w} at width {tw}, needs {}",
                    INGEST_LIST[i],
                    needs(n)
                );
            }
        }
    }

    #[test]
    fn an_id_keeps_its_full_width_so_it_can_be_copied() {
        let rows = ingest_rows();
        for tw in [80, 120, 200] {
            let shown = visible_columns(INGEST_LIST, &rows, tw);
            let widths = column_widths(INGEST_LIST, &rows, shown, tw);
            let natural = natural_widths(INGEST_LIST, &rows, shown);
            for i in id_column_indices(&INGEST_LIST[..shown]) {
                assert!(natural[i] >= 29, "the fixture's ids stopped being ids");
                assert_eq!(
                    widths[i], natural[i],
                    "{} wrapped at width {tw}",
                    INGEST_LIST[i]
                );
            }
        }
    }

    #[test]
    fn width_goes_to_the_columns_that_have_something_to_show() {
        // The failure this replaces: everything that was not an id split the
        // remainder equally, so a six-character state and a forty-character
        // selector were given the same two columns.
        let rows = ingest_rows();
        let widths = column_widths(INGEST_LIST, &rows, 6, 160);
        let natural = natural_widths(INGEST_LIST, &rows, 6);
        // A column that fits inside its share takes only what it needs …
        assert_eq!(widths[1], natural[1], "TYPE asked for more than it holds");
        assert_eq!(widths[2], natural[2], "STATE asked for more than it holds");
        // … and what it left over goes to the ones still short.
        assert!(
            widths[4] > widths[1],
            "SCHEDULE ({}) should outgrow TYPE ({})",
            widths[4],
            widths[1]
        );
    }
}
