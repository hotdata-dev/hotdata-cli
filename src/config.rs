use crossterm::style::Stylize;
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::ops::Deref;
use std::path::PathBuf;

/// Returns the config directory, defaulting to ~/.hotdata.
/// Override with HOTDATA_CONFIG_DIR env var (useful for testing).
pub fn config_dir() -> Result<PathBuf, String> {
    if let Ok(dir) = env::var("HOTDATA_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    let user_dirs = UserDirs::new().ok_or("could not determine home directory")?;
    Ok(user_dirs.home_dir().join(".hotdata"))
}

fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("config.yml"))
}

pub const DEFAULT_API_URL: &str = "https://api.hotdata.dev/v1";
pub const DEFAULT_APP_URL: &str = "https://app.hotdata.dev";

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct WorkspaceEntry {
    pub public_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppUrl(pub(crate) Option<String>);

impl Default for AppUrl {
    fn default() -> Self {
        AppUrl(None)
    }
}

impl Deref for AppUrl {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_deref().unwrap_or(DEFAULT_APP_URL)
    }
}

impl std::fmt::Display for AppUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

impl<'de> Deserialize<'de> for AppUrl {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(AppUrl(Option::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum ApiKeySource {
    #[default]
    Config,
    Env,
    Flag,
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiUrl(pub(crate) Option<String>);

impl Default for ApiUrl {
    fn default() -> Self {
        ApiUrl(None)
    }
}

impl Deref for ApiUrl {
    type Target = str;

    fn deref(&self) -> &str {
        self.0.as_deref().unwrap_or(DEFAULT_API_URL)
    }
}

impl std::fmt::Display for ApiUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self)
    }
}

impl<'de> Deserialize<'de> for ApiUrl {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(ApiUrl(Option::deserialize(deserializer)?))
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProfileConfig {
    // Transient only: populated from `--api-key` and `HOTDATA_API_KEY`,
    // never persisted to or read from YAML. Auth state on disk lives
    // entirely in session.json.
    #[serde(skip)]
    pub api_key: Option<String>,
    #[serde(skip)]
    pub api_url: ApiUrl,
    #[serde(skip)]
    pub app_url: AppUrl,
    #[serde(skip)]
    pub api_key_source: ApiKeySource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "session")]
    pub sandbox: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigFile {
    pub profiles: HashMap<String, ProfileConfig>,
}

fn write_config(config_path: &std::path::Path, content: &str) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("error creating config directory: {e}"))?;
    }
    fs::write(config_path, content).map_err(|e| format!("error writing config file: {e}"))
}

/// Wipe the workspace cache for a profile. Paired with
/// `jwt::clear_session()` in `auth::logout` — together they reset the
/// on-disk state that login populates.
pub fn clear_workspaces(profile: &str) -> Result<(), String> {
    let config_path = config_path()?;

    if !config_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("error reading config file: {e}"))?;
    let mut config_file: ConfigFile =
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?;

    if let Some(entry) = config_file.profiles.get_mut(profile) {
        entry.workspaces.clear();
    }

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;
    write_config(&config_path, &content)
}

pub fn save_workspaces(profile: &str, workspaces: Vec<WorkspaceEntry>) -> Result<(), String> {
    let config_path = config_path()?;

    let mut config_file: ConfigFile = if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("error reading config file: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?
    } else {
        ConfigFile {
            profiles: HashMap::new(),
        }
    };

    config_file
        .profiles
        .entry(profile.to_string())
        .or_default()
        .workspaces = workspaces;

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;

    write_config(&config_path, &content)
}

pub fn save_default_workspace(profile: &str, workspace: WorkspaceEntry) -> Result<(), String> {
    let config_path = config_path()?;

    let mut config_file: ConfigFile = if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("error reading config file: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?
    } else {
        ConfigFile { profiles: HashMap::new() }
    };

    let entry = config_file.profiles.entry(profile.to_string()).or_default();
    entry.workspaces.retain(|w| w.public_id != workspace.public_id);
    entry.workspaces.insert(0, workspace);

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;
    write_config(&config_path, &content)
}

