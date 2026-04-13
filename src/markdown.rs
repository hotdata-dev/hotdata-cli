//! Minimal markdown-to-terminal renderer using crossterm styles.
//!
//! Line-level: headings (colored + bold by level), bullet/ordered list
//! markers (cyan), blockquotes (italic dim), fenced code blocks (dim cyan),
//! and horizontal rules (dim).
//!
//! Inline: backtick code (dim cyan), **bold** (bold), *em* / _em_ (italic).
//! Intentionally not a full CommonMark implementation — aims for legible,
//! not faithful.

use crossterm::style::Stylize;
use std::io::IsTerminal;

/// Decide whether the "markdown" format should actually emit ANSI styles.
/// "markdown" → always style; "plain" → never; "auto" → only when stdout is a TTY.
pub fn should_style(output: &str) -> bool {
    match output {
        "markdown" => true,
        "plain" => false,
        _ => std::io::stdout().is_terminal(),
    }
}

/// Render a markdown string to stdout with styling.
pub fn render(md: &str) {
    let mut in_fence = false;
    for line in md.lines() {
        let trimmed = line.trim_start();

        // Fenced code block boundaries
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            println!("{}", line.dark_grey());
            continue;
        }
        if in_fence {
            println!("{}", line.dark_cyan());
            continue;
        }

        // Heading
        if let Some((level, text)) = parse_heading(trimmed) {
            let indent = &line[..line.len() - trimmed.len()];
            println!("{indent}{}", style_heading(text, level));
            continue;
        }

        // Horizontal rule
        if is_hrule(trimmed) {
            println!("{}", trimmed.dark_grey());
            continue;
        }

        // Blockquote
        if trimmed == ">" || trimmed.starts_with("> ") {
            println!("{}", line.dark_grey().italic());
            continue;
        }

        // List marker (unordered `-/*/+` or ordered `N.`)
        if let Some((indent, marker, rest)) = parse_list_marker(line) {
            println!("{indent}{} {}", marker.cyan(), render_inline(rest));
            continue;
        }

        println!("{}", render_inline(line));
    }
}

/// Style a single heading line. Exposed so `sessions outline` can reuse it.
pub fn style_heading(text: &str, level: usize) -> String {
    match level {
        1 => text.bold().cyan().to_string(),
        2 => text.bold().green().to_string(),
        3 => text.bold().yellow().to_string(),
        4 => text.bold().magenta().to_string(),
        _ => text.bold().grey().to_string(),
    }
}

fn parse_heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None;
    }
    Some((level, rest.trim()))
}

fn is_hrule(s: &str) -> bool {
    let t = s.trim();
    let Some(first) = t.chars().next() else { return false };
    if !matches!(first, '-' | '_' | '*') {
        return false;
    }
    let count = t.chars().filter(|c| *c == first).count();
    count >= 3 && t.chars().all(|c| c == first || c.is_whitespace())
}

fn parse_list_marker(line: &str) -> Option<(&str, String, &str)> {
    let trimmed = line.trim_start();
    let indent_len = line.len() - trimmed.len();
    let indent = &line[..indent_len];

    // Unordered
    for m in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(m) {
            return Some((indent, m.trim_end().to_string(), rest));
        }
    }
    // Ordered: "N. "
    if let Some(dot) = trimmed.find(". ") {
        let before = &trimmed[..dot];
        if !before.is_empty() && before.chars().all(|c| c.is_ascii_digit()) {
            let rest = &trimmed[dot + 2..];
            return Some((indent, format!("{before}."), rest));
        }
    }
    None
}

/// Apply inline styles: `code`, **bold**, *em*/_em_. Single-pass, no nesting.
fn render_inline(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len() + 16);
    let mut i = 0;

    while i < bytes.len() {
        // `code`
        if bytes[i] == b'`' {
            if let Some(end_rel) = bytes[i + 1..].iter().position(|b| *b == b'`') {
                let content = &line[i + 1..i + 1 + end_rel];
                out.push_str(&content.dark_cyan().to_string());
                i += end_rel + 2;
                continue;
            }
        }

        // **bold**
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(end_rel) = find_seq(&bytes[i + 2..], b"**") {
                let content = &line[i + 2..i + 2 + end_rel];
                if !content.is_empty() {
                    out.push_str(&content.bold().to_string());
                    i += end_rel + 4;
                    continue;
                }
            }
        }

        // *em* or _em_
        if bytes[i] == b'*' || bytes[i] == b'_' {
            let d = bytes[i];
            // skip — ** is handled above; a single * here would have fallen through
            if d == b'_' || (d == b'*' && (i + 1 >= bytes.len() || bytes[i + 1] != b'*')) {
                if let Some(end_rel) = bytes[i + 1..].iter().position(|b| *b == d) {
                    let content = &line[i + 1..i + 1 + end_rel];
                    if !content.is_empty()
                        && !content.starts_with(' ')
                        && !content.ends_with(' ')
                    {
                        out.push_str(&content.italic().to_string());
                        i += end_rel + 2;
                        continue;
                    }
                }
            }
        }

        // default: pass through (handle UTF-8 by slicing on char boundaries)
        let ch_end = next_char_boundary(line, i);
        out.push_str(&line[i..ch_end]);
        i = ch_end;
    }
    out
}

fn find_seq(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn next_char_boundary(s: &str, start: usize) -> usize {
    let mut end = start + 1;
    while end < s.len() && !s.is_char_boundary(end) {
        end += 1;
    }
    end.min(s.len())
}
