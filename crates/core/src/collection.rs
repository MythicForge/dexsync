//! Merge many parsed saves into one collection.
//!
//! The serialised [`Collection`] is the `collection.json` contract read by
//! simple clients such as the handheld Pokédex (research.md §10): keep field
//! names stable and bump [`SCHEMA`] on breaking changes.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{Game, SaveInfo};

pub const SCHEMA: u32 = 1;

/// One parsed save and where it came from.
#[derive(Debug, Clone)]
pub struct SaveRef<'a> {
    pub device: &'a str,
    pub file: &'a str,
    pub info: &'a SaveInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub schema: u32,
    /// Unix seconds, supplied by the caller (this crate does no I/O).
    pub generated: u64,
    /// Union over every save: what the handheld shows.
    pub overall: Dex,
    /// One entry per trainer (name + TID + SID), i.e. per playthrough.
    pub trainers: Vec<Trainer>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Dex {
    pub owned: Vec<u16>,
    pub seen: Vec<u16>,
    pub owned_count: usize,
    pub seen_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trainer {
    pub name: String,
    pub tid: u16,
    pub sid: u16,
    pub games: Vec<Game>,
    pub dex: Dex,
    pub saves: Vec<SaveSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSummary {
    pub device: String,
    pub file: String,
    pub game: Game,
    pub owned_count: usize,
    pub seen_count: usize,
}

#[derive(Default)]
struct Sets {
    owned: BTreeSet<u16>,
    seen: BTreeSet<u16>,
}

impl Sets {
    fn add(&mut self, info: &SaveInfo) {
        self.owned.extend(&info.owned);
        // Owning a Pokémon implies having seen it.
        self.seen.extend(info.seen.iter().chain(&info.owned));
    }

    fn into_dex(self) -> Dex {
        Dex {
            owned_count: self.owned.len(),
            seen_count: self.seen.len(),
            owned: self.owned.into_iter().collect(),
            seen: self.seen.into_iter().collect(),
        }
    }
}

pub fn merge<'a>(saves: impl IntoIterator<Item = SaveRef<'a>>, generated: u64) -> Collection {
    let mut overall = Sets::default();
    let mut trainers: BTreeMap<(String, u16, u16), (Sets, Vec<SaveSummary>)> = BTreeMap::new();

    for s in saves {
        overall.add(s.info);
        let (sets, list) = trainers.entry((s.info.trainer.clone(), s.info.tid, s.info.sid)).or_default();
        sets.add(s.info);
        list.push(SaveSummary {
            device: s.device.into(),
            file: s.file.into(),
            game: s.info.game,
            owned_count: s.info.owned.len(),
            seen_count: s.info.seen.len(),
        });
    }

    Collection {
        schema: SCHEMA,
        generated,
        overall: overall.into_dex(),
        trainers: trainers
            .into_iter()
            .map(|((name, tid, sid), (sets, mut saves))| {
                saves.sort_by(|a, b| (&a.device, &a.file).cmp(&(&b.device, &b.file)));
                let mut games: Vec<Game> = saves.iter().map(|s| s.game).collect();
                games.dedup();
                Trainer { name, tid, sid, games, dex: sets.into_dex(), saves }
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(trainer: &str, tid: u16, game: Game, owned: &[u16], seen: &[u16]) -> SaveInfo {
        SaveInfo { game, generation: 3, trainer: trainer.into(), tid, sid: 0, owned: owned.into(), seen: seen.into() }
    }

    #[test]
    fn merges_per_trainer_and_overall() {
        let emerald_deck = info("BRANDON", 30309, Game::Emerald, &[252], &[252, 263]);
        // Same playthrough copied to the phone, a bit further along.
        let emerald_phone = info("BRANDON", 30309, Game::Emerald, &[252, 263], &[252, 261, 263]);
        let firered = info("RED", 1, Game::FireRedLeafGreen, &[4], &[1]);

        let c = merge(
            [
                SaveRef { device: "steamdeck", file: "e.sav", info: &emerald_deck },
                SaveRef { device: "phone", file: "e.sav", info: &emerald_phone },
                SaveRef { device: "steamdeck", file: "fr.sav", info: &firered },
            ],
            42,
        );

        assert_eq!((c.schema, c.generated), (SCHEMA, 42));
        assert_eq!(c.overall.owned, [4, 252, 263]);
        assert_eq!(c.overall.seen, [1, 4, 252, 261, 263], "owned counts as seen");
        assert_eq!(c.trainers.len(), 2);

        let brandon = &c.trainers[0];
        assert_eq!((brandon.name.as_str(), brandon.games.as_slice()), ("BRANDON", &[Game::Emerald][..]));
        assert_eq!(brandon.dex.owned, [252, 263]);
        assert_eq!(brandon.saves.iter().map(|s| s.device.as_str()).collect::<Vec<_>>(), ["phone", "steamdeck"]);
    }

    #[test]
    fn json_shape_is_stable() {
        let e = info("A", 1, Game::Emerald, &[1], &[1]);
        let json = serde_json::to_value(merge([SaveRef { device: "d", file: "f", info: &e }], 0)).unwrap();
        assert_eq!(json["schema"], 1);
        assert_eq!(json["overall"]["ownedCount"], 1);
        assert_eq!(json["trainers"][0]["saves"][0]["game"], "Emerald");
    }
}
