//! Gen 3 (Ruby / Sapphire / Emerald / FireRed / LeafGreen) save parser.
//!
//! Layout (verify against Bulbapedia "Save data structure (Generation III)"
//! and PKHeX `SAV3.cs`):
//! - Two slots of 14 sections × 4 KB at 0x0000 and 0xE000.
//! - Sections are rotated on every save, so look each one up by its ID.
//! - Footer per section: ID u16 @0xFF4, checksum u16 @0xFF6,
//!   signature u32 @0xFF8 = 0x08012025, save index u32 @0xFFC.
//! - The slot with the higher save index (among valid slots) is current.

use crate::{Game, ParseError, SaveInfo, dex, text};

pub const SECTION_SIZE: usize = 0x1000;
pub const SECTIONS_PER_SLOT: usize = 14;
pub const SLOT_SIZE: usize = SECTION_SIZE * SECTIONS_PER_SLOT;
/// Both slots; the rest of a 128 KB file is Hall of Fame / extras.
pub const MIN_SIZE: usize = SLOT_SIZE * 2;
pub const SIGNATURE: u32 = 0x0801_2025;
pub const DEX_MAX: u16 = 386;

const OFF_ID: usize = 0xFF4;
const OFF_CHECKSUM: usize = 0xFF6;
const OFF_SIGNATURE: usize = 0xFF8;
const OFF_INDEX: usize = 0xFFC;

/// Bytes covered by each section's checksum, by section ID.
const DATA_SIZE: [usize; SECTIONS_PER_SLOT] = [
    0xF2C, 0xF80, 0xF80, 0xF80, 0xF08, 0xF80, 0xF80, 0xF80, 0xF80, 0xF80, 0xF80, 0xF80, 0xF80, 0x7D0,
];

// Section 0 (trainer info).
const TRAINER_NAME: usize = 0x00;
const TRAINER_NAME_LEN: usize = 7;
const TRAINER_TID: usize = 0x0A;
const TRAINER_SID: usize = 0x0C;
const DEX_OWNED: usize = 0x28;
const DEX_SEEN: usize = 0x5C;
const DEX_LEN: usize = 49;
const GAME_CODE: usize = 0xAC;

