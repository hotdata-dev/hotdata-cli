//! Interactive prompt helpers — thin `inquire` wrappers shared by the guided
//! flows.
//!
//! Two conventions every caller depends on:
//!
//! - **The caller gates prompting**, on `util::is_interactive()` — false for
//!   `--no-input`, for CI, and for a non-TTY stdin. Nothing in here checks it,
//!   so gating happens once at the entry to a flow rather than being re-decided
//!   per question; a helper called from a script path would block on a terminal
//!   that is not there instead of failing.
//! - **Ctrl-C and ESC exit 0.** `inquire` reports both as `Err`, and a flow the
//!   user backed out of has created nothing and failed at nothing.
//!
//! Secrets go through [`secret`], which neither echoes nor accepts a default: a
//! masked prompt carrying a pre-filled value still shows its length, and the
//! value would then be a secret the user never typed and cannot see to correct.

use inquire::{Confirm, Password, PasswordDisplayMode, Select, Text};

/// A free-text answer, optionally pre-filled and annotated.
pub fn text(label: &str, help: Option<&str>, default: Option<&str>) -> String {
    let mut prompt = Text::new(label);
    if let Some(d) = default {
        prompt = prompt.with_default(d);
    }
    if let Some(h) = help {
        prompt = prompt.with_help_message(h);
    }
    prompt.prompt().unwrap_or_else(|_| std::process::exit(0))
}

/// A hidden answer. Masked rather than fully invisible so the user can see that
/// a paste landed — a terminal that shows nothing at all reads as a hung prompt,
/// and the retry is where people paste into the scrollback instead.
pub fn secret(label: &str, help: Option<&str>) -> String {
    let mut prompt = Password::new(label)
        .without_confirmation()
        .with_display_mode(PasswordDisplayMode::Masked);
    if let Some(h) = help {
        prompt = prompt.with_help_message(h);
    }
    prompt.prompt().unwrap_or_else(|_| std::process::exit(0))
}

/// Pick one of `options`, returning its index. The index, not the label,
/// because the label is built for reading and the caller needs the row.
pub fn select_index(label: &str, help: Option<&str>, options: &[String]) -> usize {
    let mut prompt = Select::new(label, options.to_vec()).with_page_size(15);
    if let Some(h) = help {
        prompt = prompt.with_help_message(h);
    }
    let chosen = prompt.prompt().unwrap_or_else(|_| std::process::exit(0));
    options
        .iter()
        .position(|o| *o == chosen)
        .expect("inquire returns one of the options it was given")
}

pub fn confirm(label: &str, default: bool) -> bool {
    Confirm::new(label)
        .with_default(default)
        .prompt()
        .unwrap_or_else(|_| std::process::exit(0))
}
