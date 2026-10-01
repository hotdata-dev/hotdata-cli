use crossterm::style::Stylize;
use directories::UserDirs;
use semver::Version;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Subcommands for `hotdata manage skills`.
#[derive(clap::Subcommand)]
pub enum SkillCommands {
    /// Install or update the hotdata skill into agent directories
    Install {
        /// Install into the current project directory instead of globally
        #[arg(long)]
        project: bool,
    },
    /// Show the installation status of the hotdata skill
    Status,
    /// List installed skills and their versions (alias for status)
    List,
}

const REPO: &str = "hotdata-dev/hotdata-cli";
const PRIMARY_SKILL_NAME: &str = "hotdata";
/// Skills registered as top-level agent skills (autocompletable). Only `hotdata`
/// installs at the top level; the specialized skills (search, analytics,
/// geospatial) are bundled *inside* `hotdata/subskills/` and loaded on demand by
/// the `hotdata` skill, so they never appear as separate entries.
const SKILL_NAMES: &[&str] = &["hotdata"];
/// Skills that were previously installed as top-level entries and are now bundled
/// under `hotdata/subskills/`. On install/update we remove any stale top-level
/// copies so upgraders stop seeing them in the agent skill list.
const RETIRED_SKILL_NAMES: &[&str] = &["hotdata-search", "hotdata-analytics", "hotdata-geospatial"];
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Agent root directories to check for symlink installation.
/// If the root dir exists, we create <root>/skills/<skill> -> ~/.agents/skills/<skill>
const AGENT_ROOTS: &[&str] = &[".claude", ".pi"];

fn home_dir() -> PathBuf {
    UserDirs::new()
        .expect("could not determine home directory")
        .home_dir()
        .to_path_buf()
}

/// The canonical store location: ~/.hotdata/skills/<skill>
fn skill_store_path(skill_name: &str) -> PathBuf {
    home_dir().join(".hotdata").join("skills").join(skill_name)
}

/// Canonical agents layer: ~/.agents/skills/<skill>
fn agents_skill_path(skill_name: &str) -> PathBuf {
    home_dir().join(".agents").join("skills").join(skill_name)
}

fn agents_lock_path() -> PathBuf {
    home_dir().join(".agents").join(".skill-lock.json")
}

fn download_url() -> String {
    format!("https://github.com/{REPO}/releases/download/v{CURRENT_VERSION}/skills.tar.gz")
}

/// Returns agent skill paths for all agent roots that exist on disk.
fn detected_agent_skill_paths(skill_name: &str) -> Vec<(String, PathBuf)> {
    let home = home_dir();
    AGENT_ROOTS
        .iter()
        .filter_map(|root| {
            let root_path = home.join(root);
            if root_path.exists() {
                Some((root.to_string(), root_path.join("skills").join(skill_name)))
            } else {
                None
            }
        })
        .collect()
}

fn parse_version_from_skill_md(content: &str) -> Option<Version> {
    let content = content.strip_prefix('\u{FEFF}').unwrap_or(content);
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))?;
    let inner = rest.split("\n---").next()?;
    for raw_line in inner.lines() {
        let line = raw_line.trim();
        let Some(ver_raw) = line.strip_prefix("version:") else {
            continue;
        };
        let ver_str = ver_raw.trim().trim_start_matches('v').trim();
        if let Ok(v) = Version::parse(ver_str) {
            return Some(v);
        }
    }
    None
}

fn read_installed_version() -> Option<Version> {
    let content = fs::read_to_string(skill_store_path(PRIMARY_SKILL_NAME).join("SKILL.md")).ok()?;
    parse_version_from_skill_md(&content)
}

fn all_skill_stores_present() -> bool {
    SKILL_NAMES
        .iter()
        .all(|name| skill_store_path(name).exists())
}

fn skill_auto_update_suppress_path() -> PathBuf {
    home_dir()
        .join(".hotdata")
        .join("skills")
        .join(".skill_auto_update_suppressed_for_cli")
}

fn skill_auto_update_suppressed_for_this_cli() -> bool {
    let Ok(s) = fs::read_to_string(skill_auto_update_suppress_path()) else {
        return false;
    };
    s.trim() == CURRENT_VERSION
}

fn suppress_skill_auto_update_for_this_cli() {
    let path = skill_auto_update_suppress_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, format!("{CURRENT_VERSION}\n"));
}

