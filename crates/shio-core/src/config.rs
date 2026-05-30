use crate::error::{Result, ShioError};
use crate::types::ProxyConfig;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WindowMaterialPreference {
    Acrylic,
    Solid,
}

impl WindowMaterialPreference {
    pub const ALL: [Self; 2] = [Self::Acrylic, Self::Solid];
}

impl std::fmt::Display for WindowMaterialPreference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Acrylic => "acrylic",
            Self::Solid => "solid",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub id: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            id: "dark".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowConfig {
    pub material: WindowMaterialPreference,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            material: WindowMaterialPreference::Acrylic,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TorrentConfig {
    pub listen_port: u16,
    pub dht: bool,
    pub upnp: bool,
    pub seed_policy: crate::torrent::SeedPolicy,
}

impl Default for TorrentConfig {
    fn default() -> Self {
        Self {
            listen_port: 6881,
            dht: true,
            upnp: true,
            seed_policy: crate::torrent::SeedPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    pub download_dir: PathBuf,
    pub max_concurrent: u8,
    pub default_segments: u8,
    pub max_retries: u32,
    pub speed_limit: Option<u64>,
    pub clipboard_monitor: bool,
    pub notifications: bool,
    pub theme: ThemeConfig,
    pub window: WindowConfig,
    pub proxy: Option<ProxyConfig>,
    pub default_create_subfolder: bool,
    pub default_auto_extract: bool,
    pub extract_to_subfolder: bool,
    pub delete_archive_after_extract: bool,
    pub close_to_tray: bool,
    pub scroll_long_names: bool,
    pub torrent: TorrentConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        let download_dir = dirs_download_dir();

        Self {
            download_dir,
            max_concurrent: 3,
            default_segments: 8,
            max_retries: 3,
            speed_limit: None,
            clipboard_monitor: true,
            notifications: true,
            theme: ThemeConfig::default(),
            window: WindowConfig::default(),
            proxy: None,
            default_create_subfolder: true,
            default_auto_extract: true,
            extract_to_subfolder: true,
            delete_archive_after_extract: false,
            close_to_tray: true,
            scroll_long_names: false,
            torrent: TorrentConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| ShioError::Config(format!("parse {}: {}", path.display(), e)))?;
        config.validate()
    }

    fn validate(self) -> Result<Self> {
        validate_theme_id(&self.theme.id)?;
        self.torrent
            .seed_policy
            .validate()
            .map_err(|e| ShioError::Config(format!("invalid torrent seed policy: {e}")))?;
        Ok(self)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        self.save_to_path(&path)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| ShioError::Config("config path has no parent".into()))?;
        std::fs::create_dir_all(parent).map_err(ShioError::Io)?;
        let content = toml::to_string_pretty(self).map_err(|e| ShioError::Config(e.to_string()))?;
        let tmp = path.with_extension("toml.tmp");
        {
            let mut file = std::fs::File::create(&tmp).map_err(ShioError::Io)?;
            file.write_all(content.as_bytes()).map_err(ShioError::Io)?;
            file.sync_all().map_err(ShioError::Io)?;
        }
        std::fs::rename(&tmp, path).map_err(ShioError::Io)?;
        tracing::debug!("Config saved to {}", path.display());
        Ok(())
    }

    pub fn config_exists() -> bool {
        Self::config_path().exists()
    }

    pub fn config_path() -> PathBuf {
        if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "shio") {
            proj_dirs.config_dir().join("config.toml")
        } else {
            PathBuf::from("shio_config.toml")
        }
    }

    pub fn config_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "shio").map_or_else(
            || PathBuf::from("."),
            |proj_dirs| proj_dirs.config_dir().to_path_buf(),
        )
    }

    pub fn theme_dir() -> PathBuf {
        Self::config_dir().join("themes")
    }

    pub fn data_dir() -> PathBuf {
        if let Some(proj_dirs) = directories::ProjectDirs::from("", "", "shio") {
            proj_dirs.data_dir().to_path_buf()
        } else {
            PathBuf::from(".")
        }
    }
}

pub fn validate_theme_id(id: &str) -> Result<()> {
    if id.is_empty() {
        return Err(ShioError::Config("theme id cannot be empty".to_string()));
    }
    if id.starts_with('-') || id.ends_with('-') {
        return Err(ShioError::Config(format!("invalid theme id: {id}")));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err(ShioError::Config(format!("invalid theme id: {id}")));
    }
    Ok(())
}

fn dirs_download_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|dirs| dirs.download_dir().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| {
            directories::UserDirs::new().map_or_else(
                || PathBuf::from("downloads"),
                |dirs| dirs.home_dir().join("Downloads"),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_toml_roundtrips() {
        let original = AppConfig::default();
        let serialized = toml::to_string(&original).expect("serialize default");
        let parsed: AppConfig = toml::from_str(&serialized).expect("parse own output");
        assert_eq!(parsed, original);
    }

    #[test]
    fn default_has_sane_values() {
        let cfg = AppConfig::default();
        assert!(cfg.max_concurrent >= 1);
        assert!(cfg.default_segments >= 1);
        assert!(!cfg.download_dir.as_os_str().is_empty());
        assert_eq!(cfg.theme, ThemeConfig::default());
        assert!(!cfg.scroll_long_names);
    }

    #[test]
    fn default_theme_config_uses_typed_defaults() {
        let cfg = AppConfig::default();

        assert_eq!(cfg.theme.id, "dark");
        assert_eq!(cfg.window.material, WindowMaterialPreference::Acrylic);
    }

    #[test]
    fn old_theme_string_config_is_rejected() {
        let mut value = toml::Value::try_from(AppConfig::default()).expect("serialize default");
        let table = value.as_table_mut().expect("default config is a table");
        table.insert("theme".to_string(), toml::Value::String("dark".to_string()));

        assert!(value.try_into::<AppConfig>().is_err());
    }

    #[test]
    fn invalid_theme_ids_are_rejected() {
        let mut config = AppConfig::default();
        config.theme.id = "GitHub Dark".to_string();

        assert!(config.validate().is_err());
    }

    #[test]
    fn malformed_toml_returns_error() {
        let bad = "this is not = [valid toml";
        assert!(toml::from_str::<AppConfig>(bad).is_err());
    }

    #[test]
    fn missing_required_field_returns_error() {
        let bad = "max_concurrent = 3";
        assert!(toml::from_str::<AppConfig>(bad).is_err());
    }

    #[test]
    fn torrent_config_default_is_standard() {
        let c = TorrentConfig::default();
        assert_eq!(c.listen_port, 6881);
        assert!(c.dht);
        assert!(c.upnp);
    }

    #[test]
    fn config_validation_rejects_invalid_seed_policy() {
        let mut config = AppConfig::default();
        config.torrent.seed_policy = crate::torrent::SeedPolicy::StopAtRatio { ratio: f32::NAN };

        assert!(config.validate().is_err());
    }

    #[test]
    fn save_writes_valid_config_through_temp_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        let config = AppConfig::default();

        config.save_to_path(&path).expect("save config");

        assert_eq!(
            AppConfig::load_from_path(&path).expect("load saved config"),
            config
        );
        assert!(!path.with_extension("toml.tmp").exists());
    }
}
