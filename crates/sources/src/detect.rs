//! Find likely save folders on this machine, per emulator preset.
//!
//! These are suggestions: the user confirms or edits them before they become
//! sources. Default locations are from emulator docs and real installs; the
//! ones marked "unverified" haven't been checked on a real machine yet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::{Preset, ini};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
    Mac,
}

/// The parts of the environment detection depends on, so tests can fake it.
#[derive(Debug, Clone)]
pub struct Env {
    pub os: Os,
    pub home: PathBuf,
    /// `~/.config`, `%APPDATA%`, `~/Library/Application Support`.
    pub config: PathBuf,
    /// `~/.local/share`, `%APPDATA%`, `~/Library/Application Support`.
    pub data: PathBuf,
}

impl Env {
    pub fn current() -> Option<Env> {
        let dirs = directories::BaseDirs::new()?;
        let os = if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Linux
        };
        Some(Env {
            os,
            home: dirs.home_dir().into(),
            config: dirs.config_dir().into(),
            data: dirs.data_dir().into(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    pub preset: Preset,
    pub path: PathBuf,
    /// Where it came from, e.g. "Steam RetroArch (retroarch.cfg)".
    pub label: String,
    /// False when the emulator is installed but hasn't written any saves yet.
    pub exists: bool,
}

pub fn detect(env: &Env) -> Vec<Suggestion> {
    let mut out = Vec::new();
    retroarch(env, &mut out);
    mgba(env, &mut out);
    azahar(env, &mut out);
    // The same folder can be reached through symlinks (~/.steam/steam).
    let mut seen = HashSet::new();
    out.retain(|s| seen.insert(std::fs::canonicalize(&s.path).unwrap_or_else(|_| s.path.clone())));
    out
}

// ── RetroArch ───────────────────────────────────────────────────────────

fn retroarch(env: &Env, out: &mut Vec<Suggestion>) {
    let mut roots: Vec<(PathBuf, String)> = vec![
        (env.config.join("retroarch"), "RetroArch".into()),
        (env.config.join("RetroArch"), "RetroArch".into()),
    ];
    match env.os {
        Os::Linux => roots.push((
            env.home.join(".var/app/org.libretro.RetroArch/config/retroarch"),
            "RetroArch (Flatpak)".into(),
        )),
        Os::Windows => roots.push((PathBuf::from(r"C:\RetroArch-Win64"), "RetroArch".into())),
        Os::Mac => {}
    }
    for lib in steam_libraries(env) {
        roots.push((lib.join("steamapps/common/RetroArch"), "Steam RetroArch".into()));
    }

    for (root, label) in roots {
        let Ok(cfg) = std::fs::read_to_string(root.join("retroarch.cfg")) else { continue };
        let dir = ini::value(&cfg, "savefile_directory").unwrap_or_default();
        let path = resolve_retroarch_dir(&dir, &root, &env.home);
        out.push(Suggestion { preset: Preset::RetroArch, exists: path.is_dir(), path, label: format!("{label} (retroarch.cfg)") });
    }
}

/// RetroArch path syntax: empty/"default" = `<root>/saves`, `:` = next to
/// the RetroArch program (the root for Steam and portable installs), `~` = home.
fn resolve_retroarch_dir(value: &str, root: &Path, home: &Path) -> PathBuf {
    let rel = |s: &str| s.trim_start_matches(['/', '\\']).to_string();
    match value {
        "" | "default" => root.join("saves"),
        v if v.starts_with(':') => root.join(rel(&v[1..])),
        v if v.starts_with('~') => home.join(rel(&v[1..])),
        v => PathBuf::from(v),
    }
}

/// Steam install roots plus any extra libraries (SD cards, other drives)
/// listed in `libraryfolders.vdf`.
fn steam_libraries(env: &Env) -> Vec<PathBuf> {
    let roots: Vec<PathBuf> = match env.os {
        Os::Linux => [".steam/steam", ".steam/debian-installation", ".local/share/Steam", ".var/app/com.valvesoftware.Steam/.local/share/Steam"]
            .iter()
            .map(|p| env.home.join(p))
            .collect(),
        Os::Windows => vec![PathBuf::from(r"C:\Program Files (x86)\Steam")],
        Os::Mac => vec![env.data.join("Steam")],
    };
    let mut libs = Vec::new();
    for root in roots.into_iter().filter(|r| r.is_dir()) {
        if let Ok(vdf) = std::fs::read_to_string(root.join("steamapps/libraryfolders.vdf")) {
            libs.extend(vdf_paths(&vdf));
        }
        libs.push(root);
    }
    libs
}

/// Pull `"path"  "<dir>"` entries out of libraryfolders.vdf.
fn vdf_paths(vdf: &str) -> Vec<PathBuf> {
    vdf.lines()
        .filter_map(|l| {
            let mut parts = l.split('"').filter(|p| !p.trim().is_empty());
            if parts.next()? != "path" {
                return None;
            }
            Some(PathBuf::from(parts.next()?.replace("\\\\", "\\")))
        })
        .collect()
}

// ── mGBA (standalone) ───────────────────────────────────────────────────

fn mgba(env: &Env, out: &mut Vec<Suggestion>) {
    let mut configs = vec![env.config.join("mgba/config.ini"), env.config.join("mGBA/config.ini")];
    if env.os == Os::Linux {
        configs.push(env.home.join(".var/app/io.mgba.mGBA/config/mgba/config.ini")); // unverified
    }
    for cfg in configs {
        let Ok(text) = std::fs::read_to_string(&cfg) else { continue };
        // Without savegamePath mGBA saves next to each ROM: the user has to
        // add their ROM folder as an mgba source.
        if let Some(dir) = ini::value(&text, "savegamePath").filter(|d| !d.is_empty()) {
            let path = PathBuf::from(dir);
            out.push(Suggestion { preset: Preset::Mgba, exists: path.is_dir(), path, label: "mGBA (savegamePath)".into() });
        }
    }
}

// ── Azahar and other Citra descendants ──────────────────────────────────

/// (folder name on Linux, folder name on Windows/macOS, label)
const CITRA_FAMILY: [(&str, &str, &str); 4] = [
    ("azahar-emu", "Azahar", "Azahar"),
    ("azaharplus-emu", "AzaharPlus", "AzaharPlus"),
    ("lime3ds-emu", "Lime3DS", "Lime3DS"),
    ("citra-emu", "Citra", "Citra"),
];

fn azahar(env: &Env, out: &mut Vec<Suggestion>) {
    for (linux, other, label) in CITRA_FAMILY {
        let name = if env.os == Os::Linux { linux } else { other };
        let (config_dir, data_dir) = (env.config.join(name), env.data.join(name));
        if !config_dir.is_dir() && !data_dir.is_dir() {
            continue;
        }
        // qt-config.ini can move the SD card anywhere.
        let configured = std::fs::read_to_string(config_dir.join("qt-config.ini"))
            .ok()
            .and_then(|t| ini::value(&t, "sdmc_directory"))
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        let path = configured.unwrap_or_else(|| data_dir.join("sdmc"));
        out.push(Suggestion { preset: Preset::Azahar, exists: path.is_dir(), path, label: format!("{label} (virtual SD card)") });
    }
    if env.os == Os::Linux {
        let path = env.home.join(".var/app/org.azahar_emu.Azahar/data/azahar-emu/sdmc"); // unverified
        if path.parent().is_some_and(Path::is_dir) {
            out.push(Suggestion { preset: Preset::Azahar, exists: path.is_dir(), path, label: "Azahar (Flatpak)".into() });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_env(home: &Path) -> Env {
        Env { os: Os::Linux, home: home.into(), config: home.join(".config"), data: home.join(".local/share") }
    }

    fn write(path: PathBuf, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    #[test]
    fn resolves_retroarch_paths() {
        let (root, home) = (Path::new("/ra"), Path::new("/home/u"));
        assert_eq!(resolve_retroarch_dir("", root, home), PathBuf::from("/ra/saves"));
        assert_eq!(resolve_retroarch_dir("default", root, home), PathBuf::from("/ra/saves"));
        assert_eq!(resolve_retroarch_dir(":/saves", root, home), PathBuf::from("/ra/saves"));
        assert_eq!(resolve_retroarch_dir("~/s", root, home), PathBuf::from("/home/u/s"));
        assert_eq!(resolve_retroarch_dir("/x/y", root, home), PathBuf::from("/x/y"));
    }

    #[test]
    fn reads_steam_library_folders() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.steam/steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}";
        assert_eq!(vdf_paths(vdf), vec![PathBuf::from("/home/u/.steam/steam"), PathBuf::from(r"D:\SteamLibrary")]);
    }

    #[test]
    fn finds_steam_retroarch_and_azahar() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let ra = home.join(".steam/debian-installation/steamapps/common/RetroArch");
        write(ra.join("retroarch.cfg"), "savefile_directory = \":/saves\"\n");
        fs::create_dir_all(ra.join("saves/mGBA")).unwrap();
        write(home.join(".config/azaharplus-emu/qt-config.ini"), "sdmc_directory=/elsewhere/sdmc/\n");

        let found = detect(&fake_env(home));
        assert_eq!(found.len(), 2, "{found:#?}");
        assert_eq!((found[0].preset, &found[0].path, found[0].exists), (Preset::RetroArch, &ra.join("saves"), true));
        assert_eq!((found[1].preset, &found[1].path, found[1].exists), (Preset::Azahar, &PathBuf::from("/elsewhere/sdmc/"), false));
    }

    #[test]
    fn mgba_without_save_path_suggests_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path().join(".config/mgba/config.ini"), "[ports.qt]\nsavegamePath=\n");
        assert!(detect(&fake_env(dir.path())).is_empty());
    }
}
