//! This device's parsed saves, persisted as JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dexsync_core::SaveInfo;
use dexsync_core::collection::{self, Collection, SaveRef};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Id of the source the save was found in.
    pub source: String,
    pub info: SaveInfo,
    /// Unix seconds when the dex last changed.
    pub updated: u64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Keyed by save file path.
    pub saves: BTreeMap<PathBuf, Record>,
}

/// What changed in one save since it was last recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub source: String,
    pub info: SaveInfo,
    /// First time this save was seen.
    pub first: bool,
    pub new_owned: Vec<u16>,
    pub new_seen: Vec<u16>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("{0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("{0}: {1}")]
    Json(PathBuf, serde_json::Error),
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Label for this machine's saves in a collection.
pub fn device_name() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .ok()
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this-device".into())
}

fn added(new: &[u16], old: &[u16]) -> Vec<u16> {
    new.iter().copied().filter(|n| old.binary_search(n).is_err()).collect()
}

impl State {
    /// Default location: the platform data dir, e.g. `~/.local/share/dexsync/state.json`.
    pub fn default_path() -> Option<PathBuf> {
        directories::ProjectDirs::from("", "", "dexsync").map(|d| d.data_dir().join("state.json"))
    }

    pub fn load(path: &Path) -> Result<State, StoreError> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| StoreError::Json(path.into(), e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(StoreError::Io(path.into(), e)),
        }
    }

    /// Write atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> Result<(), StoreError> {
        let io = |e| StoreError::Io(path.into(), e);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).expect("state always serialises")).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Merge every recorded save into a [`Collection`] labelled with `device`.
    pub fn collection(&self, device: &str) -> Collection {
        let files: Vec<String> = self.saves.keys().map(|p| p.display().to_string()).collect();
        let refs = self.saves.values().zip(&files).map(|(r, file)| SaveRef { device, file, info: &r.info });
        collection::merge(refs, now())
    }

    /// Record a freshly parsed save. Returns `None` if nothing changed.
    pub fn apply(&mut self, source: &str, path: PathBuf, info: SaveInfo) -> Option<Change> {
        let old = self.saves.get(&path);
        if old.is_some_and(|r| r.info == info && r.source == source) {
            return None;
        }
        let (old_owned, old_seen) = old.map(|r| (&r.info.owned[..], &r.info.seen[..])).unwrap_or_default();
        let change = Change {
            path: path.clone(),
            source: source.into(),
            first: old.is_none(),
            new_owned: added(&info.owned, old_owned),
            new_seen: added(&info.seen, old_seen),
            info: info.clone(),
        };
        self.saves.insert(path, Record { source: source.into(), info, updated: now() });
        Some(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dexsync_core::Game;

    fn info(owned: &[u16], seen: &[u16]) -> SaveInfo {
        SaveInfo { game: Game::Emerald, generation: 3, trainer: "NATE".into(), tid: 1, sid: 2, owned: owned.into(), seen: seen.into() }
    }

    #[test]
    fn apply_reports_new_entries_only() {
        let mut s = State::default();
        let c = s.apply("ra", "a.sav".into(), info(&[252], &[252, 263])).unwrap();
        assert!(c.first);
        assert!(s.apply("ra", "a.sav".into(), info(&[252], &[252, 263])).is_none());
        let c = s.apply("ra", "a.sav".into(), info(&[252, 263], &[252, 261, 263])).unwrap();
        assert!(!c.first);
        assert_eq!((c.new_owned, c.new_seen), (vec![263], vec![261]));
    }

    #[test]
    fn round_trips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x/state.json");
        let mut s = State::default();
        s.apply("ra", "a.sav".into(), info(&[1], &[1]));
        s.save(&path).unwrap();
        assert_eq!(State::load(&path).unwrap(), s);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
