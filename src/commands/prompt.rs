//! Shared interactive-prompt helpers (inquire wrappers).
//!
//! Conventions all wizards agree on: prompting is TTY-gated by callers or by
//! the helper itself; `inquire` returns Err on Ctrl-C/ESC, which exits the
//! process cleanly with code 0. Used by the ingest wizard; the connections
//! wizard (`connections/interactive.rs`) predates this module and still
//! inlines the same patterns — migrating it here is a welcome follow-up.

use crate::util;
use inquire::{Password, Select, Text};

pub(crate) fn ask_text(label: &str) -> String {
    Text::new(label)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0))
}

pub(crate) fn ask_secret(label: &str) -> String {
    Password::new(label)
        .without_confirmation()
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0))
}

/// Optional value: the flag, else a prompt (TTY) whose blank answer = none.
pub(crate) fn optional(flag: Option<String>, label: &str) -> Option<String> {
    if flag.is_some() {
        return flag;
    }
    if util::is_interactive() {
        let v = ask_text(label);
        return (!v.trim().is_empty()).then_some(v);
    }
    None
}

/// Optional value with a default; non-interactive falls back to the default.
pub(crate) fn optional_default(label: &str, default: &str) -> Option<String> {
    if !util::is_interactive() {
        return Some(default.to_string());
    }
    let v = Text::new(label)
        .with_default(default)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0));
    (!v.trim().is_empty()).then_some(v)
}

/// Select one of `options` (TTY only; None otherwise or on ESC).
pub(crate) fn select_optional(label: &str, options: &[&str]) -> Option<String> {
    if !util::is_interactive() {
        return None;
    }
    Select::new(label, options.to_vec())
        .prompt()
        .ok()
        .map(|s| s.to_string())
}

/// Prompt for a comma-separated list (TTY only; empty otherwise).
pub(crate) fn prompt_list(label: &str) -> Vec<String> {
    if !util::is_interactive() {
        return Vec::new();
    }
    ask_text(label)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
