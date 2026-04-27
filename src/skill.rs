use crossterm::style::Stylize;
use directories::UserDirs;
use semver::Version;
use std::fs;
use std::path::PathBuf;

const REPO: &str = "hotdata-dev/hotdata-cli";
const SKILL_NAME: &str = "hotdata";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Agent root directories to check for symlink installation.
/// If the root dir exists, we create <root>/skills/hotdata -> ~/.agents/skills/hotdata
const AGENT_ROOTS: &[&str] = &[".claude", ".pi"];

fn home_dir() -> PathBuf {
    UserDirs::new()
        .expect("could not determine home directory")
        .home_dir()
        .to_path_buf()
}

/// The canonical install location: ~/.agents/skills/hotdata
/// Source of truth: ~/.hotdata/skills/hotdata
fn skill_store_path() -> PathBuf {
    home_dir().join(".hotdata").join("skills").join(SKILL_NAME)
}

/// Canonical agents layer: ~/.agents/skills/hotdata
fn agents_skill_path() -> PathBuf {
    home_dir().join(".agents").join("skills").join(SKILL_NAME)
}

fn agents_lock_path() -> PathBuf {
    home_dir().join(".agents").join(".skill-lock.json")
}

fn download_url() -> String {
    format!("https://github.com/{REPO}/releases/download/v{CURRENT_VERSION}/skills.tar.gz")
}

/// Returns agent skill paths for all agent roots that exist on disk.
fn detected_agent_skill_paths() -> Vec<(String, PathBuf)> {
    let home = home_dir();
    AGENT_ROOTS
        .iter()
        .filter_map(|root| {
            let root_path = home.join(root);
            if root_path.exists() {
                Some((root.to_string(), root_path.join("skills").join(SKILL_NAME)))
            } else {
                None
            }
        })
        .collect()
}

fn parse_version_from_skill_md(content: &str) -> Option<Version> {
    let inner = content.strip_prefix("---\n")?.split("\n---").next()?;
    for line in inner.lines() {
        if let Some(v) = line.strip_prefix("version:") {
            return Version::parse(v.trim()).ok();
        }
    }
    None
}

fn read_installed_version() -> Option<Version> {
    let content = fs::read_to_string(skill_store_path().join("SKILL.md")).ok()?;
    parse_version_from_skill_md(&content)
}

fn is_managed_by_skills_agent() -> bool {
    let content = match fs::read_to_string(agents_lock_path()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return false,
    };
    json.get(SKILL_NAME).is_some()
}

fn download_and_extract() -> Result<(), String> {
    let url = download_url();
    println!("Downloading skill...");

    // Binary download — can't route through `send_debug` (which calls
    // `resp.text()` and would corrupt the gzip stream). Log the
    // request line manually so `--debug` still shows the URL.
    crate::util::debug_request("GET", &url, &[], None);
    let client = reqwest::blocking::Client::new();
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("error downloading skill: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("error downloading skill: HTTP {}", resp.status()));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("error reading response: {e}"))?;

    // Extract into ~/.hotdata/skills/
    let store_dir = home_dir().join(".hotdata").join("skills");
    fs::create_dir_all(&store_dir).map_err(|e| format!("error creating directory: {e}"))?;

    let gz = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
    let mut archive = tar::Archive::new(gz);

    for entry in archive
        .entries()
        .map_err(|e| format!("error reading archive: {e}"))?
    {
        let mut entry = entry.map_err(|e| format!("error reading archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("error reading entry path: {e}"))?
            .into_owned();

        let rel = match path.strip_prefix("skills/") {
            Ok(r) if !r.as_os_str().is_empty() => r.to_path_buf(),
            _ => continue,
        };

        let dest = store_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("error creating directory: {e}"))?;
        }
        entry
            .unpack(&dest)
            .map_err(|e| format!("error extracting {}: {e}", rel.display()))?;
    }

    Ok(())
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("error creating directory: {e}"))?;
    for entry in fs::read_dir(src).map_err(|e| format!("error reading directory: {e}"))? {
        let entry = entry.map_err(|e| format!("error reading entry: {e}"))?;
        let dest = dst.join(entry.file_name());
        if entry.file_type().map_err(|e| format!("{e}"))?.is_dir() {
            copy_dir_recursive(&entry.path(), &dest)?;
        } else {
            fs::copy(entry.path(), &dest).map_err(|e| format!("error copying file: {e}"))?;
        }
    }
    Ok(())
}