fn u16_at(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn u32_at(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

/// Gen 3 checksum: sum the data as u32 LE words, then fold to 16 bits.
pub fn checksum(data: &[u8]) -> u16 {
    let sum = data
        .as_chunks::<4>()
        .0
        .iter()
        .fold(0u32, |acc, w| acc.wrapping_add(u32::from_le_bytes(*w)));
    ((sum >> 16) as u16).wrapping_add(sum as u16)
}

/// One validated save slot with its sections indexed by ID.
struct Slot<'a> {
    index: u32,
    sections: [&'a [u8]; SECTIONS_PER_SLOT],
}

/// Validate a slot: every section ID present once, signatures and
/// checksums correct, all sharing one save index.
fn read_slot(slot: &[u8]) -> Option<Slot<'_>> {
    let mut sections: [Option<&[u8]>; SECTIONS_PER_SLOT] = [None; SECTIONS_PER_SLOT];
    let mut index = None;
    for sec in slot.as_chunks::<SECTION_SIZE>().0 {
        if u32_at(sec, OFF_SIGNATURE) != SIGNATURE {
            return None;
        }
        let id = usize::from(u16_at(sec, OFF_ID));
        let size = *DATA_SIZE.get(id)?;
        if sections[id].is_some() || checksum(&sec[..size]) != u16_at(sec, OFF_CHECKSUM) {
            return None;
        }
        let i = u32_at(sec, OFF_INDEX);
        if *index.get_or_insert(i) != i {
            return None;
        }
        sections[id] = Some(sec);
    }
    Some(Slot { index: index?, sections: sections.map(|s| s.unwrap()) })
}

/// Cheap check used by format detection: right size and at least one
/// section signature where we expect it.
pub fn looks_like(bytes: &[u8]) -> bool {
    bytes.len() >= MIN_SIZE
        && (0..SECTIONS_PER_SLOT * 2).any(|i| u32_at(bytes, i * SECTION_SIZE + OFF_SIGNATURE) == SIGNATURE)
}

pub fn parse(bytes: &[u8]) -> Result<SaveInfo, ParseError> {
    if bytes.len() < MIN_SIZE {
        return Err(ParseError::UnknownFormat(bytes.len()));
    }
    // TODO: save index wrap-around (0xFFFFFFFF → 0), see PKHeX.
    let slot = [read_slot(&bytes[..SLOT_SIZE]), read_slot(&bytes[SLOT_SIZE..MIN_SIZE])]
        .into_iter()
        .flatten()
        .max_by_key(|s| s.index)
        .ok_or(ParseError::NoValidSlot)?;

    let trainer = slot.sections[0];
    let game = match u32_at(trainer, GAME_CODE) {
        0 => Game::RubySapphire,
        1 => Game::FireRedLeafGreen,
        _ => Game::Emerald, // Emerald stores its security key here.
    };

    Ok(SaveInfo {
        game,
        generation: 3,
        trainer: text::decode_gen3(&trainer[TRAINER_NAME..TRAINER_NAME + TRAINER_NAME_LEN]),
        tid: u16_at(trainer, TRAINER_TID),
        sid: u16_at(trainer, TRAINER_SID),
        owned: dex::flags_to_numbers(&trainer[DEX_OWNED..DEX_OWNED + DEX_LEN], DEX_MAX),
        seen: dex::flags_to_numbers(&trainer[DEX_SEEN..DEX_SEEN + DEX_LEN], DEX_MAX),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a synthetic save. `rotate` shifts section order like the game does.
    struct Fixture {
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn new() -> Self {
            Self { bytes: vec![0; 0x20000] }
        }

        fn slot(&mut self, n: usize, index: u32, rotate: usize, game_code: u32, owned: &[u16], seen: &[u16]) -> &mut Self {
            let base = n * SLOT_SIZE;
            for pos in 0..SECTIONS_PER_SLOT {
                let id = (pos + rotate) % SECTIONS_PER_SLOT;
                let sec = &mut self.bytes[base + pos * SECTION_SIZE..base + (pos + 1) * SECTION_SIZE];
                sec.fill(0);
                if id == 0 {
                    let mut name = text::encode_gen3("NATE");
                    name.push(0xFF);
                    sec[..name.len()].copy_from_slice(&name);
                    sec[TRAINER_TID..TRAINER_TID + 2].copy_from_slice(&12345u16.to_le_bytes());
                    sec[TRAINER_SID..TRAINER_SID + 2].copy_from_slice(&54321u16.to_le_bytes());
                    sec[GAME_CODE..GAME_CODE + 4].copy_from_slice(&game_code.to_le_bytes());
                    for &(off, list) in &[(DEX_OWNED, owned), (DEX_SEEN, seen)] {
                        for &d in list {
                            let i = usize::from(d - 1);
                            sec[off + i / 8] |= 1 << (i % 8);
                        }
                    }
                } else {
                    // Non-zero filler so checksums aren't trivially zero.
                    sec[0] = id as u8;
                }
                let sum = checksum(&sec[..DATA_SIZE[id]]);
                sec[OFF_ID..OFF_ID + 2].copy_from_slice(&(id as u16).to_le_bytes());
                sec[OFF_CHECKSUM..OFF_CHECKSUM + 2].copy_from_slice(&sum.to_le_bytes());
                sec[OFF_SIGNATURE..OFF_SIGNATURE + 4].copy_from_slice(&SIGNATURE.to_le_bytes());
                sec[OFF_INDEX..OFF_INDEX + 4].copy_from_slice(&index.to_le_bytes());
            }
            self
        }

        fn corrupt(&mut self, slot: usize) -> &mut Self {
            self.bytes[slot * SLOT_SIZE + 0x10] ^= 0xFF;
            self
        }
    }

    #[test]
    fn checksum_folds_halves() {
        // 0x0001_0002 + 0x0003_0004 = 0x0004_0006 → 0x0004 + 0x0006
        assert_eq!(checksum(&[2, 0, 1, 0, 4, 0, 3, 0]), 0x000A);
    }

    #[test]
    fn reads_trainer_and_dex() {
        let mut f = Fixture::new();
        f.slot(0, 5, 3, 0, &[1, 25, 386], &[1, 16, 25, 386]);
        let info = parse(&f.bytes).unwrap();
        assert_eq!(info.game, Game::RubySapphire);
        assert_eq!(info.trainer, "NATE");
        assert_eq!((info.tid, info.sid), (12345, 54321));
        assert_eq!(info.owned, vec![1, 25, 386]);
        assert_eq!(info.seen, vec![1, 16, 25, 386]);
    }

    #[test]
    fn picks_newest_slot() {
        let mut f = Fixture::new();
        f.slot(0, 7, 0, 1, &[1], &[1]).slot(1, 8, 5, 1, &[1, 4], &[1, 4]);
        let info = parse(&f.bytes).unwrap();
        assert_eq!(info.game, Game::FireRedLeafGreen);
        assert_eq!(info.owned, vec![1, 4]);
    }

    #[test]
    fn falls_back_when_newest_slot_is_corrupt() {
        let mut f = Fixture::new();
        f.slot(0, 7, 0, 0xDEAD_BEEF, &[1], &[1]).slot(1, 8, 0, 0xDEAD_BEEF, &[1, 4], &[1, 4]).corrupt(1);
        let info = parse(&f.bytes).unwrap();
        assert_eq!(info.game, Game::Emerald);
        assert_eq!(info.owned, vec![1]);
    }

    #[test]
    fn both_slots_corrupt_is_an_error() {
        let mut f = Fixture::new();
        f.slot(0, 1, 0, 0, &[], &[]).slot(1, 2, 0, 0, &[], &[]).corrupt(0).corrupt(1);
        assert_eq!(parse(&f.bytes), Err(ParseError::NoValidSlot));
    }

    #[test]
    fn tolerates_trailing_bytes() {
        let mut f = Fixture::new();
        f.slot(0, 1, 0, 0, &[151], &[151]);
        f.bytes.extend([0u8; 16]); // e.g. RTC data appended by an emulator
        assert_eq!(crate::parse(&f.bytes).unwrap().owned, vec![151]);
    }

    #[test]
    fn rejects_unknown_data() {
        assert_eq!(crate::parse(&[0u8; 0x8000]), Err(ParseError::UnknownFormat(0x8000)));
        assert!(!looks_like(&vec![0u8; 0x20000]));
    }
}
