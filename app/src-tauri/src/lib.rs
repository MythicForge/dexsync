//! DexSync desktop app: Tauri commands over the dexsync crates, plus a
//! background watcher that tells the UI when a save changes.

mod sync;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use dexsync_core::collection::Collection;
use dexsync_core::species;
use dexsync_sources::{Config, Env, Preset, Source, detect};
use dexsync_watcher::{Event, State, device_name};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

struct Paths {
    config: PathBuf,
    state: PathBuf,
}

/// The running watcher thread and the flag that stops it.
type Watcher = Option<(Arc<AtomicBool>, JoinHandle<()>)>;

pub(crate) struct AppState {
    paths: Paths,
    watcher: Mutex<Watcher>,
    sync: sync::SyncState,
}

pub(crate) type Res<T = ()> = Result<T, String>;

pub(crate) fn err(e: impl ToString) -> String {
    e.to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SuggestionDto {
    preset: &'static str,
    path: PathBuf,
    label: String,
    exists: bool,
    configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceDto {
    id: String,
    preset: &'static str,
    path: PathBuf,
    exists: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Overview {
    sources: Vec<SourceDto>,
    suggestions: Vec<SuggestionDto>,
    sprites: Option<PathBuf>,
    collection: Collection,
    device: String,
}

#[derive(Serialize)]
struct SpeciesDto {
    id: u16,
    name: &'static str,
    generation: u8,
    types: Vec<&'static str>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChangeDto {
    source: String,
    file: PathBuf,
    game: &'static str,
    trainer: String,
    first: bool,
    new_owned: Vec<u16>,
    new_seen: Vec<u16>,
}

pub(crate) fn load_config(app: &AppState) -> Res<Config> {
    Config::load(&app.paths.config).map_err(err)
}

#[tauri::command]
fn overview(app: tauri::State<AppState>) -> Res<Overview> {
    let config = load_config(&app)?;
    let state = State::load(&app.paths.state).map_err(err)?;
    let suggestions = Env::current()
        .map(|env| detect(&env))
        .unwrap_or_default()
        .into_iter()
        .map(|s| SuggestionDto {
            preset: s.preset.id(),
            configured: config.sources.iter().any(|c| c.path == s.path),
            path: s.path,
            label: s.label,
            exists: s.exists,
        })
        .collect();
    let device = device_name();
    Ok(Overview {
        sources: config
            .sources
            .iter()
            .map(|s| SourceDto { id: s.id.clone(), preset: s.preset.id(), exists: s.path.is_dir(), path: s.path.clone() })
            .collect(),
        suggestions,
        sprites: config.sprites,
        collection: state.collection(&device),
        device,
    })
}

#[tauri::command]
fn species_list() -> Vec<SpeciesDto> {
    species::all()
        .iter()
        .map(|s| SpeciesDto { id: s.id, name: s.name, generation: s.generation, types: s.types.clone() })
        .collect()
}

#[tauri::command]
fn add_source(handle: AppHandle, app: tauri::State<AppState>, preset: String, path: PathBuf) -> Res {
    let preset = Preset::from_id(&preset).ok_or_else(|| format!("unknown preset '{preset}'"))?;
    let mut config = load_config(&app)?;
    let id = config.next_id(preset);
    config.add(Source { id, preset, path }).map_err(err)?;
    config.save(&app.paths.config).map_err(err)?;
    restart_watcher(&handle, &app, config);
    Ok(())
}

#[tauri::command]
fn remove_source(handle: AppHandle, app: tauri::State<AppState>, id: String) -> Res {
    let mut config = load_config(&app)?;
    config.remove(&id).map_err(err)?;
    config.save(&app.paths.config).map_err(err)?;
    restart_watcher(&handle, &app, config);
    Ok(())
}

#[tauri::command]
fn set_sprites(handle: AppHandle, app: tauri::State<AppState>, path: Option<PathBuf>) -> Res {
    let mut config = load_config(&app)?;
    if let Some(p) = &path {
        allow_sprites(&handle, p)?;
    }
    config.sprites = path;
    config.save(&app.paths.config).map_err(err)
}

/// Let the webview load images from the sprites folder (and nothing else).
fn allow_sprites(handle: &AppHandle, dir: &PathBuf) -> Res {
    handle.asset_protocol_scope().allow_directory(dir, false).map_err(err)
}

/// Stop the current watcher (if any) and start one for `config`.
fn restart_watcher(handle: &AppHandle, app: &AppState, config: Config) {
    let mut slot = app.watcher.lock().unwrap();
    if let Some((stop, thread)) = slot.take() {
        stop.store(true, Ordering::Relaxed);
        let _ = thread.join();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (handle, state_path, flag) = (handle.clone(), app.paths.state.clone(), stop.clone());
    let thread = std::thread::spawn(move || {
        let result = dexsync_watcher::watch(&config, &state_path, &flag, |event| match event {
            Event::Changed(c) => {
                sync::request(&handle.state::<AppState>(), sync::Job::Soon);
                let _ = handle.emit(
                    "dex-changed",
                    ChangeDto {
                        source: c.source.clone(),
                        file: c.path.clone(),
                        game: c.info.game.name(),
                        trainer: c.info.trainer.clone(),
                        first: c.first,
                        new_owned: c.new_owned.clone(),
                        new_seen: c.new_seen.clone(),
                    },
                );
            }
            Event::Error { path, message } => {
                let _ = handle.emit("watch-error", format!("{}: {message}", path.display()));
            }
            Event::Skipped { .. } => {}
        });
        if let Err(e) = result {
            let _ = handle.emit("watch-error", e.to_string());
        }
    });
    *slot = Some((stop, thread));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let paths = Paths {
        config: Config::default_path().expect("no config directory"),
        state: State::default_path().expect("no data directory"),
    };
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState { paths, watcher: Mutex::new(None), sync: sync::SyncState::default() })
        .setup(|app| {
            let handle = app.handle().clone();
            let state = app.state::<AppState>();
            let config = load_config(&state).unwrap_or_default();
            if let Some(dir) = &config.sprites {
                let _ = allow_sprites(&handle, dir);
            }
            *state.sync.jobs.lock().unwrap() = Some(sync::start_worker(handle.clone()));
            restart_watcher(&handle, &state, config);
            // Pull other devices' progress at startup.
            sync::request(&state, sync::Job::Now);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            overview,
            species_list,
            add_source,
            remove_source,
            set_sprites,
            sync::sync_status,
            sync::sync_connect,
            sync::sync_cancel_connect,
            sync::sync_set_token,
            sync::sync_disconnect,
            sync::sync_repos,
            sync::sync_set_repo,
            sync::sync_set_options,
            sync::sync_now,
            sync::open_github,
        ])
        .run(tauri::generate_context!())
        .expect("error while running DexSync");
}