pub fn save_sandbox(profile: &str, sandbox_id: &str) -> Result<(), String> {
    let config_path = config_path()?;

    let mut config_file: ConfigFile = if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .map_err(|e| format!("error reading config file: {e}"))?;
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?
    } else {
        ConfigFile { profiles: HashMap::new() }
    };

    config_file
        .profiles
        .entry(profile.to_string())
        .or_default()
        .sandbox = Some(sandbox_id.to_string());

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;
    write_config(&config_path, &content)
}

pub fn clear_sandbox(profile: &str) -> Result<(), String> {
    let config_path = config_path()?;

    if !config_path.exists() {
        return Ok(());
    }

    let content = fs::read_to_string(&config_path)
        .map_err(|e| format!("error reading config file: {e}"))?;
    let mut config_file: ConfigFile =
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?;

    if let Some(entry) = config_file.profiles.get_mut(profile) {
        entry.sandbox = None;
    }

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;
    write_config(&config_path, &content)
}

pub fn resolve_workspace_id(provided: Option<String>, profile_config: &ProfileConfig) -> Result<String, String> {
    if let Some(id) = provided {
        return Ok(id);
    }
    profile_config
        .workspaces
        .first()
        .map(|w| w.public_id.clone())
        .ok_or_else(|| "no workspace-id provided and no default workspace found. Run 'hotdata auth login' (or 'hotdata auth') or specify --workspace-id.".to_string())
}

/// Global API key override set via --api-key flag.
/// Call `set_api_key_flag` once at startup; `load` picks it up automatically.
static API_KEY_FLAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn set_api_key_flag(key: String) {
    let _ = API_KEY_FLAG.set(key);
}

pub fn load(profile: &str) -> Result<ProfileConfig, String> {
    let config_file = config_path()?;

    let mut profile_config = if config_file.exists() {
        let content =
            fs::read_to_string(&config_file).map_err(|e| format!("error reading config file: {e}"))?;
        let config_file: ConfigFile = serde_yaml::from_str(&content).unwrap_or_else(|_| {
            eprintln!("{}", "error parsing config file.".red());
            eprintln!("Run 'hotdata auth login' (or 'hotdata auth') to generate a new config file.");
            std::process::exit(1);
        });
        config_file.profiles.get(profile).cloned().unwrap_or_default()
    } else {
        ProfileConfig::default()
    };

    // Priority: config (lowest) < env var < --api-key flag (highest)
    if let Ok(val) = env::var("HOTDATA_API_KEY") {
        profile_config.api_key = Some(val);
        profile_config.api_key_source = ApiKeySource::Env;
    }

    if let Some(val) = API_KEY_FLAG.get() {
        profile_config.api_key = Some(val.clone());
        profile_config.api_key_source = ApiKeySource::Flag;
    }

    if let Ok(val) = env::var("HOTDATA_API_URL") {
        profile_config.api_url = ApiUrl(Some(val));
    }

    if let Ok(val) = env::var("HOTDATA_APP_URL") {
        profile_config.app_url = AppUrl(Some(val));
    }

    Ok(profile_config)
}

/// Test utilities shared across modules.
#[cfg(test)]
pub mod test_helpers {
    use std::sync::Mutex;

