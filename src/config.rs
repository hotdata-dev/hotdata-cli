use directories::UserDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::ops::Deref;

pub const DEFAULT_API_URL: &str = "https://api.hotdata.dev/v1";

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

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ProfileConfig {
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_url: ApiUrl,
    #[serde(skip)]
    pub api_key_source: ApiKeySource,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ConfigFile {
    pub profiles: HashMap<String, ProfileConfig>,
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

    let config_file: ConfigFile =
        serde_yaml::from_str(&content).map_err(|e| format!("error parsing config file: {e}"))?;

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

    Ok(profile_config)
}
