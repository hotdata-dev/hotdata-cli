use crate::util;
use crossterm::style::Stylize;
use semver::Version;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const REPO_OWNER: &str = "hotdata-dev";
const REPO_NAME: &str = "hotdata-cli";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// Fully-qualified Homebrew formula (e.g. `hotdata-dev/tap/cli`). Pulled from
/// `[package.metadata.hotdata]` in Cargo.toml via build.rs so the README,
/// dist-workspace.toml, and this binary all agree on the same target.
const HOMEBREW_FORMULA: &str = env!("HOTDATA_HOMEBREW_FORMULA");
const CHECK_INTERVAL_SECS: u64 = 86_400;
const NETWORK_TIMEOUT_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMethod {
    Homebrew,
    Other,
}

pub fn detect_install_method() -> InstallMethod {
    let Ok(exe) = std::env::current_exe() else {
        return InstallMethod::Other;
    };
    let path = fs::canonicalize(&exe).unwrap_or(exe);
    let s = path.to_string_lossy();
    // Homebrew installs land under .../Cellar/<formula>/<version>/bin on every
    // platform (`/opt/homebrew/Cellar`, `/usr/local/Cellar`, `/home/linuxbrew/...`).
    if s.contains("/Cellar/") {
        return InstallMethod::Homebrew;
    }
    InstallMethod::Other
}

#[derive(Serialize, Deserialize)]
struct UpdateCheckCache {
    checked_at: u64,
    latest_version: String,
}

fn cache_path() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|d| d.join(".update_check.json"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn read_cache() -> Option<UpdateCheckCache> {
    let s = fs::read_to_string(cache_path()?).ok()?;
    serde_json::from_str(&s).ok()
}

fn write_cache(cache: &UpdateCheckCache) {
    let Some(path) = cache_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(s) = serde_json::to_string(cache) {
        let _ = fs::write(path, s);
    }
}

fn fetch_latest_version() -> Result<Version, String> {
    let url = format!("https://api.github.com/repos/{REPO_OWNER}/{REPO_NAME}/releases/latest");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(NETWORK_TIMEOUT_SECS))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&url)
        .header("User-Agent", concat!("hotdata-cli/", env!("CARGO_PKG_VERSION")))
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: serde_json::Value = resp.json().map_err(|e| e.to_string())?;
    let tag = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or("no tag_name in response")?;
    Version::parse(tag.trim_start_matches('v')).map_err(|e| e.to_string())
}

/// Returns Some(latest) if a newer version is available, using the cached
/// value when fresh and refreshing it (best-effort) otherwise. Silent on errors.
fn cached_latest_if_newer() -> Option<Version> {
    let current = Version::parse(CURRENT_VERSION).ok()?;
    let cache = read_cache();
    let fresh = cache
        .as_ref()
        .map(|c| now_secs().saturating_sub(c.checked_at) < CHECK_INTERVAL_SECS)
        .unwrap_or(false);

    let latest = if fresh {
        Version::parse(&cache.as_ref()?.latest_version).ok()?
    } else {
        let v = fetch_latest_version().ok()?;
        write_cache(&UpdateCheckCache {
            checked_at: now_secs(),
            latest_version: v.to_string(),
        });
        v
    };

    (latest > current).then_some(latest)
}

fn stderr_is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stderr().is_terminal()
}

fn should_check() -> bool {
    stderr_is_tty()
        && util::is_interactive()
        && std::env::var_os("HOTDATA_NO_UPDATE_CHECK").is_none()
}

/// How long `maybe_print_update_notice` will wait for the background thread
/// before giving up.  In practice the thread finishes well within this window
/// because `fetch_latest_version` has its own 5-second HTTP timeout and cache
/// hits resolve in microseconds.
const NOTICE_WAIT_MS: u64 = 6_000;

/// Spawn a background thread that checks for a newer release.  Returns a
/// channel receiver that `maybe_print_update_notice` can poll after the
/// command runs.  No-op (returns None) when stderr isn't a TTY, `--no-input`
/// is set, or `HOTDATA_NO_UPDATE_CHECK` is set.
pub fn spawn_update_check() -> Option<std::sync::mpsc::Receiver<Option<Version>>> {
    if !should_check() {
        return None;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(cached_latest_if_newer());
    });
    Some(rx)
}

/// Poll the receiver returned by `spawn_update_check` and print a one-line
/// notice if a newer release was found.  Call this *after* the command has
/// produced its own output so the notice appears at the bottom.
pub fn maybe_print_update_notice(rx: Option<std::sync::mpsc::Receiver<Option<Version>>>) {
    let Some(rx) = rx else { return };
    let Ok(Some(latest)) = rx.recv_timeout(Duration::from_millis(NOTICE_WAIT_MS)) else {
        return;
    };
    eprintln!(
        "{}",
        format!(
            "\nA new version of hotdata is available (v{CURRENT_VERSION} → v{latest}). Run: hotdata update"
        )
        .yellow()
    );
}