    // Serialize all tests that modify HOTDATA_CONFIG_DIR env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Set HOTDATA_CONFIG_DIR to a temp dir and return it with a lock guard.
    /// Hold the guard for the duration of the test.
    pub fn with_temp_config_dir() -> (tempfile::TempDir, std::sync::MutexGuard<'static, ()>) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        // SAFETY: tests are serialized via ENV_LOCK mutex, so no concurrent env mutation.
        unsafe { std::env::set_var("HOTDATA_CONFIG_DIR", tmp.path()) };
        (tmp, guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_helpers::with_temp_config_dir;

    fn ws(id: &str, name: &str) -> WorkspaceEntry {
        WorkspaceEntry { public_id: id.into(), name: name.into() }
    }

    #[test]
    fn save_workspaces_creates_config_dir() {
        let (_tmp, _guard) = with_temp_config_dir();

        let path = config_path().unwrap();
        assert!(!path.exists());

        save_workspaces("default", vec![ws("ws-1", "WS")]).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn clear_workspaces_empties_the_list() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-1", "Test WS")]).unwrap();

        clear_workspaces("default").unwrap();

        let profile = load("default").unwrap();
        assert!(profile.workspaces.is_empty());
    }

    #[test]
    fn clear_workspaces_noop_when_no_config() {
        let (_tmp, _guard) = with_temp_config_dir();
        assert!(clear_workspaces("default").is_ok());
    }

    #[test]
    fn save_and_load_workspaces() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-1", "First"), ws("ws-2", "Second")]).unwrap();

        let profile = load("default").unwrap();
        assert_eq!(profile.workspaces.len(), 2);
        assert_eq!(profile.workspaces[0].public_id, "ws-1");
        assert_eq!(profile.workspaces[1].name, "Second");
    }

    #[test]
    fn save_default_workspace_moves_to_front() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-1", "First"), ws("ws-2", "Second")]).unwrap();

        // Set ws-2 as default — should move to front
        save_default_workspace("default", ws("ws-2", "Second")).unwrap();

        let profile = load("default").unwrap();
        assert_eq!(profile.workspaces[0].public_id, "ws-2");
        assert_eq!(profile.workspaces[1].public_id, "ws-1");
    }

    #[test]
    fn load_missing_profile_returns_default() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-1", "WS")]).unwrap();

        let profile = load("nonexistent").unwrap();
        assert_eq!(profile.api_key, None);
        assert!(profile.workspaces.is_empty());
    }

    #[test]
    fn load_no_config_file_returns_default() {
        let (_tmp, _guard) = with_temp_config_dir();

        let profile = load("default").unwrap();
        assert_eq!(profile.api_key, None);
    }

    #[test]
    fn multiple_profiles_keep_independent_workspaces() {
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-default", "Default WS")]).unwrap();
        save_workspaces("staging", vec![ws("ws-staging", "Staging WS")]).unwrap();

        let default = load("default").unwrap();
        let staging = load("staging").unwrap();
        assert_eq!(default.workspaces[0].public_id, "ws-default");
        assert_eq!(staging.workspaces[0].public_id, "ws-staging");
    }

    #[test]
    fn legacy_api_key_in_yaml_is_ignored_on_load() {
        // Older configs (pre-jwt-branch) had `api_key: hd_xxx` written
        // to disk. After the migration, the api_key field is purely
        // transient — `#[serde(skip)]` must drop any value present in
        // YAML on load. This pins down the migration behavior so a
        // stale entry can't silently reappear in profile.api_key.
        let (_tmp, _guard) = with_temp_config_dir();
        let path = config_path().unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "profiles:\n  default:\n    api_key: legacy-hd-token\n",
        )
        .unwrap();

        let profile = load("default").unwrap();
        assert_eq!(profile.api_key, None);
    }

    #[test]
    fn save_does_not_persist_transient_api_key() {
        // Even if api_key was set in-memory (e.g. via env var), saving
        // workspaces must NOT round-trip api_key into YAML.
        let (_tmp, _guard) = with_temp_config_dir();
        save_workspaces("default", vec![ws("ws-1", "WS")]).unwrap();

        let yaml = fs::read_to_string(config_path().unwrap()).unwrap();
        assert!(
            !yaml.contains("api_key"),
            "api_key must not appear in YAML, got:\n{yaml}"
        );
    }

    #[test]
    fn resolve_workspace_id_prefers_provided() {
        let profile = ProfileConfig {
            workspaces: vec![WorkspaceEntry { public_id: "ws-1".into(), name: "WS".into() }],
            ..Default::default()
        };
        let result = resolve_workspace_id(Some("explicit-id".into()), &profile).unwrap();
        assert_eq!(result, "explicit-id");
    }

    #[test]
    fn resolve_workspace_id_falls_back_to_first() {
        let profile = ProfileConfig {
            workspaces: vec![WorkspaceEntry { public_id: "ws-1".into(), name: "WS".into() }],
            ..Default::default()
        };
        let result = resolve_workspace_id(None, &profile).unwrap();
        assert_eq!(result, "ws-1");
    }

    #[test]
    fn resolve_workspace_id_errors_when_none() {
        let profile = ProfileConfig::default();
        let result = resolve_workspace_id(None, &profile);
        assert!(result.is_err());
    }
}
