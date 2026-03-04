use crate::config::{self, ApiKeySource};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor, Stylize};
use crossterm::ExecutableCommand;
use std::io::stdout;

pub fn status(profile: &str) {
    let profile_config = match config::load(profile) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let source_label = if profile_config.api_key_source == ApiKeySource::Env {
        " (env override)"
    } else {
        ""
    };

    let api_key = match &profile_config.api_key {
        Some(key) if key != "PLACEHOLDER" => key.clone(),
        _ => {
            print_row("Profile", &profile.white().to_string());
            print_row("Authenticated", &"No".red().to_string());
            print_row("API Key", &"Not configured".red().to_string());
            return;
        }
    };

    let url = format!("{}/workspaces", profile_config.api_url);
    let client = reqwest::blocking::Client::new();

    match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
    {
        Ok(resp) if resp.status().is_success() => {
            print_row("Profile", &profile.white().to_string());
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row("Authenticated", &"Yes".green().to_string());
            print_row("API Key", &format!("{}{source_label}", "Valid".green()));
        }
        Ok(resp) => {
            print_row("Profile", &profile.white().to_string());
            print_row("API URL", &profile_config.api_url.cyan().to_string());
            print_row("Authenticated", &"No".red().to_string());
            print_row(
                "API Key",
                &format!("{}{source_label}", format!("Invalid (HTTP {})", resp.status()).red()),
            );
        }
        Err(e) => {
            eprintln!("error connecting to API: {e}");
            std::process::exit(1);
        }
    }
}

fn print_row(label: &str, value: &str) {
    stdout()
        .execute(SetForegroundColor(Color::DarkGrey))
        .unwrap()
        .execute(Print(format!("{:<15}", format!("{label}:"))))
        .unwrap()
        .execute(ResetColor)
        .unwrap()
        .execute(Print(format!("{value}\n")))
        .unwrap();
}
