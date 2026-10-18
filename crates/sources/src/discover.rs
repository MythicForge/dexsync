//! Find and parse the save files inside a source folder.

use std::path::{Path, PathBuf};

use dexsync_core::{ParseError, SaveInfo};

use crate::{Preset, Source};

/// Largest save we expect (DS saves are 512 KB; leave room for footers).
const MAX_SAVE_SIZE: u64 = 1024 * 1024;

/// Extensions that are never saves: ROMs, archives, states, images.
const SKIP_EXTS: &[&str] = &[
    "gb", "gbc", "gba", "nds", "3ds", "cia", "cxi", "zip", "7z", "rar", "state", "png", "jpg", "txt", "cfg", "ini",
];

#[derive(Debug)]
pub struct Found {
    pub path: PathBuf,
    pub result: Result<SaveInfo, String>,
}

/// Could this file be a save, judging by name and size only?
pub fn is_candidate(preset: Preset, path: &Path, size: u64) -> bool {
    if preset == Preset::Azahar {
        return path.file_name().is_some_and(|n| n == "main");
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    let is_state = ext.starts_with("state") || ext.starts_with("ss"); // .state1, .ss0 …
    size > 0 && size <= MAX_SAVE_SIZE && !is_state && !SKIP_EXTS.contains(&ext.as_str())
}

/// Read and parse one file. `None` means "not a save we recognise" — not an
/// error, since source folders hold saves for every system.
pub fn parse_file(path: &Path) -> Option<Result<SaveInfo, String>> {
    // Read the whole file at once: parse a snapshot, never a file that's
    // changing underneath us.
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => return Some(Err(e.to_string())),
    };
    match dexsync_core::parse(&bytes) {
        Err(ParseError::UnknownFormat(_)) => None,
        other => Some(other.map_err(|e| e.to_string())),
    }
}

/// Every recognised save under a source.
pub fn discover(source: &Source) -> Vec<Found> {
    walkdir::WalkDir::new(&source.path)
        .max_depth(source.preset.max_depth())
        .sort_by_file_name()
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| e.metadata().is_ok_and(|m| is_candidate(source.preset, e.path(), m.len())))
        .filter_map(|e| parse_file(e.path()).map(|result| Found { path: e.into_path(), result }))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates() {
        let p = Path::new;
        assert!(is_candidate(Preset::RetroArch, p("Emerald.srm"), 131072));
        assert!(is_candidate(Preset::Custom, p("noext"), 32768));
        assert!(!is_candidate(Preset::RetroArch, p("Emerald.gba"), 131072));
        assert!(!is_candidate(Preset::RetroArch, p("Emerald.state1"), 131072));
        assert!(!is_candidate(Preset::RetroArch, p("big.srm"), 4 * 1024 * 1024));
        assert!(is_candidate(Preset::Azahar, p("data/00000001/main"), 400_000));
        assert!(!is_candidate(Preset::Azahar, p("data/00000001/other"), 400_000));
    }

    #[test]
    fn discovers_nested_saves_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let core = dir.path().join("mGBA");
        std::fs::create_dir_all(&core).unwrap();
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/../core/tests/fixtures/emerald-early.sav");
        std::fs::copy(fixture, core.join("Emerald.srm")).unwrap();
        std::fs::write(core.join("Tetris.srm"), [0u8; 8192]).unwrap(); // some other game

        let source = Source { id: "ra".into(), preset: Preset::RetroArch, path: dir.path().into() };
        let found = discover(&source);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].result.as_ref().unwrap().owned, vec![252]);
    }
}