fn ensure_symlink_or_copy(src: &PathBuf, link_path: &PathBuf) -> Result<bool, String> {
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("error creating {}: {e}", parent.display()))?;
    }

    // Remove any existing symlink or directory so we can (re)create it
    if link_path.symlink_metadata().is_ok() {
        if link_path.is_symlink() {
            fs::remove_file(link_path).map_err(|e| format!("error removing old symlink: {e}"))?;
        } else {
            fs::remove_dir_all(link_path)
                .map_err(|e| format!("error removing old directory: {e}"))?;
        }
    }

    // Try symlink first, fall back to copy
    #[cfg(unix)]
    if std::os::unix::fs::symlink(src, link_path).is_ok() {
        return Ok(true);
    }

    #[cfg(windows)]
    if std::os::windows::fs::symlink_dir(src, link_path).is_ok() {
        return Ok(true);
    }

    copy_dir_recursive(src, link_path)?;
    Ok(false) // false = copied, not symlinked
}

fn ensure_symlinks() -> Vec<(String, PathBuf, Result<bool, String>)> {
    let store_path = skill_store_path();
    let agents_path = agents_skill_path();
    let mut results = Vec::new();

    // First: ~/.agents/skills/hotdata -> ~/.hotdata/skills/hotdata
    let agents_result = ensure_symlink_or_copy(&store_path, &agents_path);
    results.push(("~/.agents".to_string(), agents_path.clone(), agents_result));

    // Then: each detected agent root -> ~/.agents/skills/hotdata
    for (root, link_path) in detected_agent_skill_paths() {
        let result = ensure_symlink_or_copy(&agents_path, &link_path);
        results.push((format!("~/{root}"), link_path, result));
    }

    results
}

pub fn install_project() {
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");
    let store_path = skill_store_path();

    // Ensure skill files exist locally first
    match read_installed_version() {
        Some(ref v) if *v >= current => {}
        Some(ref v) => {
            println!(
                "{}",
                format!("Global skill is outdated (v{v}), downloading v{current} first...")
                    .yellow()
            );
            if let Err(e) = download_and_extract() {
                eprintln!("{}", e.red());
                std::process::exit(1);
            }
        }
        None => {
            println!("Skill not installed globally, downloading v{current}...");
            if let Err(e) = download_and_extract() {
                eprintln!("{}", e.red());
                std::process::exit(1);
            }
        }
    }

    let cwd = std::env::current_dir().expect("could not determine current directory");
    let project_agents = cwd.join(".agents").join("skills").join(SKILL_NAME);

    // Always copy (not symlink) from store to .agents/skills/hotdata
    if project_agents.exists() {
        fs::remove_dir_all(&project_agents).unwrap_or_else(|e| {
            eprintln!(
                "{}",
                format!("error removing existing directory: {e}").red()
            );
            std::process::exit(1);
        });
    }
    if let Some(parent) = project_agents.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("{}", format!("error creating directory: {e}").red());
            std::process::exit(1);
        });
    }
    copy_dir_recursive(&store_path, &project_agents).unwrap_or_else(|e| {
        eprintln!("{}", e.red());
        std::process::exit(1);
    });

    let rel_agents = project_agents.strip_prefix(&cwd).unwrap_or(&project_agents);

    println!(
        "{}",
        format!("Skill installed to project (v{current}).").green()
    );
    println!(
        "{:<20}{}",
        "Location:",
        rel_agents.display().to_string().cyan()
    );

    // For .claude and .pi in cwd: symlink (fallback copy) from .agents/skills/hotdata
    for root in AGENT_ROOTS {
        let root_path = cwd.join(root);
        if root_path.exists() {
            let link_path = root_path.join("skills").join(SKILL_NAME);
            let rel_link = link_path.strip_prefix(&cwd).unwrap_or(&link_path);
            match ensure_symlink_or_copy(&project_agents, &link_path) {
                Ok(true) => println!(
                    "{:<20}{}",
                    format!("./{root}:"),
                    rel_link.display().to_string().cyan()
                ),
                Ok(false) => println!(
                    "{:<20}{} (copied)",
                    format!("./{root}:"),
                    rel_link.display().to_string().cyan()
                ),
                Err(e) => eprintln!("{}", format!("./{root}: failed: {e}").red()),
            }
        }
    }
}

