//! Keep a parsed state store in sync with the user's save folders.
//!
//! The state file (`state.json`) is this device's view of every save it has
//! seen. It is what later gets synced (see research.md §10).

mod store;

pub use store::{Change, Record, State, StoreError};

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::sleep;
use std::time::Duration;

use dexsync_sources::{Config, Source, discover, discover::is_candidate, parse_file};
use notify::RecursiveMode;

/// Wait this long after the last write before parsing: emulators write saves
/// in several chunks.
const DEBOUNCE: Duration = Duration::from_millis(1500);
/// A save that fails its checksum may still be mid-write; retry a few times.
const RETRIES: u32 = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug)]
pub enum Event {
    /// A save is new or its dex changed.
    Changed(Change),
    /// A file looked like a save but couldn't be read or failed validation.
    Error { path: PathBuf, message: String },
    /// A configured source can't be watched (e.g. its folder doesn't exist yet).
    Skipped { source: String, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("file watcher: {0}")]
    Notify(#[from] notify::Error),
}

/// Scan every source once, update `state`, and report what changed.
pub fn scan_all(config: &Config, state: &mut State) -> Vec<Event> {
    let mut events = Vec::new();
    for source in &config.sources {
        if !source.path.is_dir() {
            events.push(Event::Skipped { source: source.id.clone(), reason: format!("{} doesn't exist", source.path.display()) });
            continue;
        }
        for found in discover(source) {
            events.extend(record(state, source, found.path, found.result));
        }
    }
    events
}

/// Re-parse specific paths reported by the file watcher.
pub fn handle_paths(config: &Config, state: &mut State, paths: &[PathBuf]) -> Vec<Event> {
    let mut events = Vec::new();
    for path in paths {
        let Some(source) = owning_source(config, path) else { continue };
        let Ok(meta) = std::fs::metadata(path) else { continue }; // deleted: keep its last dex
        if !meta.is_file() || !is_candidate(source.preset, path, meta.len()) {
            continue;
        }
        let mut result = parse_file(path);
        for _ in 0..RETRIES {
            if !matches!(result, Some(Err(_))) {
                break;
            }
            sleep(RETRY_DELAY);
            result = parse_file(path);
        }
        if let Some(result) = result {
            events.extend(record(state, source, path.clone(), result));
        }
    }
    events
}

fn record(state: &mut State, source: &Source, path: PathBuf, result: Result<dexsync_core::SaveInfo, String>) -> Option<Event> {
    match result {
        Ok(info) => state.apply(&source.id, path, info).map(Event::Changed),
        Err(message) => Some(Event::Error { path, message }),
    }
}

/// The most specific source containing `path`.
fn owning_source<'a>(config: &'a Config, path: &Path) -> Option<&'a Source> {
    config
        .sources
        .iter()
        .filter(|s| path.starts_with(&s.path))
        .max_by_key(|s| s.path.components().count())
}

/// Scan once, then watch every source until the process is stopped.
/// `state_path` is rewritten after every change.
pub fn watch(config: &Config, state_path: &Path, mut on_event: impl FnMut(&Event)) -> Result<(), WatchError> {
    let mut state = State::load(state_path)?;
    let emit = |state: &State, events: Vec<Event>, on_event: &mut dyn FnMut(&Event)| -> Result<(), WatchError> {
        if events.iter().any(|e| matches!(e, Event::Changed(_))) {
            state.save(state_path)?;
        }
        events.iter().for_each(on_event);
        Ok(())
    };

    let events = scan_all(config, &mut state);
    emit(&state, events, &mut on_event)?;

    let (tx, rx) = mpsc::channel();
    let mut debouncer = notify_debouncer_mini::new_debouncer(DEBOUNCE, tx)?;
    for source in config.sources.iter().filter(|s| s.path.is_dir()) {
        debouncer.watcher().watch(&source.path, RecursiveMode::Recursive)?;
    }

    for batch in rx {
        let events = match batch {
            Ok(changes) => {
                let mut paths: Vec<PathBuf> = changes.into_iter().map(|e| e.path).collect();
                paths.sort();
                paths.dedup();
                handle_paths(config, &mut state, &paths)
            }
            Err(e) => vec![Event::Error { path: PathBuf::new(), message: format!("watcher: {e:?}") }],
        };
        emit(&state, events, &mut on_event)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dexsync_sources::Preset;

    const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/emerald-early.sav");

    #[test]
    fn scan_then_rescan_reports_only_changes() {
        let dir = tempfile::tempdir().unwrap();
        let save = dir.path().join("mGBA/Emerald.srm");
        std::fs::create_dir_all(save.parent().unwrap()).unwrap();
        std::fs::copy(FIXTURE, &save).unwrap();
        let config = Config {
            sources: vec![
                Source { id: "ra".into(), preset: Preset::RetroArch, path: dir.path().into() },
                Source { id: "missing".into(), preset: Preset::Azahar, path: dir.path().join("nope") },
            ],
        };
        let mut state = State::default();

        let events = scan_all(&config, &mut state);
        assert!(matches!(&events[0], Event::Changed(c) if c.first && c.new_owned == [252]));
        assert!(matches!(&events[1], Event::Skipped { source, .. } if source == "missing"));

        // Unchanged file: nothing to report.
        assert!(handle_paths(&config, &mut state, std::slice::from_ref(&save)).is_empty());

        // A save that fails validation (e.g. caught mid-write) is reported
        // after retries, and the last good dex is kept.
        let mut bytes = std::fs::read(FIXTURE).unwrap();
        bytes[0x10] ^= 0xFF;
        bytes[0xE010] ^= 0xFF;
        std::fs::write(&save, &bytes).unwrap();
        let events = handle_paths(&config, &mut state, std::slice::from_ref(&save));
        assert!(matches!(&events[..], [Event::Error { .. }]), "{events:?}");
        assert_eq!(state.saves[&save].info.owned, vec![252]);
    }
}
