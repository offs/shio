use crate::chunk::MAX_SEGMENTS;
use crate::error::{Result, ShioError};
use crate::types::ProxyConfig;
use serde::{Deserialize, Serialize};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
const MAX_CONCURRENT: u8 = 20;
const MAX_RETRIES: u32 = 30;
const MIN_SPEED_LIMIT: u64 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
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
#[serde(default)]
pub struct WindowConfig {
    pub blur: bool,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self { blur: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
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
            upnp: false,
            seed_policy: crate::torrent::SeedPolicy::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
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
    pub delete_archive_after_extract: bool,
    pub close_to_tray: bool,
    pub torrent: TorrentConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            download_dir: default_download_dir(),
            max_concurrent: 3,
            default_segments: 8,
            max_retries: 10,
            speed_limit: None,
            clipboard_monitor: false,
            notifications: true,
            theme: ThemeConfig::default(),
            window: WindowConfig::default(),
            proxy: None,
            default_create_subfolder: true,
            default_auto_extract: false,
            delete_archive_after_extract: false,
            close_to_tray: true,
            torrent: TorrentConfig::default(),
        }
    }
}

impl AppConfig {
    pub fn load() -> Result<Self> {
        Self::load_from_path(&Self::config_path())
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        let mut content = String::new();
        std::fs::File::open(path)?
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_string(&mut content)?;
        if content.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ShioError::Config(format!(
                "{} exceeds the 1 MiB config limit",
                path.display()
            )));
        }
        // toml's errors quote the offending source (possibly the proxy password); report only the line.
        let config: Self = toml::from_str(&content).map_err(|error| {
            let line = error.span().map_or(1, |span| {
                content[..span.start.min(content.len())]
                    .matches('\n')
                    .count()
                    + 1
            });
            ShioError::Config(format!("{} line {line} is invalid", path.display()))
        })?;
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate_theme_id(&self.theme.id)?;
        if self.download_dir.as_os_str().is_empty() {
            return Err(ShioError::Config("download_dir cannot be empty".into()));
        }
        if !(1..=MAX_CONCURRENT).contains(&self.max_concurrent) {
            return Err(ShioError::Config(format!(
                "max_concurrent must be between 1 and {MAX_CONCURRENT}"
            )));
        }
        if !(1..=MAX_SEGMENTS).contains(&self.default_segments) {
            return Err(ShioError::Config(format!(
                "default_segments must be between 1 and {MAX_SEGMENTS}"
            )));
        }
        if self.max_retries > MAX_RETRIES {
            return Err(ShioError::Config(format!(
                "max_retries must be between 0 and {MAX_RETRIES}"
            )));
        }
        if self
            .speed_limit
            .is_some_and(|limit| limit < MIN_SPEED_LIMIT)
        {
            return Err(ShioError::Config(format!(
                "speed_limit must be at least {MIN_SPEED_LIMIT} bytes/s"
            )));
        }
        if self.torrent.listen_port == 0 {
            return Err(ShioError::Config(
                "torrent.listen_port must be between 1 and 65535".into(),
            ));
        }
        if let Some(proxy) = self.proxy.as_ref() {
            if proxy.host.trim().is_empty() || proxy.port == 0 {
                return Err(ShioError::Config(
                    "proxy host must be non-empty and proxy port must be between 1 and 65535"
                        .into(),
                ));
            }
            if proxy.username.is_some() != proxy.password.is_some() {
                return Err(ShioError::Config(
                    "proxy username and password must either both be set or both be absent".into(),
                ));
            }
        }
        self.torrent.seed_policy.validate()
    }

    pub fn save(&self) -> Result<()> {
        self.save_to_path(&Self::config_path())
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| ShioError::Config("config path has no parent".into()))?;
        std::fs::create_dir_all(parent)?;
        let content = toml::to_string_pretty(self).map_err(|e| ShioError::Config(e.to_string()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(content.as_bytes())?;
        temporary.as_file().sync_all()?;
        temporary.persist(path).map_err(|error| error.error)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }

    pub fn config_exists() -> bool {
        Self::config_path().exists()
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn config_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "shio").map_or_else(
            || PathBuf::from("."),
            |dirs| dirs.config_dir().to_path_buf(),
        )
    }

    pub fn theme_dir() -> PathBuf {
        Self::config_dir().join("themes")
    }

    pub fn data_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "shio")
            .map_or_else(|| PathBuf::from("."), |dirs| dirs.data_dir().to_path_buf())
    }
}

pub fn validate_theme_id(id: &str) -> Result<()> {
    let valid = !id.is_empty()
        && !id.starts_with('-')
        && !id.ends_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(ShioError::Config(format!("invalid theme id: {id}")))
    }
}

fn default_download_dir() -> PathBuf {
    directories::UserDirs::new().map_or_else(
        || PathBuf::from("Downloads"),
        |dirs| {
            dirs.download_dir()
                .map_or_else(|| dirs.home_dir().join("Downloads"), Path::to_path_buf)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation_rejects_hostile_values() {
        let mut theme = AppConfig::default();
        theme.theme.id = "GitHub Dark".to_string();
        let mut seed = AppConfig::default();
        seed.torrent.seed_policy = crate::torrent::SeedPolicy::StopAtRatio { ratio: f32::NAN };

        for config in [theme, seed] {
            assert!(config.validate().is_err());
        }
    }

    #[test]
    fn parse_errors_never_echo_config_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for line in ["password = 12345678", "password = \"hunter\\q2\""] {
            std::fs::write(&path, format!("[proxy]\nhost = \"h\"\n{line}\n")).unwrap();

            let error = AppConfig::load_from_path(&path).unwrap_err().to_string();

            assert!(error.contains("line 3"), "{error}");
            assert!(
                !error.contains("12345678") && !error.contains("hunter"),
                "{error}"
            );
        }
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = AppConfig::default();

        config.save_to_path(&path).unwrap();

        assert_eq!(AppConfig::load_from_path(&path).unwrap(), config);
    }
}
