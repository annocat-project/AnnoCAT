use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Default, Deserialize, Serialize)]
pub(crate) struct AppConfig {
    pub(crate) resource_directory: Option<PathBuf>,
    pub(crate) downloads_directory: Option<PathBuf>,
    pub(crate) results_directory: Option<PathBuf>,
    #[serde(default)]
    pub(crate) favor_enabled: Option<bool>,
}

pub(crate) fn config_file(home: &Path) -> PathBuf {
    home.join("config").join("annocat.json")
}

pub(crate) fn load_config(home: &Path) -> Result<AppConfig, String> {
    let file = config_file(home);
    if !file.exists() {
        return Ok(AppConfig::default());
    }
    let contents = std::fs::read_to_string(&file)
        .map_err(|error| format!("cannot read {}: {error}", file.display()))?;
    serde_json::from_str(&contents)
        .map_err(|error| format!("invalid configuration {}: {error}", file.display()))
}

pub(crate) fn save_config(home: &Path, config: &AppConfig) -> Result<(), String> {
    let directory = home.join("config");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let contents = serde_json::to_string_pretty(config)
        .map_err(|error| format!("cannot serialize configuration: {error}"))?;
    let file = config_file(home);
    std::fs::write(&file, format!("{contents}\n"))
        .map_err(|error| format!("cannot write {}: {error}", file.display()))
}
