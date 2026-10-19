//! Sync this device's parsed saves through the user's private GitHub repo.
//!
//! Repo layout (a public contract, see research.md §10):
//! ```text
//! devices/<device>.json          this device's saves (each device writes only its own)
//! collection.json                merged view of every device, regenerated on sync
//! saves/<device>/<source>/<file> raw save backups (opt-in)
//! ```

pub mod github;
pub mod token;

pub use github::{DeviceCode, GitHub, poll_device_flow, start_device_flow};
pub use token::TokenStore;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use dexsync_core::SaveInfo;
use dexsync_core::collection::{self, Collection, SaveRef};
use dexsync_watcher::State;
use serde::{Deserialize, Serialize};

/// Client ID of the public DexSync GitHub App. `None` until it's registered;
/// users can always set their own in `config.toml` (`[sync] client_id`).
pub const DEFAULT_CLIENT_ID: Option<&str> = None;
/// URL name of the public DexSync GitHub App (github.com/apps/<slug>).
pub const DEFAULT_APP_SLUG: Option<&str> = None;

pub const SCHEMA: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("network: {0}")]
    Http(String),
    #[error("{0}")]
    Auth(String),
    #[error("no access to {0}: check the repo name, and that your token (or the DexSync app) can reach that repo")]
    NoAccess(String),
    #[error("GitHub returned {0} for {1}")]
    Status(u16, String),
    #[error("{0} changed on GitHub while syncing; try again")]
    Conflict(String),
    #[error("cancelled")]
    Cancelled,
    #[error("{0}")]
    Data(String),
}

/// `devices/<device>.json`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceFile {
    pub schema: u32,
    pub device: String,
    pub updated: u64,
    pub saves: Vec<DeviceSave>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSave {
    pub file: String,
    pub source: String,
    pub info: SaveInfo,
}

/// The repo operations sync needs; [`GitHub`] in real use, a map in tests.
pub trait Remote {
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, SyncError>;
    /// File names directly inside `dir`.
    fn list(&self, dir: &str) -> Result<Vec<String>, SyncError>;
    /// Returns whether anything was written.
    fn put_if_changed(&self, path: &str, bytes: &[u8], message: &str) -> Result<bool, SyncError>;
}

/// A [`GitHub`] client bound to one repo.
pub struct RepoRemote<'a> {
    pub github: &'a GitHub,
    pub repo: &'a str,
}

impl Remote for RepoRemote<'_> {
    fn read(&self, path: &str) -> Result<Option<Vec<u8>>, SyncError> {
        Ok(self.github.read_file(self.repo, path)?.map(|f| f.bytes))
    }
    fn list(&self, dir: &str) -> Result<Vec<String>, SyncError> {
        Ok(self.github.list_dir(self.repo, dir)?.into_iter().filter(|e| e.kind == "file").map(|e| e.name).collect())
    }
    fn put_if_changed(&self, path: &str, bytes: &[u8], message: &str) -> Result<bool, SyncError> {
        self.github.put_if_changed(self.repo, path, bytes, message)
    }
}

pub struct Options<'a> {
    /// Already sanitised with [`device_id`].
    pub device: &'a str,
    pub backups: bool,
}

#[derive(Debug)]
pub struct Report {
    /// Repo paths written this time.
    pub written: Vec<String>,
    /// Devices included in the collection.
    pub devices: Vec<String>,
    /// Non-fatal problems (unreadable device files, missing local saves).
    pub warnings: Vec<String>,
    pub collection: Collection,
}

/// A repo-safe device name: lowercase letters, digits and dashes.
pub fn device_id(name: &str) -> String {
    let id: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let id = id.trim_matches('-').to_string();
    if id.is_empty() { "device".into() } else { id }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn to_json<T: Serialize>(v: &T) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(v).expect("serialisable");
    bytes.push(b'\n');
    bytes
}

/// This device's file, built from the local state.
pub fn device_file(device: &str, state: &State) -> DeviceFile {
    DeviceFile {
        schema: SCHEMA,
        device: device.into(),
        updated: state.saves.values().map(|r| r.updated).max().unwrap_or(0),
        saves: state
            .saves
            .iter()
            .map(|(path, r)| DeviceSave { file: path.display().to_string(), source: r.source.clone(), info: r.info.clone() })
            .collect(),
    }
}