pub fn install() {
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    let needs_download = if is_managed_by_skills_agent() {
        match read_installed_version() {
            Some(ref v) if *v >= current => {
                println!("Managed by skills agent — already up to date (v{v}).");
                false
            }
            Some(ref v) => {
                println!(
                    "{}",
                    format!("Managed by skills agent — updating from v{v} to v{current}...")
                        .yellow()
                );
                true
            }
            None => {
                println!("Installing hotdata skill v{current}...");
                true
            }
        }
    } else {
        match read_installed_version() {
            Some(ref v) if *v >= current => {
                println!("Already up to date (v{v}).");
                false
            }
            Some(ref v) => {
                println!("Updating from v{v} to v{current}...");
                true
            }
            None => {
                println!("Installing hotdata skill v{current}...");
                true
            }
        }
    };

    if needs_download && let Err(e) = download_and_extract() {
        eprintln!("{}", e.red());
        std::process::exit(1);
    }

    let symlinks = ensure_symlinks();

    println!(
        "{}",
        format!("Skill installed successfully (v{current}).").green()
    );
    println!("{:<20}{}", "Location:", skill_store_path().display());

    for (label, path, result) in &symlinks {
        let status = match result {
            Ok(true) => format!("{} (symlinked)", path.display().to_string().cyan()),
            Ok(false) => format!("{} (copied)", path.display().to_string().cyan()),
            Err(e) => format!("failed: {e}").red().to_string(),
        };
        println!("{:<20}{}", format!("{label}:"), status);
    }
}

pub fn status() {
    let store_path = skill_store_path();
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    let installed_version = read_installed_version();
    let exists = store_path.exists();

    fn row(label: &str, value: &str) {
        println!("{:<20}{}", format!("{label}:"), value);
    }

    if !exists {
        row("Installed", &"No".red().to_string());
        println!("\nRun 'hotdata skills install' to install.");
        return;
    }

    row("Installed", &"Yes".green().to_string());

    match &installed_version {
        Some(v) if *v < current => {
            row(
                "Version",
                &format!(
                    "{} (outdated, current is v{current})",
                    v.to_string().yellow()
                ),
            );
        }
        Some(v) => row("Version", &v.to_string().green().to_string()),
        None => row("Version", &"unknown".dark_grey().to_string()),
    }

    let home = home_dir();

    // Collect installed agent skill paths
    let agents_path = agents_skill_path();
    let mut installed_agents: Vec<String> = Vec::new();

    if agents_path.exists() {
        installed_agents.push("~/.agents".to_string());
    }
    for root in AGENT_ROOTS {
        let link_path = home.join(root).join("skills").join(SKILL_NAME);
        if link_path.exists() {
            installed_agents.push(format!("~/{root}"));
        }
    }

    if installed_agents.is_empty() {
        row("Agent Skills", &"none".dark_grey().to_string());
    } else {
        row(
            "Agent Skills Added",
            &installed_agents.join(", ").cyan().to_string(),
        );
    }

    if installed_version.is_some_and(|v| v < current) {
        println!("\nRun 'hotdata skills install' to update.");
    }
}