pub fn run_update() {
    let current = Version::parse(CURRENT_VERSION).expect("invalid package version");

    if detect_install_method() == InstallMethod::Homebrew {
        println!("hotdata was installed via Homebrew. Update with:");
        println!("  {}", format!("brew upgrade {HOMEBREW_FORMULA}").cyan());
        return;
    }

    println!("Checking for updates...");
    let latest = match fetch_latest_version() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}", format!("error: could not check for updates: {e}").red());
            std::process::exit(1);
        }
    };

    if latest <= current {
        println!("Already up to date (v{current}).");
        // Refresh cache so the notice goes away.
        write_cache(&UpdateCheckCache {
            checked_at: now_secs(),
            latest_version: latest.to_string(),
        });
        return;
    }

    println!("Updating from v{current} to v{latest}...");
    if let Err(e) = perform_update(&latest) {
        eprintln!("{}", format!("error: update failed: {e}").red());
        std::process::exit(1);
    }
    println!("{}", format!("Updated to v{latest}.").green());

    // Install/update skills to match the new binary.  The tarball URL is built
    // from `latest` (not CURRENT_VERSION) because the old binary is still
    // running at this point — we want the skills for the version we just
    // downloaded, not the one we replaced.
    crate::skill::install_for_version(&latest);

    // Bust the cache so the notice clears on the next run.
    write_cache(&UpdateCheckCache {
        checked_at: now_secs(),
        latest_version: latest.to_string(),
    });
}

/// Download the cargo-dist tar.xz asset for the running target, unpack it,
/// and atomically swap the running binary with the new one.
fn perform_update(version: &Version) -> Result<(), String> {
    let target = self_update::get_target();
    let asset_stem = format!("{REPO_NAME}-{target}");
    let asset_name = format!("{asset_stem}.tar.xz");
    let url = format!(
        "https://github.com/{REPO_OWNER}/{REPO_NAME}/releases/download/v{version}/{asset_name}"
    );

    util::debug_request("GET", &url, &[], None);
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("http client: {e}"))?;
    let resp = client
        .get(&url)
        .header(
            "User-Agent",
            concat!("hotdata-cli/", env!("CARGO_PKG_VERSION")),
        )
        .send()
        .map_err(|e| format!("download: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {} downloading {asset_name}", resp.status()));
    }
    let xz_bytes = resp
        .bytes()
        .map_err(|e| format!("reading download: {e}"))?;

    let mut tar_bytes: Vec<u8> = Vec::with_capacity(xz_bytes.len() * 4);
    lzma_rs::xz_decompress(&mut std::io::Cursor::new(&xz_bytes[..]), &mut tar_bytes)
        .map_err(|e| format!("xz decompress: {e}"))?;

    // `tempfile::TempDir` creates a randomly-named directory with 0700
    // permissions and removes it on drop. The random suffix prevents a
    // local attacker on a shared system from pre-planting a symlink at a
    // predictable path and redirecting the extraction to a directory
    // they control.
    let tmp_dir = tempfile::TempDir::new().map_err(|e| format!("creating temp dir: {e}"))?;

    let mut archive = tar::Archive::new(std::io::Cursor::new(&tar_bytes[..]));
    archive
        .unpack(tmp_dir.path())
        .map_err(|e| format!("extract tar: {e}"))?;

    // cargo-dist lays out the tarball as `<asset-stem>/hotdata` (the binary
    // sits at the top of a single directory matching the asset name without
    // its extension).
    let new_binary = tmp_dir.path().join(&asset_stem).join("hotdata");
    if !new_binary.exists() {
        return Err(format!(
            "binary not found in archive at {}",
            new_binary.display()
        ));
    }

    let current_exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let current_exe = fs::canonicalize(&current_exe).unwrap_or(current_exe);

    // Reserve a sibling temp file on the same filesystem as the destination
    // so `Move::to_dest` can do an atomic rename.
    let backup = current_exe.with_extension("old");
    let _ = fs::remove_file(&backup);

    self_update::Move::from_source(&new_binary)
        .replace_using_temp(&backup)
        .to_dest(&current_exe)
        .map_err(|e| format!("replacing binary: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_install_method_returns_one_of_the_variants() {
        let m = detect_install_method();
        assert!(matches!(m, InstallMethod::Homebrew | InstallMethod::Other));
    }
}
