use crossterm::style::Stylize;
use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::ops::Deref;

pub const DEFAULT_API_URL: &str = "https://api.hotdata.dev/v1";
pub const DEFAULT_APP_URL: &str = "https://app.hotdata.dev";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkspaceEntry {
    pub public_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AppUrl(Option<String>);

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
}

#[derive(Debug, Clone, Serialize)]
pub struct ApiUrl(Option<String>);

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
    pub api_key: Option<String>,
    #[serde(skip)]
    pub api_url: ApiUrl,
    #[serde(skip)]
    pub app_url: AppUrl,
    #[serde(skip)]
    pub api_key_source: ApiKeySource,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<WorkspaceEntry>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigFile {
    pub profiles: HashMap<String, ProfileConfig>,
}

pub fn save_api_key(profile: &str, api_key: &str) -> Result<(), String> {
    let user_dirs = UserDirs::new().ok_or("could not determine home directory")?;
    let config_path = user_dirs.home_dir().join(".hotdata").join("config.yml");

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
        .api_key = Some(api_key.to_string());

    let content = serde_yaml::to_string(&config_file)
        .map_err(|e| format!("error serializing config: {e}"))?;

    fs::write(&config_path, content).map_err(|e| format!("error writing config file: {e}"))
}

pub fn save_workspaces(profile: &str, workspaces: Vec<WorkspaceEntry>) -> Result<(), String> {
    let user_dirs = UserDirs::new().ok_or("could not determine home directory")?;
    let config_path = user_dirs.home_dir().join(".hotdata").join("config.yml");

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

    fs::write(&config_path, content).map_err(|e| format!("error writing config file: {e}"))
}

pub fn save_default_workspace(profile: &str, workspace: WorkspaceEntry) -> Result<(), String> {
    let user_dirs = UserDirs::new().ok_or("could not determine home directory")?;
    let config_path = user_dirs.home_dir().join(".hotdata").join("config.yml");

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
    fs::write(&config_path, content).map_err(|e| format!("error writing config file: {e}"))
}

pub fn resolve_workspace_id(provided: Option<String>, profile_config: &ProfileConfig) -> Result<String, String> {
    if let Some(id) = provided {
        return Ok(id);
    }
    profile_config
        .workspaces
        .first()
        .map(|w| w.public_id.clone())
        .ok_or_else(|| "no workspace-id provided and no default workspace found. Run 'hotdata auth login' or specify --workspace-id.".to_string())
}

pub fn load(profile: &str) -> Result<ProfileConfig, String> {
    let user_dirs = UserDirs::new().ok_or("could not determine home directory")?;
    let config_file = user_dirs.home_dir().join(".hotdata").join("config.yml");

    if !config_file.exists() {
        return Err(format!(
            "config file not found at {}. Run 'hotdata init' to create one.",
            config_file.display()
        ));
    }

    let content =
        fs::read_to_string(&config_file).map_err(|e| format!("error reading config file: {e}"))?;

    let config_file: ConfigFile = serde_yaml::from_str(&content).unwrap_or_else(|_| {
        eprintln!("{}", "error parsing config file.".red());
        eprintln!("Run 'hotdata auth login' to generate a new config file.");
        std::process::exit(1);
    });

    let mut profile_config = config_file
        .profiles
        .get(profile)
        .cloned()
        .ok_or_else(|| format!("profile '{profile}' not found in config"))?;

    if let Ok(val) = env::var("HOTDATA_API_KEY") {
        profile_config.api_key = Some(val);
        profile_config.api_key_source = ApiKeySource::Env;
    }

    if let Ok(val) = env::var("HOTDATA_API_URL") {
        profile_config.api_url = ApiUrl(Some(val));
    }

    if let Ok(val) = env::var("HOTDATA_APP_URL") {
        profile_config.app_url = AppUrl(Some(val));
    }

    Ok(profile_config)
}