fn clear_skill_auto_update_suppression() {
    let _ = fs::remove_file(skill_auto_update_suppress_path());
}

/// If the user has previously installed agent skills (`~/.hotdata/skills/hotdata` exists) but the on-disk
/// bundle is older than this CLI or incomplete, download the matching release tarball and refresh symlinks.
/// Does nothing when skills were never installed or when [`is_managed_by_skills_agent`] is true.
/// Download failures print a warning and do not exit.
pub fn maybe_auto_update_after_cli_upgrade() {
    if is_managed_by_skills_agent() {
        return;
    }
    if !skill_store_path(PRIMARY_SKILL_NAME).exists() {
        return;
    }

    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");
    let needs_refresh =
        !matches!(read_installed_version(), Some(v) if v >= current && all_skill_stores_present());
    if !needs_refresh {
        clear_skill_auto_update_suppression();
        return;
    }

    if skill_auto_update_suppressed_for_this_cli() {
        return;
    }

    if let Err(e) = download_and_extract() {
        eprintln!(
            "{}",
            format!("warning: could not auto-update agent skills: {e}").yellow()
        );
        return;
    }

    let _symlinks = ensure_symlinks();
    remove_retired_skills_global();

    let still_needed =
        !matches!(read_installed_version(), Some(v) if v >= current && all_skill_stores_present());

    if still_needed {
        suppress_skill_auto_update_for_this_cli();
        eprintln!(
            "{}",
            format!(
                "warning: agent skills still do not match this CLI after download (release tarball may lag the binary). Automatic refresh is suppressed for CLI v{CURRENT_VERSION}; remove {} to retry, or run `hotdata manage skills install`.",
                skill_auto_update_suppress_path().display()
            )
            .yellow()
        );
        return;
    }

    clear_skill_auto_update_suppression();
    eprintln!("{}", format!("Agent skills updated to v{current}.").green());
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
    json.get(PRIMARY_SKILL_NAME).is_some()
}

fn download_and_extract() -> Result<(), String> {
    download_and_extract_from_url(&download_url())
}

fn download_and_extract_from_url(url: &str) -> Result<(), String> {
    eprintln!("Downloading skill...");

    // Binary download — can't route through `send_debug` (which calls
    // `resp.text()` and would corrupt the gzip stream). Log the
    // request line manually so `--debug` still shows the URL.
    crate::util::debug_request("GET", url, &[], None);
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("error creating HTTP client: {e}"))?;
    let resp = client
        .get(url)
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
    extract_skills_archive(&bytes, &store_dir)
}

/// Unpack the `skills/` tree of a gzipped tarball into `store_dir`.
///
/// The whole archive is validated before anything is written, so an archive
/// with an invalid entry path leaves the store untouched.
fn extract_skills_archive(bytes: &[u8], store_dir: &Path) -> Result<(), String> {
    for_each_skill_entry(bytes, |_, _| Ok(()))?;

    fs::create_dir_all(store_dir).map_err(|e| format!("error creating directory: {e}"))?;
    for_each_skill_entry(bytes, |mut entry, rel| {
        let dest = store_dir.join(&rel);
        ensure_no_symlinks(store_dir, &rel)?;
        if entry.header().entry_type().is_dir() {
            return fs::create_dir_all(&dest).map_err(|e| format!("error creating directory: {e}"));
        }
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("error creating directory: {e}"))?;
        }
        entry
            .unpack(&dest)
            .map(|_| ())
            .map_err(|e| format!("error extracting {}: {e}", rel.display()))
    })
}

