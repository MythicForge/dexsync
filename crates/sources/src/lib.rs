//! Where saves live: emulator presets, path detection, the user's source
//! config, and discovering save files inside a source.

pub mod config;
pub mod detect;
pub mod discover;
mod ini;

pub use config::{Config, ConfigError, Source};
pub use detect::{Env, Suggestion, detect};
pub use discover::{Found, discover, parse_file};

use serde::{Deserialize, Serialize};

/// Emulator families we know the save layout of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Preset {
    /// RetroArch `savefile_directory`, possibly with per-core subfolders.
    RetroArch,
    /// mGBA standalone: `savegamePath`, or next to the ROMs.
    Mgba,
    /// Azahar and other Citra descendants: the virtual SD card (`sdmc`).
    Azahar,
    /// Any folder the user picks.
    Custom,
}

impl Preset {
    pub const ALL: [Preset; 4] = [Preset::RetroArch, Preset::Mgba, Preset::Azahar, Preset::Custom];

    pub fn id(self) -> &'static str {
        match self {
            Preset::RetroArch => "retroarch",
            Preset::Mgba => "mgba",
            Preset::Azahar => "azahar",
            Preset::Custom => "custom",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        Preset::ALL.into_iter().find(|p| p.id() == id)
    }

    /// How deep to look below the source folder.
    pub(crate) fn max_depth(self) -> usize {
        match self {
            // saves/<core>/<content>/file.srm
            Preset::RetroArch => 3,
            Preset::Mgba | Preset::Custom => 4,
            // sdmc/Nintendo 3DS/<id0>/<id1>/title/00040000/<title>/data/00000001/main
            Preset::Azahar => 10,
        }
    }
}
