//! Real saves from real games, cross-checked against the in-game Pokédex.

use dexsync_core::{Game, parse};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// Emerald via RetroArch mGBA core: just picked Treecko, saw Birch's Zigzagoon.
#[test]
fn emerald_early() {
    let info = parse(&fixture("emerald-early.sav")).unwrap();
    assert_eq!(info.game, Game::Emerald);
    assert_eq!(info.trainer, "BRANDON");
    assert_eq!((info.tid, info.sid), (30309, 53400));
    assert_eq!(info.owned, vec![252]);
    assert_eq!(info.seen, vec![252, 263]);
}
