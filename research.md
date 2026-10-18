# DexSync — Research Handoff

Status: refocused on companion app, ready for prototype
Date: 2026-10-18
Owner: Nate

---

## 1. Goal

A **companion app** that reads Pokémon save files from the user's emulators, extracts Pokédex "owned" and "seen" flags, and combines them into one persistent collection across games and devices.

- The user sets up their emulators and save paths once (with auto-detected suggestions).
- The app watches those paths and updates the dex whenever a save changes.
- **Read-only.** Never writes to save files.
- Works alongside any frontend. Cartridge gets optional extra integration (§4), but is not required.

---

## 2. Prior art (as of Oct 2026)

| Project | What it does | Gap vs this idea |
|---|---|---|
| [PKVault](https://projectpokemon.org/home/files/file/5766-pkvault/) | Built on PKHeX. Reads local saves, builds centralized Pokédex / living dex | **Closest competitor.** Check: does it watch folders automatically, sync across devices, expose an API? Reported issues with Gen 6+ `main` saves |
| [PKForge](https://newreleases.io/project/github/sofianeelhor/PKForge/release/v2.3.0) | Save tool with a Pokédex view (owned in colour, missing as silhouettes) | Manual load, not a watcher |
| [Ironmon Tracker](https://github.com/akdb/Ironmon-Tracker) | Lua, BizHawk + mGBA, Gen 3 live RAM | No dex completion |
| [PokeStreamer Tools](https://github.com/EverOddish/PokeStreamer-Tools/) | Lua party/box reader for overlays | No dex tracking |
| Archipelago (R/B, Emerald) | BizHawk Lua bridge | Only inside the randomizer |
| [pokedextracker.com](https://pokedextracker.com), [OMNIDEX](https://megalow.itch.io/omnidex), others | Manual checklists | Not automatic |
| PKHeX | Save editor, every mainline game | Not a tracker. Best format reference |

**Gap to own:** automatic folder watching + emulator-aware setup + combined collection across devices + an API/JSON other tools (overlays, frontends) can read.

**TODO before coding:** spend an hour with PKVault and list what it can't do.

---

## 3. Core design decisions

| Decision | Choice | Why |
|---|---|---|
| Data source | **Save files**, not RAM | Works with any emulator/frontend and real-cart dumps. Dex only counts after an in-game save anyway |
| Setup | **User configures sources**, app suggests | Save locations vary too much per emulator/OS to guess reliably |
| Access | **Read-only**, copy-then-parse | Never risk corrupting a save; avoid reading half-written files |
| Identity | Trainer name + TID (+ SID) + game | Same trainer across devices merges; different playthroughs stay separate |

---

## 4. Cartridge (optional integration)

Inspected Cartridge 0.4.9.3 builds in this folder (read-only, nothing executed):

- **Not Socket.** Author "Isaiah" in `package.json`, license field MIT. Socket (Depmots) is a separate Godot app.
- Desktop (Win/Mac/SteamOS) = **Electron 43 + React**. App code in `resources/app.asar`, plain unminified JS.
- Android = same React UI in **Capacitor**, no native plugins.
- **No plugin system.** `preload.js` exposes a fixed IPC list.

**Usable unofficial signal (desktop):**
- `library.json` in Electron userData (`%APPDATA%\Cartridge`, `~/Library/Application Support/Cartridge`, `~/.config/Cartridge`).
- On emulator exit, `onGameExit` (main.js ~L758) patches the game's `lastPlayed`, `playtime`, `plays`, then writes `library.json` (~250 ms debounce). Each entry has ROM `path` and `system`.
- Companion can watch it to know *which game just closed* and map it to a save file.
- `logs/last-launch.log` holds the exact emulator command (emulator + core).
- RetroArch saves stay in RetroArch's own `savefile_directory` (Cartridge only overrides the states dir).

**Android:** library is app-private, so no exit signal. Rely on save-folder watching.

**Later pitch to the developer:** one small official hook in `onGameExit` (write `last-exit.json`, or run a user command with ROM + system). Ask whether the repo could be opened, given the MIT field.

**Don't:** patch `app.asar` for distribution. Breaks every update.

---

## 5. Setup flow (user-configured sources)

1. **Add a source.** Choose an emulator preset or "Custom folder".
2. App **suggests default paths** for that preset on the current OS (and reads emulator config where possible, e.g. RetroArch `retroarch.cfg` → `savefile_directory`, `sort_savefiles_by_content_enable`).
3. User confirms or browses to the real folder. App shows saves found and what it detected in each (game, trainer, dex count) so the user can verify.
4. Optional: point at Cartridge's data folder to enable the exit signal and ROM→save mapping.
5. Sources stored in the app's config; watcher starts.

### Emulator presets (paths unverified — confirm on real installs)

| Emulator | Systems | Save location / pattern | Notes |
|---|---|---|---|
| mGBA | GB/GBC/GBA | `<rom>.sav` next to ROM (configurable) | Raw |
| RetroArch | all | `savefile_directory` / `<rom>.srm` (maybe per-core subfolder) | Read `retroarch.cfg` |
| BizHawk | GB/GBA/NDS | `<system>/SaveRAM/<rom>.SaveRAM` | |
| VisualBoyAdvance(-M) | GB/GBA | `.sav` | Old versions: wrong size |
| melonDS | NDS | `.sav` next to ROM | |
| DeSmuME | NDS | `.dsv` | Strip footer |
| DraStic (Android) | NDS | `.dsv` | May differ |
| Pizza Boy / My Boy! (Android) | GB/GBA | app/user folder | Scoped storage |
| **Azahar / Lime3DS / Citra / Mandarine** | 3DS | `<user>/sdmc/Nintendo 3DS/000…0/000…0/title/00040000/<title-id>/data/00000001/main` | Plain file. User dir: Linux `~/.local/share/azahar-emu/`, Windows `%APPDATA%\Azahar\`, Android user-chosen |

- Detect format by **file size + signature/checksum**, not extension. Tolerate trailing RTC bytes.
- 3DS: title ID in path identifies the game.

---

## 6. Save format notes (verify against Bulbapedia + PKHeX before coding)

> All offsets from memory/common references. **Unverified** until checked against [Bulbapedia save data structure](https://bulbapedia.bulbagarden.net/wiki/Save_data_structure_(Generation_III)) and PKHeX `PKHeX.Core/Saves/`.

### Gen 1 (R/B/Y) — 32 KB
- Owned `0x25A3`, seen `0x25B6`, 19 bytes (151 bits), bit `n-1` = dex #n, LSB-first. One checksum.

### Gen 2 (G/S/C) — 32 KB (+RTC)
- G/S owned `0x2A4C`, seen `0x2A6C`. Crystal owned `0x2A27`, seen `0x2A47`. 32 bytes (251 bits).
- Primary + backup copies. JP/KR offsets differ.

### Gen 3 (R/S/E/FR/LG) — 128 KB  ← **prototype target, implemented**
- ✅ **Verified on a real Emerald save** (RetroArch mGBA core, 2026-10-18): slot/section layout, signature, checksums, save index, trainer name/TID/SID, dex owned/seen offsets and bit order, Emerald detection. Fixture: `crates/core/tests/fixtures/emerald-early.sav`.
- Still to verify: R/S and FR/LG saves, and a save with dex numbers > 256.
- Two slots × 14 sections × 4 KB. Active slot = higher save index. Find sections by ID.
- Footer: ID `0xFF4`, checksum `0xFF6`, signature `0xFF8` = `0x08012025`, index `0xFFC`.
- Checksum: sum u32 LE over section data, fold upper16 + lower16.
- Section 0: owned `0x28`, seen `0x5C`, 49 bytes (386 bits, National order). Game ID at `0xAC`.

### Gen 4 / 5 (DS) — 512 KB
- Save not encrypted (only Pokémon structs). CRC16-CCITT blocks, backup copies.
- Dex offsets per game (DP / Pt / HGSS / BW / B2W2) from PKHeX. Strip DeSmuME footer.

### Gen 6 / 7 (3DS) — `main` file
- **In scope for emulators:** Citra-family writes saves decrypted. Real-hardware dumps (Checkpoint/JKSM) produce the same `main`.
- Only Pokémon structs encrypted; dex flags are plain bit fields in a block.
- Per-game size and block layout: PKHeX `SAV6XY.cs`, `SAV6AO.cs`, `SAV7SM.cs`, `SAV7USUM.cs`. Gen 6 dex tracks form/gender per species.

| Game | Title ID (low) |
|---|---|
| X / Y | `00055D00` / `00055E00` |
| OR / AS | `0011C400` / `0011C500` |
| Sun / Moon | `00164800` / `00175E00` |
| US / UM | `001B5000` / `001B5100` |

### Out of scope
Switch (Let's Go onward): different emulation situation, newer save formats. Revisit later.

---

## 7. Architecture

**Stack: Rust** (Cargo workspace). One codebase builds native for Windows / macOS / Linux / SteamOS, as a library for Android (uniffi), and can be exposed to Node/Electron (napi-rs) or the web (WASM) — so the core can still be offered to Cartridge. UI via **Tauri 2** (desktop first, Android later; web UI in TypeScript).

```
crates/core/      Pure parsers, no I/O: bytes → {game, gen, trainer, tid, sid, owned[], seen[]}
                  gen1 gen2 gen3 gen4 gen5 gen6 gen7, detect (size/signature/title ID → parser)
crates/sources/   Emulator presets: default paths per OS, config readers (retroarch.cfg), save globbing,
                  optional Cartridge library.json integration
crates/watcher/   `notify`-based watching; debounce; copy-then-parse; retry on bad checksum (mid-write)
crates/collection Merge saves → per-game + combined dex; keyed by trainer+TID+game; change history
crates/cli/       `dexsync scan <path>`, later `dexsync watch`
app/              Tauri 2: setup wizard (sources), dex grid, combined collection, local API
data/             National Dex list (number, name, gen) from PokéAPI CSV
```

Keep `core` free of I/O so it ports to every target.

**Output contract (JSON, per save):**
```json
{ "source": "azahar", "file": "...", "game": "Ultra Moon", "gen": 7,
  "trainer": "NATE", "tid": 12345, "sid": 54321,
  "owned": [1, 4, 25], "seen": [1, 4, 16, 25], "ownedCount": 3, "seenCount": 4,
  "parsedAt": "2026-10-18T12:00:00Z" }
```

---

## 8. Milestones

1. **M1 — Gen 3 parser + CLI** (`dex scan <path>`): slots/sections, checksums, owned/seen, names. Unit tests.
2. **M2 — Sources + watcher**: config file of sources, presets for mGBA + RetroArch + Azahar, debounced watching, local store.
3. **M3 — Gen 1/2 parsers.**
4. **M4 — Setup UI + dex view**: source wizard with detected saves preview, dex grid, combined collection.
5. **M5 — Gen 4/5**, then **Gen 6/7** (3DS `main`).
6. **M6 — Cartridge `library.json` integration** + local API.
7. **M7 — GitHub sync (§10) + Android** (scoped storage / all-files access, user-picked folders).
8. **M8 — Cartridge developer outreach** with working demo.

---

## 9. Testing

- Use **your own** saves. Never commit ROMs. Only commit saves you're OK sharing.
- Fixtures per gen: new game, mid-game, completed dex, RTC-padded, corrupted checksum, half-written (truncated).
- Cross-check counts against PKHeX and the in-game Pokédex.
- Verify every preset path on a real install before shipping it.

---

## 10. Sync backend: GitHub repo (proposed)

Use a **private GitHub repo** the user owns as free storage and sync between devices.

- **Parsed dex JSON syncs automatically**: tiny, readable diffs, git history becomes a timeline of the user's dex progress.
- **Raw save backups are opt-in** (setting, off by default): uploads saves (≤512 KB each) to `saves/` so a lost save can be restored.
- **One file per device** (`devices/<device-id>.json`), merged when read → no merge conflicts between devices.
- **Use the GitHub REST Contents API**, not a git binary → works on Android and needs no git install.
- **Auth:** GitHub OAuth device flow (user enters a code on github.com) or a fine-grained token scoped to that single repo. Store in the OS keychain.
- **Private by default:** saves hold trainer names and IDs. Never upload ROMs.
- Keep the backend behind a trait so a plain folder (Syncthing, Dropbox) or a hosted API can be added later.

```
dexsync-data/                (user's private repo)
  collection.json            merged view (optional, regenerated)
  devices/steamdeck.json     per-device parsed saves
  devices/phone.json
  saves/<device>/<game>.sav  raw backups (only when enabled)
```

---

## 11. Open questions

- What does PKVault already do? (§2)
- Android: which emulators keep saves in reachable storage?

---

## 12. References

- Bulbapedia — Save data structure (Gen I–VII)
- PKHeX — `PKHeX.Core/Saves/SAV1.cs` … `SAV7USUM.cs`
- PokéAPI data CSVs (species list)
- Cartridge 0.4.9.3 builds in this folder (§4)
