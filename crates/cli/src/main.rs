use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use dexsync_core::SaveInfo;
use dexsync_sources::{Config, Env, Preset, Source, detect, discover, parse_file};
use dexsync_watcher::{Event, State};
use serde_json::json;

#[derive(Parser)]
#[command(name = "dexsync", version, about = "Track Pokédex progress across your emulators' save files")]
struct Cli {
    /// Config file (default: platform config dir, e.g. ~/.config/dexsync/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// State file (default: platform data dir, e.g. ~/.local/share/dexsync/state.json).
    #[arg(long, global = true)]
    state: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Look for emulator save folders on this machine.
    Detect {
        /// Add every suggestion that isn't configured yet.
        #[arg(long)]
        add: bool,
    },
    /// Manage the folders dexsync watches.
    #[command(subcommand)]
    Source(SourceCmd),
    /// Parse saves and print dex progress. With no paths, scans configured sources.
    Scan {
        paths: Vec<PathBuf>,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Scan configured sources, then keep watching them and update the state file.
    Watch {
        /// Print events as JSON lines.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum SourceCmd {
    /// List configured sources.
    List,
    /// Add a source. Without a path, uses the first detected folder for the preset.
    Add {
        /// retroarch, mgba, azahar or custom.
        preset: String,
        path: Option<PathBuf>,
        /// Name for the source (default: the preset name).
        #[arg(long)]
        id: Option<String>,
    },
    /// Remove a source by name.
    Remove { id: String },
}

type Res<T = ()> = Result<T, String>;

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Res<bool> {
    let config_path = cli.config.or_else(Config::default_path).ok_or("can't find a config directory; pass --config")?;
    let state_path = cli.state.or_else(State::default_path).ok_or("can't find a data directory; pass --state")?;
    let load = || Config::load(&config_path).map_err(|e| e.to_string());
    let save = |c: &Config| c.save(&config_path).map_err(|e| e.to_string());

    match cli.command {
        Command::Detect { add } => {
            let env = Env::current().ok_or("can't find the home directory")?;
            let mut config = load()?;
            let found = detect(&env);
            if found.is_empty() {
                println!("No emulator save folders found. Add one with `dexsync source add <preset> <path>`.");
            }
            for s in found {
                let configured = config.sources.iter().any(|c| c.path == s.path);
                let note = if configured { "  [configured]" } else if !s.exists { "  [no saves yet]" } else { "" };
                println!("{:<10} {}\n           {}{note}", s.preset.id(), s.label, s.path.display());
                if add && !configured {
                    let id = config.next_id(s.preset);
                    println!("           → added as '{id}'");
                    config.add(Source { id, preset: s.preset, path: s.path }).map_err(|e| e.to_string())?;
                }
            }
            if add {
                save(&config)?;
            }
            Ok(true)
        }

        Command::Source(cmd) => {
            let mut config = load()?;
            match cmd {
                SourceCmd::List => {
                    if config.sources.is_empty() {
                        println!("No sources yet. Try `dexsync detect --add`.");
                    }
                    for s in &config.sources {
                        let missing = if s.path.is_dir() { "" } else { "  [missing]" };
                        println!("{:<14} {:<10} {}{missing}", s.id, s.preset.id(), s.path.display());
                    }
                }
                SourceCmd::Add { preset, path, id } => {
                    let preset = Preset::from_id(&preset).ok_or_else(|| {
                        format!("unknown preset '{preset}' (use {})", Preset::ALL.map(Preset::id).join(", "))
                    })?;
                    let path = match path {
                        Some(p) => std::path::absolute(&p).map_err(|e| e.to_string())?,
                        None => {
                            let env = Env::current().ok_or("can't find the home directory")?;
                            detect(&env)
                                .into_iter()
                                .find(|s| s.preset == preset)
                                .map(|s| s.path)
                                .ok_or_else(|| format!("no {} folder detected; pass a path", preset.id()))?
                        }
                    };
                    let id = id.unwrap_or_else(|| config.next_id(preset));
                    println!("Added '{id}': {} {}", preset.id(), path.display());
                    config.add(Source { id, preset, path }).map_err(|e| e.to_string())?;
                    save(&config)?;
                }
                SourceCmd::Remove { id } => {
                    let s = config.remove(&id).map_err(|e| e.to_string())?;
                    println!("Removed '{}' ({})", s.id, s.path.display());
                    save(&config)?;
                }
            }
            Ok(true)
        }

        Command::Scan { paths, json } => {
            let results: Vec<(PathBuf, Result<SaveInfo, String>)> = if paths.is_empty() {
                let config = load()?;
                if config.sources.is_empty() {
                    return Err("no sources configured; pass paths or run `dexsync detect --add`".into());
                }
                config.sources.iter().flat_map(discover).map(|f| (f.path, f.result)).collect()
            } else {
                paths
                    .iter()
                    .flat_map(|p| files_in(p))
                    .map(|f| {
                        let r = parse_file(&f).unwrap_or_else(|| Err("unrecognised save format".into()));
                        (f, r)
                    })
                    .collect()
            };
            print_scan(&results, json);
            Ok(results.iter().all(|(_, r)| r.is_ok()))
        }

        Command::Watch { json } => {
            let config = load()?;
            if config.sources.is_empty() {
                return Err("no sources configured; run `dexsync detect --add`".into());
            }
            eprintln!("Watching {} source(s). State: {}. Ctrl+C to stop.", config.sources.len(), state_path.display());
            dexsync_watcher::watch(&config, &state_path, |e| print_event(e, json)).map_err(|e| e.to_string())?;
            Ok(true)
        }
    }
}

fn files_in(path: &Path) -> Vec<PathBuf> {
    if path.is_dir() {
        let mut files: Vec<_> =
            std::fs::read_dir(path).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
        files.sort();
        files
    } else {
        vec![path.to_path_buf()]
    }
}

fn save_json(info: &SaveInfo) -> serde_json::Value {
    json!({
        "game": info.game.name(), "gen": info.generation,
        "trainer": info.trainer, "tid": info.tid, "sid": info.sid,
        "owned": info.owned, "seen": info.seen,
        "ownedCount": info.owned.len(), "seenCount": info.seen.len(),
    })
}

fn summary(i: &SaveInfo) -> String {
    format!(
        "{} (Gen {})  {}  TID {:05}  owned {:>3}  seen {:>3}",
        i.game.name(),
        i.generation,
        i.trainer,
        i.tid,
        i.owned.len(),
        i.seen.len()
    )
}

fn print_scan(results: &[(PathBuf, Result<SaveInfo, String>)], json: bool) {
    if json {
        let out: Vec<_> = results
            .iter()
            .map(|(file, r)| match r {
                Ok(info) => {
                    let mut v = save_json(info);
                    v["file"] = json!(file);
                    v
                }
                Err(e) => json!({ "file": file, "error": e }),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }
    if results.is_empty() {
        println!("No Pokémon saves found.");
    }
    for (file, r) in results {
        match r {
            Ok(i) => println!("{}\n  {}", file.display(), summary(i)),
            Err(e) => println!("{}\n  error: {e}", file.display()),
        }
    }
}

fn print_event(event: &Event, json: bool) {
    if json {
        let v = match event {
            Event::Changed(c) => {
                let mut v = save_json(&c.info);
                v["event"] = json!("changed");
                v["file"] = json!(c.path);
                v["source"] = json!(c.source);
                v["first"] = json!(c.first);
                v["newOwned"] = json!(c.new_owned);
                v["newSeen"] = json!(c.new_seen);
                v
            }
            Event::Error { path, message } => json!({ "event": "error", "file": path, "error": message }),
            Event::Skipped { source, reason } => json!({ "event": "skipped", "source": source, "reason": reason }),
        };
        println!("{v}");
        return;
    }
    match event {
        Event::Changed(c) => {
            let what = if c.first { "found" } else { "updated" };
            println!("[{}] {what} {}\n  {}", c.source, c.path.display(), summary(&c.info));
            if !c.first && !c.new_owned.is_empty() {
                println!("  new owned: {:?}", c.new_owned);
            }
            if !c.first && !c.new_seen.is_empty() {
                println!("  new seen:  {:?}", c.new_seen);
            }
        }
        Event::Error { path, message } => println!("error {}: {message}", path.display()),
        Event::Skipped { source, reason } => println!("[{source}] not watched: {reason}"),
    }
}