/// Call `f` with each regular file or directory under `skills/`, along with its
/// path relative to `skills/`. Other entry types and paths are skipped.
fn for_each_skill_entry<'b>(
    bytes: &'b [u8],
    mut f: impl FnMut(tar::Entry<'_, flate2::read::GzDecoder<&'b [u8]>>, PathBuf) -> Result<(), String>,
) -> Result<(), String> {
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for entry in archive
        .entries()
        .map_err(|e| format!("error reading archive: {e}"))?
    {
        let entry = entry.map_err(|e| format!("error reading archive entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("error reading entry path: {e}"))?
            .into_owned();

        // Archive paths are untrusted; keep writes inside store_dir.
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
        {
            return Err(format!("invalid path in skill archive: {}", path.display()));
        }
        let mut components = path.components();
        if components.next() != Some(Component::Normal("skills".as_ref())) {
            continue;
        }
        let rel: PathBuf = components.collect();
        let kind = entry.header().entry_type();
        if rel.as_os_str().is_empty() || !(kind.is_file() || kind.is_dir()) {
            continue;
        }
        f(entry, rel)?;
    }
    Ok(())
}

/// Refuse to write through a symlink already present under `store_dir`.
fn ensure_no_symlinks(store_dir: &Path, rel: &Path) -> Result<(), String> {
    let mut path = store_dir.to_path_buf();
    for component in rel.components() {
        path.push(component);
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(format!(
                "error extracting {}: {} is a symlink",
                rel.display(),
                path.display()
            ));
        }
    }
    Ok(())
}

/// Download and install skills for `version`.  Called from `update_to()` after
/// the binary has been atomically swapped so that skills match the new CLI on
/// first use.  Uses the release tarball URL for `version` (not `CURRENT_VERSION`,
/// which is still the old binary at call time).  Skips silently when managed by
/// a skills agent.  Prints a warning on failure so the binary update is not
/// rolled back.
pub fn install_for_version(version: &Version) {
    if is_managed_by_skills_agent() {
        return;
    }
    let url = format!("https://github.com/{REPO}/releases/download/v{version}/skills.tar.gz");
    if let Err(e) = download_and_extract_from_url(&url) {
        eprintln!(
            "{}",
            format!("warning: could not update agent skills: {e}").yellow()
        );
        return;
    }
    let _symlinks = ensure_symlinks();
    remove_retired_skills_global();
    clear_skill_auto_update_suppression();
    println!("{}", format!("Agent skills updated to v{version}.").green());
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
    let mut results = Vec::new();

    for skill_name in SKILL_NAMES {
        let store_path = skill_store_path(skill_name);
        let agents_path = agents_skill_path(skill_name);

        // First: ~/.agents/skills/<skill> -> ~/.hotdata/skills/<skill>
        let agents_result = ensure_symlink_or_copy(&store_path, &agents_path);
        results.push((
            format!("~/.agents ({skill_name})"),
            agents_path.clone(),
            agents_result,
        ));

        // Then: each detected agent root -> ~/.agents/skills/<skill>
        for (root, link_path) in detected_agent_skill_paths(skill_name) {
            let result = ensure_symlink_or_copy(&agents_path, &link_path);
            results.push((format!("~/{root} ({skill_name})"), link_path, result));
        }
    }

    results
}

/// Best-effort removal of a single skill directory or symlink at `path`.
fn remove_skill_path(path: &PathBuf) {
    if path.symlink_metadata().is_err() {
        return;
    }
    let _ = if path.is_symlink() {
        fs::remove_file(path)
    } else {
        fs::remove_dir_all(path)
    };
}

/// Remove stale top-level copies of skills that are now bundled under
/// `hotdata/subskills/` (see `RETIRED_SKILL_NAMES`). Cleans the store, the
/// `~/.agents` layer, and every detected home agent root. Best-effort: a
/// retired skill that isn't present is simply skipped.
fn remove_retired_skills_global() {
    for name in RETIRED_SKILL_NAMES {
        remove_skill_path(&skill_store_path(name));
        remove_skill_path(&agents_skill_path(name));
        for (_root, link_path) in detected_agent_skill_paths(name) {
            remove_skill_path(&link_path);
        }
    }
}

/// Project-scoped counterpart to `remove_retired_skills_global`: clears stale
/// retired skills from `./.agents/skills` and each detected project agent root.
fn remove_retired_skills_project(cwd: &std::path::Path, project_skills_root: &std::path::Path) {
    for name in RETIRED_SKILL_NAMES {
        remove_skill_path(&project_skills_root.join(name));
        for root in AGENT_ROOTS {
            let root_path = cwd.join(root);
            if root_path.exists() {
                remove_skill_path(&root_path.join("skills").join(name));
            }
        }
    }
}

pub fn install_project() {
    clear_skill_auto_update_suppression();
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    // Ensure skill files exist locally first
    match read_installed_version() {
        Some(ref v) if *v >= current && all_skill_stores_present() => {}
        Some(ref v) if *v >= current => {
            println!(
                "{}",
                format!("Incomplete skills in ~/.hotdata/skills, downloading v{current}...")
                    .yellow()
            );
            if let Err(e) = download_and_extract() {
                eprintln!("{}", e.red());
                std::process::exit(1);
            }
        }
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
    let project_skills_root = cwd.join(".agents").join("skills");

    // Always copy (not symlink) from store to .agents/skills/<skill>
    if let Some(parent) = project_skills_root.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| {
            eprintln!("{}", format!("error creating directory: {e}").red());
            std::process::exit(1);
        });
    }

    for skill_name in SKILL_NAMES {
        let store_path = skill_store_path(skill_name);
        let project_agents = project_skills_root.join(skill_name);

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
    }

    println!(
        "{}",
        format!("Skill installed to project (v{current}).").green()
    );
    println!("{:<20}{}", "Location:", ".agents/skills".cyan());

    // For .claude and .pi in cwd: symlink (fallback copy) from .agents/skills/<skill>
    for root in AGENT_ROOTS {
        let root_path = cwd.join(root);
        if !root_path.exists() {
            continue;
        }
        for skill_name in SKILL_NAMES {
            let project_agents = project_skills_root.join(skill_name);
            let link_path = root_path.join("skills").join(skill_name);
            let rel_link = link_path.strip_prefix(&cwd).unwrap_or(&link_path);
            match ensure_symlink_or_copy(&project_agents, &link_path) {
                Ok(true) => println!(
                    "{:<20}{}",
                    format!("./{root} ({skill_name}):"),
                    rel_link.display().to_string().cyan()
                ),
                Ok(false) => println!(
                    "{:<20}{} (copied)",
                    format!("./{root} ({skill_name}):"),
                    rel_link.display().to_string().cyan()
                ),
                Err(e) => eprintln!("{}", format!("./{root} ({skill_name}): failed: {e}").red()),
            }
        }
    }

    remove_retired_skills_project(&cwd, &project_skills_root);
}

