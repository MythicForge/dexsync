//! The user's configured sources, stored as TOML.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Preset;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// Short unique name, e.g. `retroarch` or `azahar-sd`.
    pub id: String,
    pub preset: Preset,
    pub path: PathBuf,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// Folder of `{id}.png` sprites for the UI (e.g. the handheld Pokédex's
    /// `data/sprites`). Not bundled: sprites are Nintendo artwork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sprites: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "SyncConfig::is_default")]
    pub sync: SyncConfig,
    #[serde(default, rename = "source")]
    pub sources: Vec<Source>,
}

/// GitHub sync settings. The token itself lives in the OS keychain, never here.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncConfig {
    /// GitHub App Client ID; overrides the one built into DexSync
    /// (for forks, or anyone who registers their own app).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The GitHub App's URL name (github.com/apps/<slug>), for the install link.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_slug: Option<String>,
    /// `owner/name` of the user's private sync repo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Also upload raw save files (opt-in).
    #[serde(default)]
    pub backups: bool,
    /// Name for this device in the repo; defaults to the hostname.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
}

impl SyncConfig {
    fn is_default(&self) -> bool {
        *self == SyncConfig::default()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Parse(PathBuf, toml::de::Error),
    #[error("a source named '{0}' already exists")]
    DuplicateId(String),
    #[error("no source named '{0}'")]
    UnknownId(String),
}

impl Config {
    /// Default location: the platform config dir, e.g. `~/.config/dexsync/config.toml`.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "dexsync").map(|d| d.config_dir().join("config.toml"))
    }

    /// Load, or return an empty config if the file doesn't exist yet.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| ConfigError::Parse(path.into(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(ConfigError::Io(path.into(), e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let io = |e| ConfigError::Io(path.into(), e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let text = toml::to_string_pretty(self).expect("config always serialises");
        std::fs::write(path, text).map_err(io)
    }

    pub fn add(&mut self, source: Source) -> Result<(), ConfigError> {
        if self.sources.iter().any(|s| s.id == source.id) {
            return Err(ConfigError::DuplicateId(source.id));
        }
        self.sources.push(source);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<Source, ConfigError> {
        let i = self.sources.iter().position(|s| s.id == id).ok_or_else(|| ConfigError::UnknownId(id.into()))?;
        Ok(self.sources.remove(i))
    }

    /// A free id based on the preset name: `retroarch`, `retroarch-2`, ...
    pub fn next_id(&self, preset: Preset) -> String {
        let base = preset.id();
        (1..)
            .map(|n| if n == 1 { base.to_string() } else { format!("{base}-{n}") })
            .find(|id| self.sources.iter().all(|s| &s.id != id))
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_names_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub/config.toml");
        let mut c = Config::load(&file).unwrap();
        assert!(c.sources.is_empty());

        let id = c.next_id(Preset::RetroArch);
        c.add(Source { id, preset: Preset::RetroArch, path: "/a".into() }).unwrap();
        assert_eq!(c.next_id(Preset::RetroArch), "retroarch-2");
        assert!(c.add(Source { id: "retroarch".into(), preset: Preset::Mgba, path: "/b".into() }).is_err());
        c.save(&file).unwrap();

        let loaded = Config::load(&file).unwrap();
        assert_eq!(loaded, c);
        assert!(std::fs::read_to_string(&file).unwrap().contains("[[source]]"));

        c.sync.repo = Some("me/dexsync-data".into());
        c.sprites = Some("/sprites".into());
        c.save(&file).unwrap();
        assert_eq!(Config::load(&file).unwrap(), c);
    }
}