/// Push this device, pull the others, regenerate `collection.json`.
pub fn sync(remote: &impl Remote, opts: &Options, state: &State) -> Result<Report, SyncError> {
    let mut written = Vec::new();
    let mut warnings = Vec::new();
    let msg = |what: &str| format!("dexsync: {what} from {}", opts.device);

    // 1. This device.
    let mine = device_file(opts.device, state);
    let path = format!("devices/{}.json", opts.device);
    if remote.put_if_changed(&path, &to_json(&mine), &msg("update dex"))? {
        written.push(path);
    }

    // 2. Optional raw save backups.
    if opts.backups {
        for (file, record) in &state.saves {
            let Some(name) = file.file_name().and_then(|n| n.to_str()) else { continue };
            let Ok(bytes) = std::fs::read(file) else {
                warnings.push(format!("couldn't read {} for backup", file.display()));
                continue;
            };
            let path = format!("saves/{}/{}/{}", opts.device, device_id(&record.source), name);
            if remote.put_if_changed(&path, &bytes, &msg("back up save"))? {
                written.push(path);
            }
        }
    }

    // 3. Every other device.
    let mut devices = vec![mine];
    for name in remote.list("devices")? {
        if !name.ends_with(".json") || name == format!("{}.json", opts.device) {
            continue;
        }
        let Some(bytes) = remote.read(&format!("devices/{name}"))? else { continue };
        match serde_json::from_slice::<DeviceFile>(&bytes) {
            Ok(d) if d.schema <= SCHEMA => devices.push(d),
            Ok(d) => warnings.push(format!("{name} uses schema {} (newer than this DexSync); skipped", d.schema)),
            Err(e) => warnings.push(format!("{name} is unreadable ({e}); skipped")),
        }
    }

    // 4. The merged collection; untouched if nothing but the timestamp would change.
    let refs = devices.iter().flat_map(|d| d.saves.iter().map(move |s| SaveRef { device: &d.device, file: &s.file, info: &s.info }));
    let mut merged = collection::merge(refs, now());
    let previous = remote.read("collection.json")?.and_then(|b| serde_json::from_slice::<Collection>(&b).ok());
    let unchanged = previous.as_ref().is_some_and(|p| Collection { generated: p.generated, ..merged.clone() } == *p);
    if unchanged {
        merged.generated = previous.unwrap().generated;
    } else if remote.put_if_changed("collection.json", &to_json(&merged), &msg("update collection"))? {
        written.push("collection.json".into());
    }

    Ok(Report { written, devices: devices.into_iter().map(|d| d.device).collect(), warnings, collection: merged })
}

/// Where the token falls back to when there's no keychain.
pub fn token_fallback_path() -> Option<PathBuf> {
    State::default_path().map(|p| p.with_file_name("github-token"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dexsync_core::Game;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct Fake {
        files: RefCell<BTreeMap<String, Vec<u8>>>,
        writes: RefCell<usize>,
    }

    impl Remote for Fake {
        fn read(&self, path: &str) -> Result<Option<Vec<u8>>, SyncError> {
            Ok(self.files.borrow().get(path).cloned())
        }
        fn list(&self, dir: &str) -> Result<Vec<String>, SyncError> {
            let prefix = format!("{dir}/");
            Ok(self.files.borrow().keys().filter_map(|k| k.strip_prefix(&prefix)).filter(|k| !k.contains('/')).map(String::from).collect())
        }
        fn put_if_changed(&self, path: &str, bytes: &[u8], _: &str) -> Result<bool, SyncError> {
            if self.files.borrow().get(path).is_some_and(|b| b == bytes) {
                return Ok(false);
            }
            *self.writes.borrow_mut() += 1;
            self.files.borrow_mut().insert(path.into(), bytes.to_vec());
            Ok(true)
        }
    }

    fn state(owned: &[u16]) -> State {
        let mut s = State::default();
        let info = SaveInfo { game: Game::Emerald, generation: 3, trainer: "BRANDON".into(), tid: 30309, sid: 1, owned: owned.into(), seen: owned.into() };
        s.apply("retroarch", "/saves/Emerald.srm".into(), info);
        s
    }

    #[test]
    fn device_ids_are_repo_safe() {
        assert_eq!(device_id("Nate's Steam Deck"), "nate-s-steam-deck");
        assert_eq!(device_id("  "), "device");
    }

    #[test]
    fn pushes_merges_and_skips_unchanged() {
        let remote = Fake::default();
        let opts = Options { device: "deck", backups: false };

        // Another device already synced a FireRed save.
        let other = DeviceFile {
            schema: 1,
            device: "phone".into(),
            updated: 1,
            saves: vec![DeviceSave {
                file: "fr.sav".into(),
                source: "custom".into(),
                info: SaveInfo { game: Game::FireRedLeafGreen, generation: 3, trainer: "RED".into(), tid: 1, sid: 2, owned: vec![4], seen: vec![4] },
            }],
        };
        remote.files.borrow_mut().insert("devices/phone.json".into(), to_json(&other));

        let r = sync(&remote, &opts, &state(&[252])).unwrap();
        assert_eq!(r.written, ["devices/deck.json", "collection.json"]);
        assert_eq!(r.devices, ["deck", "phone"]);
        assert_eq!(r.collection.overall.owned, [4, 252]);
        assert_eq!(r.collection.trainers.len(), 2);

        // Nothing changed: nothing written, not even a new timestamp.
        let writes = *remote.writes.borrow();
        let r = sync(&remote, &opts, &state(&[252])).unwrap();
        assert!(r.written.is_empty(), "{:?}", r.written);
        assert_eq!(*remote.writes.borrow(), writes);

        // A new catch updates both files.
        let r = sync(&remote, &opts, &state(&[252, 263])).unwrap();
        assert_eq!(r.written, ["devices/deck.json", "collection.json"]);
    }

    #[test]
    fn skips_newer_schemas() {
        let remote = Fake::default();
        remote.files.borrow_mut().insert("devices/future.json".into(), br#"{"schema":2,"device":"future","updated":0,"saves":[]}"#.to_vec());
        let r = sync(&remote, &Options { device: "deck", backups: false }, &state(&[1])).unwrap();
        assert_eq!(r.devices, ["deck"]);
        assert_eq!(r.warnings.len(), 1);
    }
}
