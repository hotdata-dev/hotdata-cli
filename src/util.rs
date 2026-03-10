pub fn make_table() -> comfy_table::Table {
    let mut table = comfy_table::Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL_CONDENSED);
    if let Ok((width, _)) = crossterm::terminal::size() {
        table.set_width(width);
    }
    table
}
