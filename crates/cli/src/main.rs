use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::json;

#[derive(Parser)]
#[command(name = "dexsync", version, about = "Read Pokédex progress from Pokémon save files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Parse save files (or every file in the given folders) and print dex progress.
    Scan {
        paths: Vec<PathBuf>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
}

fn files_in(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let mut files: Vec<_> = std::fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        files.sort();
        files
    } else {
        vec![path.to_path_buf()]
    }
}

fn main() -> ExitCode {
    let Command::Scan { paths, json } = Cli::parse().command;
    let mut results = Vec::new();
    let mut failed = false;

    for file in paths.iter().flat_map(|p| files_in(p)) {
        let parsed = std::fs::read(&file)
            .map_err(|e| e.to_string())
            .and_then(|b| dexsync_core::parse(&b).map_err(|e| e.to_string()));
        failed |= parsed.is_err();
        results.push((file, parsed));
    }

    if json {
        let out: Vec<_> = results
            .iter()
            .map(|(file, r)| match r {
                Ok(info) => json!({
                    "file": file, "game": info.game.name(), "gen": info.generation,
                    "trainer": info.trainer, "tid": info.tid, "sid": info.sid,
                    "owned": info.owned, "seen": info.seen,
                    "ownedCount": info.owned.len(), "seenCount": info.seen.len(),
                }),
                Err(e) => json!({ "file": file, "error": e }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        for (file, r) in &results {
            match r {
                Ok(i) => println!(
                    "{}\n  {} (Gen {})  {}  TID {:05}  owned {:>3}  seen {:>3}",
                    file.display(), i.game.name(), i.generation, i.trainer, i.tid, i.owned.len(), i.seen.len()
                ),
                Err(e) => println!("{}\n  error: {e}", file.display()),
            }
        }
    }

    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}
