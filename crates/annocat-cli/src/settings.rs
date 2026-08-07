use serde::{Deserialize, Serialize};
use std::path::Component;
use std::path::{Path, PathBuf};

#[derive(Clone, Default, Deserialize, Serialize)]
pub(crate) struct AppConfig {
    pub(crate) resource_directory: Option<PathBuf>,
    pub(crate) downloads_directory: Option<PathBuf>,
    pub(crate) results_directory: Option<PathBuf>,
    #[serde(default)]
    pub(crate) favor_enabled: Option<bool>,
}

pub(crate) struct ResolvedDirectories {
    pub(crate) resource_directory: PathBuf,
    pub(crate) downloads_directory: PathBuf,
    pub(crate) results_directory: PathBuf,
}

pub(crate) fn resolve_directories(
    home: &Path,
    config: &AppConfig,
) -> Result<ResolvedDirectories, String> {
    let configured_resource = resolve_directory(
        home,
        config.resource_directory.as_deref(),
        &home.join("resources"),
    )?;
    let legacy_resource_root = config
        .resource_directory
        .is_some()
        .then_some(configured_resource.as_path())
        .filter(|path| {
            !contains_annotation_data(path) && contains_annotation_data(&path.join("resources"))
        });
    let resource_directory = legacy_resource_root
        .map(|root| root.join("resources"))
        .unwrap_or_else(|| configured_resource.clone());
    let downloads_directory = resolve_directory(
        home,
        config.downloads_directory.as_deref(),
        &legacy_resource_root
            .map(|root| root.join("downloads"))
            .unwrap_or_else(|| home.join("downloads")),
    )?;
    let results_directory = resolve_directory(
        home,
        config.results_directory.as_deref(),
        &home.join("runs"),
    )?;
    Ok(ResolvedDirectories {
        resource_directory,
        downloads_directory,
        results_directory,
    })
}

fn contains_annotation_data(path: &Path) -> bool {
    [
        "reference",
        "transcript-cache",
        "dbnsfp",
        "clinvar",
        "dbsnp",
        "gnomad",
        "gnomad-genomes",
        "cadd",
        "phylop",
        "spliceai",
        "hpo",
        "reactome",
    ]
    .iter()
    .any(|name| path.join(name).is_dir())
}

pub(crate) fn stored_directory(
    home: &Path,
    selected: &Path,
    default: &Path,
) -> Result<Option<PathBuf>, String> {
    let selected = resolve_directory(home, Some(selected), selected)?;
    if selected == default {
        return Ok(None);
    }
    let Ok(relative) = selected.strip_prefix(home) else {
        return Ok(Some(selected));
    };
    Ok(Some(if relative.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        relative.to_path_buf()
    }))
}

fn resolve_directory(
    home: &Path,
    configured: Option<&Path>,
    default: &Path,
) -> Result<PathBuf, String> {
    let Some(configured) = configured else {
        return Ok(default.to_path_buf());
    };
    if configured.is_absolute() {
        return Ok(configured.to_path_buf());
    }
    if configured
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "configured folder must stay inside the AnnoCAT folder: {}",
            configured.display()
        ));
    }
    Ok(configured
        .components()
        .fold(home.to_path_buf(), |path, component| match component {
            Component::Normal(value) => path.join(value),
            Component::CurDir => path,
            _ => unreachable!("validated relative directory component"),
        }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portable_directories_follow_the_application_folder() {
        let home = std::env::temp_dir().join("annocat-portable-home");
        let defaults = resolve_directories(&home, &AppConfig::default()).unwrap();
        assert_eq!(defaults.resource_directory, home.join("resources"));
        assert_eq!(defaults.downloads_directory, home.join("downloads"));
        assert_eq!(defaults.results_directory, home.join("runs"));

        assert_eq!(
            stored_directory(&home, &home.join("runs"), &home.join("runs")).unwrap(),
            None
        );
        assert_eq!(
            stored_directory(&home, &home.join("storage"), &home.join("runs")).unwrap(),
            Some(PathBuf::from("storage"))
        );
        assert!(resolve_directory(&home, Some(Path::new("../outside")), &home).is_err());
        assert_eq!(
            resolve_directory(&home, Some(Path::new(".")), &home).unwrap(),
            home
        );

        let external = home.parent().unwrap().join("annocat-external");
        assert_eq!(
            stored_directory(&home, &external, &home.join("runs")).unwrap(),
            Some(external.clone())
        );

        let config = AppConfig {
            resource_directory: Some(PathBuf::from("storage")),
            downloads_directory: None,
            results_directory: Some(PathBuf::from("results")),
            favor_enabled: None,
        };
        let moved_home = home.with_file_name("annocat-portable-home-moved");
        let moved = resolve_directories(&moved_home, &config).unwrap();
        assert_eq!(moved.resource_directory, moved_home.join("storage"));
        assert_eq!(moved.downloads_directory, moved_home.join("downloads"));
        assert_eq!(moved.results_directory, moved_home.join("results"));

        let external_config = AppConfig {
            resource_directory: Some(external.clone()),
            downloads_directory: None,
            results_directory: None,
            favor_enabled: None,
        };
        let moved = resolve_directories(&moved_home, &external_config).unwrap();
        assert_eq!(moved.resource_directory, external);
        assert_eq!(moved.downloads_directory, moved_home.join("downloads"));
    }

    #[test]
    fn resource_directory_accepts_direct_and_legacy_layouts() {
        let root =
            std::env::temp_dir().join(format!("annocat-resource-layout-{}", std::process::id()));
        let home = root.join("home");
        let direct = root.join("direct-resources");
        std::fs::create_dir_all(direct.join("reference")).unwrap();
        std::fs::create_dir_all(direct.join("resources")).unwrap();
        let direct_config = AppConfig {
            resource_directory: Some(direct.clone()),
            ..AppConfig::default()
        };
        let resolved = resolve_directories(&home, &direct_config).unwrap();
        assert_eq!(resolved.resource_directory, direct);
        assert_eq!(resolved.downloads_directory, home.join("downloads"));

        let legacy = root.join("legacy-root");
        std::fs::create_dir_all(legacy.join("resources").join("reference")).unwrap();
        let legacy_config = AppConfig {
            resource_directory: Some(legacy.clone()),
            ..AppConfig::default()
        };
        let resolved = resolve_directories(&home, &legacy_config).unwrap();
        assert_eq!(resolved.resource_directory, legacy.join("resources"));
        assert_eq!(resolved.downloads_directory, legacy.join("downloads"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
