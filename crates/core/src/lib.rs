//! Pure Pokémon save parsers: bytes in, Pokédex flags out.
//!
//! This crate does no I/O so it can be reused on every target (desktop,
//! Android, WASM). Callers read the file and hand over the bytes.

pub mod collection;
pub mod dex;
pub mod gen3;
pub mod species;
mod text;

use serde::{Deserialize, Serialize};

/// Everything we extract from one save file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveInfo {
    pub game: Game,
    pub generation: u8,
    pub trainer: String,
    pub tid: u16,
    pub sid: u16,
    /// National Dex numbers flagged as owned, ascending.
    pub owned: Vec<u16>,
    /// National Dex numbers flagged as seen, ascending.
    pub seen: Vec<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Game {
    RubySapphire,
    Emerald,
    FireRedLeafGreen,
}

impl Game {
    pub fn name(self) -> &'static str {
        match self {
            Game::RubySapphire => "Ruby/Sapphire",
            Game::Emerald => "Emerald",
            Game::FireRedLeafGreen => "FireRed/LeafGreen",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("unrecognised save format ({0} bytes)")]
    UnknownFormat(usize),
    #[error("no valid save slot (both slots failed signature or checksum checks)")]
    NoValidSlot,
}

/// Detect the save format and parse it.
///
/// Detection uses size and in-file signatures, never the file extension.
pub fn parse(bytes: &[u8]) -> Result<SaveInfo, ParseError> {
    if gen3::looks_like(bytes) {
        return gen3::parse(bytes);
    }
    Err(ParseError::UnknownFormat(bytes.len()))
}