pub fn install() {
    clear_skill_auto_update_suppression();
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    let needs_download = if is_managed_by_skills_agent() {
        match read_installed_version() {
            Some(ref v) if *v >= current && all_skill_stores_present() => {
                println!("Managed by skills agent — already up to date (v{v}).");
                false
            }
            Some(ref v) if *v >= current => {
                println!(
                    "{}",
                    format!("Managed by skills agent — completing skill install (v{current})...")
                        .yellow()
                );
                true
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
            Some(ref v) if *v >= current && all_skill_stores_present() => {
                println!("Already up to date (v{v}).");
                false
            }
            Some(ref v) if *v >= current => {
                println!(
                    "{}",
                    format!("Completing skill install (v{current})...").yellow()
                );
                true
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
    remove_retired_skills_global();

    println!(
        "{}",
        format!("Skill installed successfully (v{current}).").green()
    );
    println!(
        "{:<20}{}",
        "Location:",
        "~/.hotdata/skills/<skill>".dark_grey()
    );
    for skill_name in SKILL_NAMES {
        println!(
            "{:<20}{}",
            format!("{skill_name}:"),
            skill_store_path(skill_name).display().to_string().cyan()
        );
    }

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
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    let installed_version = read_installed_version();

    fn row(label: &str, value: &str) {
        println!("{:<20}{}", format!("{label}:"), value);
    }

    let any_exist = SKILL_NAMES
        .iter()
        .any(|name| skill_store_path(name).exists());
    let all_exist = SKILL_NAMES
        .iter()
        .all(|name| skill_store_path(name).exists());

    if !any_exist {
        row("Installed", &"No".red().to_string());
        println!("\nRun 'hotdata manage skills install' to install.");
        return;
    }

    if !all_exist {
        row("Installed", &"Partial".yellow().to_string());
        for skill_name in SKILL_NAMES {
            let ok = skill_store_path(skill_name).exists();
            let status = if ok {
                "Yes".green().to_string()
            } else {
                "No".red().to_string()
            };
            row(skill_name, &status);
        }
    } else {
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
    }

    let home = home_dir();
    for skill_name in SKILL_NAMES {
        let label = format!("Agent Skills ({skill_name})");
        if !skill_store_path(skill_name).exists() {
            row(&label, &"—".dark_grey().to_string());
            continue;
        }
        let mut installed_agents: Vec<String> = Vec::new();
        if agents_skill_path(skill_name).exists() {
            installed_agents.push("~/.agents".to_string());
        }
        for root in AGENT_ROOTS {
            let link_path = home.join(root).join("skills").join(skill_name);
            if link_path.exists() {
                installed_agents.push(format!("~/{root}"));
            }
        }
        if installed_agents.is_empty() {
            row(&label, &"none".dark_grey().to_string());
        } else {
            row(&label, &installed_agents.join(", ").cyan().to_string());
        }
    }

    if !all_exist {
        println!("\nRun 'hotdata manage skills install' to install.");
    } else if installed_version.is_some_and(|v| v < current) {
        println!("\nRun 'hotdata manage skills install' to update.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_skill_md_accepts_lf_frontmatter() {
        let s = "---\nname: hotdata\nversion: 0.1.14\n---\n";
        assert_eq!(
            parse_version_from_skill_md(s),
            Some(Version::parse("0.1.14").unwrap())
        );
    }

    #[test]
    fn parse_skill_md_accepts_crlf_opening() {
        let s = "---\r\nname: hotdata\r\nversion: 0.1.14\r\n---\r\n";
        assert_eq!(
            parse_version_from_skill_md(s),
            Some(Version::parse("0.1.14").unwrap())
        );
    }

    #[test]
    fn parse_skill_md_accepts_bom() {
        let s = "\u{FEFF}---\nname: hotdata\nversion: 0.1.14\n---\n";
        assert_eq!(
            parse_version_from_skill_md(s),
            Some(Version::parse("0.1.14").unwrap())
        );
    }

    #[test]
    fn parse_skill_md_accepts_v_prefix() {
        let s = "---\nname: hotdata\nversion: v0.1.14\n---\n";
        assert_eq!(
            parse_version_from_skill_md(s),
            Some(Version::parse("0.1.14").unwrap())
        );
    }

    #[test]
    fn remove_retired_skills_project_clears_stale_top_level_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let project_skills_root = cwd.join(".agents").join("skills");

        // A retired skill present as a real dir in the `.agents` layer and as a
        // symlink in the `.claude` root (exercises both branches of
        // `remove_skill_path`), plus the surviving `hotdata` skill.
        let agents_retired = project_skills_root.join("hotdata-search");
        fs::create_dir_all(&agents_retired).unwrap();
        let agents_hotdata = project_skills_root.join("hotdata");
        fs::create_dir_all(&agents_hotdata).unwrap();

        let claude_skills = cwd.join(".claude").join("skills");
        fs::create_dir_all(&claude_skills).unwrap();
        let claude_retired = claude_skills.join("hotdata-search");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&agents_retired, &claude_retired).unwrap();
        #[cfg(not(unix))]
        fs::create_dir_all(&claude_retired).unwrap();

        remove_retired_skills_project(cwd, &project_skills_root);

        assert!(
            !agents_retired.exists(),
            "retired skill in .agents should be removed"
        );
        assert!(
            claude_retired.symlink_metadata().is_err(),
            "retired skill symlink in .claude should be removed"
        );
        assert!(
            agents_hotdata.exists(),
            "the surviving hotdata skill must be left in place"
        );
    }

    enum TestEntry<'a> {
        Dir(&'a str),
        File(&'a str, &'a [u8]),
        Symlink(&'a str, &'a Path),
        HardLink(&'a str, &'a Path),
    }

    /// Build a gzipped tarball, writing header paths verbatim so entries the
    /// `tar` builder would normalise can still be exercised.
    fn build_archive(entries: &[TestEntry]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);
        for entry in entries {
            let (path, data, kind, link): (&str, &[u8], _, Option<&Path>) = match entry {
                TestEntry::Dir(p) => (p, &[], tar::EntryType::Directory, None),
                TestEntry::File(p, d) => (p, d, tar::EntryType::Regular, None),
                TestEntry::Symlink(p, t) => (p, &[], tar::EntryType::Symlink, Some(t)),
                TestEntry::HardLink(p, t) => (p, &[], tar::EntryType::Link, Some(t)),
            };
            let mut header = tar::Header::new_gnu();
            let gnu = header.as_gnu_mut().unwrap();
            gnu.name[..path.len()].copy_from_slice(path.as_bytes());
            if let Some(target) = link {
                let target = target.to_str().unwrap().as_bytes();
                gnu.linkname[..target.len()].copy_from_slice(target);
            }
            header.set_entry_type(kind);
            header.set_mode(if kind.is_dir() { 0o755 } else { 0o644 });
            header.set_size(data.len() as u64);
            header.set_cksum();
            builder.append(&header, data).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    fn store_in(tmp: &tempfile::TempDir) -> PathBuf {
        tmp.path().join("store")
    }

    #[test]
    fn extracts_skills_archive_into_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let archive = build_archive(&[
            TestEntry::Dir("skills/"),
            TestEntry::Dir("skills/hotdata/"),
            TestEntry::File("skills/hotdata/SKILL.md", b"---\nversion: 1.2.3\n---\n"),
            TestEntry::File("skills/hotdata/subskills/search/SKILL.md", b"search"),
        ]);

        extract_skills_archive(&archive, &store).unwrap();

        assert_eq!(
            fs::read(store.join("hotdata/SKILL.md")).unwrap(),
            b"---\nversion: 1.2.3\n---\n"
        );
        assert_eq!(
            fs::read(store.join("hotdata/subskills/search/SKILL.md")).unwrap(),
            b"search"
        );
    }

    #[test]
    fn ignores_entries_outside_skills_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let archive = build_archive(&[
            TestEntry::File("README.md", b"readme"),
            TestEntry::File("other/hotdata/SKILL.md", b"other"),
            TestEntry::File("skills/hotdata/SKILL.md", b"skill"),
        ]);

        extract_skills_archive(&archive, &store).unwrap();

        assert_eq!(fs::read(store.join("hotdata/SKILL.md")).unwrap(), b"skill");
        assert!(!store.join("README.md").exists());
        assert!(!store.join("other").exists());
        assert!(!tmp.path().join("README.md").exists());
        assert!(!tmp.path().join("other").exists());
    }

    #[test]
    fn rejects_parent_dir_entries() {
        for path in ["skills/../escaped", "skills/hotdata/../../escaped"] {
            let tmp = tempfile::tempdir().unwrap();
            let store = store_in(&tmp);
            let archive = build_archive(&[
                TestEntry::File("skills/hotdata/SKILL.md", b"skill"),
                TestEntry::File(path, b"escaped"),
            ]);

            let err = extract_skills_archive(&archive, &store).unwrap_err();

            assert!(
                err.contains("invalid path in skill archive"),
                "{path}: {err}"
            );
            assert!(!tmp.path().join("escaped").exists(), "{path}");
            assert!(
                !store.join("hotdata/SKILL.md").exists(),
                "{path}: nothing should be extracted from a rejected archive"
            );
        }
    }

    #[test]
    fn rejects_absolute_path_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let outside = tmp.path().join("outside.md");
        let archive = build_archive(&[TestEntry::File(outside.to_str().unwrap(), b"outside")]);

        let err = extract_skills_archive(&archive, &store).unwrap_err();

        assert!(err.contains("invalid path in skill archive"), "{err}");
        assert!(!outside.exists());
    }

    #[test]
    fn skips_symlink_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let archive = build_archive(&[
            TestEntry::Symlink("skills/hotdata/linked", &outside),
            TestEntry::File("skills/hotdata/linked/SKILL.md", b"through link"),
        ]);

        extract_skills_archive(&archive, &store).unwrap();

        assert!(!outside.join("SKILL.md").exists());
        let linked = store.join("hotdata/linked");
        assert!(!linked.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read(linked.join("SKILL.md")).unwrap(), b"through link");
    }

    #[test]
    fn skips_hard_link_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let outside = tmp.path().join("outside.md");
        fs::write(&outside, b"original").unwrap();
        let archive = build_archive(&[
            TestEntry::HardLink("skills/hotdata/SKILL.md", &outside),
            TestEntry::File("skills/hotdata/README.md", b"readme"),
        ]);

        extract_skills_archive(&archive, &store).unwrap();

        assert!(!store.join("hotdata/SKILL.md").exists());
        assert_eq!(
            fs::read(store.join("hotdata/README.md")).unwrap(),
            b"readme"
        );
        assert_eq!(fs::read(&outside).unwrap(), b"original");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_existing_symlink_in_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = store_in(&tmp);
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        fs::create_dir_all(&store).unwrap();
        std::os::unix::fs::symlink(&outside, store.join("hotdata")).unwrap();
        let archive = build_archive(&[TestEntry::File("skills/hotdata/SKILL.md", b"skill")]);

        assert!(extract_skills_archive(&archive, &store).is_err());
        assert!(!outside.join("SKILL.md").exists());
    }
}
