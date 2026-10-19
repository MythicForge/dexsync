//! GitHub sync: connect (device flow or personal token), pick the repo, and a
//! background worker that syncs shortly after every dex change.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dexsync_sources::Config;
use dexsync_sync::{GitHub, Options, RepoRemote, TokenStore, device_id};
use dexsync_watcher::{State, device_name};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::{AppState, Res, err, load_config};

/// Wait for things to settle after a dex change before syncing.
const DEBOUNCE: Duration = Duration::from_secs(5);

pub enum Job {
    /// After a dex change: debounced.
    Soon,
    /// The user pressed "Sync now".
    Now,
}

#[derive(Default)]
pub struct SyncState {
    pub jobs: Mutex<Option<Sender<Job>>>,
    login: Mutex<Option<String>>,
    last: Mutex<Option<LastSync>>,
    connecting: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct LastSync {
    at: u64,
    ok: bool,
    message: String,
    written: usize,
    devices: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// A Client ID is available (built in or configured).
    can_connect: bool,
    client_id: Option<String>,
    connected: bool,
    login: Option<String>,
    repo: Option<String>,
    backups: bool,
    device: String,
    new_repo_url: String,
    install_url: Option<String>,
    last: Option<LastSync>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectCode {
    user_code: String,
    verification_uri: String,
}

fn tokens() -> Res<TokenStore> {
    dexsync_sync::token_fallback_path().map(TokenStore::new).ok_or_else(|| "no data directory".into())
}

fn client_id(config: &Config) -> Option<String> {
    config.sync.client_id.clone().or(dexsync_sync::DEFAULT_CLIENT_ID.map(String::from))
}

fn device(config: &Config) -> String {
    device_id(config.sync.device.as_deref().unwrap_or(&device_name()))
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn save_config(app: &AppState, f: impl FnOnce(&mut Config)) -> Res {
    let mut config = load_config(app)?;
    f(&mut config);
    config.save(&app.paths.config).map_err(err)
}

#[tauri::command]
pub fn sync_status(app: tauri::State<AppState>) -> Res<SyncStatus> {
    let config = load_config(&app)?;
    let connected = tokens()?.load().is_some();
    let slug = config.sync.app_slug.clone().or(dexsync_sync::DEFAULT_APP_SLUG.map(String::from));
    Ok(SyncStatus {
        can_connect: client_id(&config).is_some(),
        client_id: config.sync.client_id.clone(),
        connected,
        login: if connected { app.sync.login.lock().unwrap().clone() } else { None },
        repo: config.sync.repo.clone(),
        backups: config.sync.backups,
        device: device(&config),
        new_repo_url: "https://github.com/new?name=dexsync-data&visibility=private&description=DexSync+data".into(),
        install_url: slug.map(|s| format!("https://github.com/apps/{s}/installations/new")),
        last: app.sync.last.lock().unwrap().clone(),
    })
}

/// Start the device flow: returns the code to show, opens the browser, and
/// finishes in the background (emits `sync-auth`).
#[tauri::command]
pub fn sync_connect(handle: AppHandle, app: tauri::State<AppState>) -> Res<ConnectCode> {
    let config = load_config(&app)?;
    let client = client_id(&config).ok_or("No GitHub App Client ID yet. Use a personal access token, or set one under Advanced.")?;
    let code = dexsync_sync::start_device_flow(&client).map_err(err)?;
    let _ = handle.opener().open_url(&code.verification_uri, None::<&str>);

    let cancel = Arc::new(AtomicBool::new(false));
    if let Some(old) = app.sync.connecting.lock().unwrap().replace(cancel.clone()) {
        old.store(true, Ordering::Relaxed);
    }
    let reply = ConnectCode { user_code: code.user_code.clone(), verification_uri: code.verification_uri.clone() };
    std::thread::spawn(move || {
        let result = dexsync_sync::poll_device_flow(&client, &code, &cancel).and_then(|token| {
            let login = GitHub::new(&token).login()?;
            tokens().map_err(dexsync_sync::SyncError::Data)?.save(&token).map_err(|e| dexsync_sync::SyncError::Data(e.to_string()))?;
            Ok(login)
        });
        let app = handle.state::<AppState>();
        match result {
            Ok(login) => {
                *app.sync.login.lock().unwrap() = Some(login.clone());
                let _ = handle.emit("sync-auth", serde_json::json!({ "ok": true, "login": login }));
                request(&app, Job::Now);
            }
            Err(dexsync_sync::SyncError::Cancelled) => {}
            Err(e) => {
                let _ = handle.emit("sync-auth", serde_json::json!({ "ok": false, "error": e.to_string() }));
            }
        }
    });
    Ok(reply)
}

#[tauri::command]
pub fn sync_cancel_connect(app: tauri::State<AppState>) {
    if let Some(flag) = app.sync.connecting.lock().unwrap().take() {
        flag.store(true, Ordering::Relaxed);
    }
}

/// Fallback: a fine-grained personal access token (Contents: read & write on one repo).
#[tauri::command]
pub async fn sync_set_token(app: tauri::State<'_, AppState>, token: String) -> Res<String> {
    let token = token.trim().to_string();
    let login = GitHub::new(&token).login().map_err(err)?;
    tokens()?.save(&token).map_err(err)?;
    *app.sync.login.lock().unwrap() = Some(login.clone());
    Ok(login)
}

#[tauri::command]
pub fn sync_disconnect(app: tauri::State<AppState>) -> Res {
    tokens()?.clear();
    *app.sync.login.lock().unwrap() = None;
    *app.sync.last.lock().unwrap() = None;
    Ok(())
}

/// Repos the user installed the GitHub App on (empty for personal tokens).
#[tauri::command]
pub async fn sync_repos(app: tauri::State<'_, AppState>) -> Res<Vec<String>> {
    let token = tokens()?.load().ok_or("not connected")?;
    let github = GitHub::new(token);
    if app.sync.login.lock().unwrap().is_none() {
        *app.sync.login.lock().unwrap() = github.login().ok();
    }
    github.installed_repos().map_err(err)
}

#[tauri::command]
pub fn sync_set_repo(app: tauri::State<AppState>, repo: Option<String>) -> Res {
    let repo = repo.map(|r| r.trim().trim_start_matches("https://github.com/").trim_end_matches('/').to_string()).filter(|r| !r.is_empty());
    if let Some(r) = &repo
        && r.split('/').filter(|p| !p.is_empty()).count() != 2
    {
        return Err("use the form owner/name".into());
    }
    save_config(&app, |c| c.sync.repo = repo)?;
    request(&app, Job::Now);
    Ok(())
}

#[tauri::command]
pub fn sync_set_options(app: tauri::State<AppState>, backups: bool, client_id: Option<String>, device: Option<String>) -> Res {
    let blank = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    save_config(&app, |c| {
        c.sync.backups = backups;
        c.sync.client_id = blank(client_id);
        c.sync.device = blank(device);
    })
}

#[tauri::command]
pub fn sync_now(app: tauri::State<AppState>) {
    request(&app, Job::Now);
}

/// Only GitHub pages; the UI never opens arbitrary URLs.
#[tauri::command]
pub fn open_github(handle: AppHandle, url: String) -> Res {
    if !url.starts_with("https://github.com/") {
        return Err("not a GitHub link".into());
    }
    handle.opener().open_url(url, None::<&str>).map_err(err)
}

pub fn request(app: &AppState, job: Job) {
    if let Some(tx) = app.sync.jobs.lock().unwrap().as_ref() {
        let _ = tx.send(job);
    }
}

/// One worker thread so syncs never overlap.
pub fn start_worker(handle: AppHandle) -> Sender<Job> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || worker(handle, rx));
    tx
}

fn worker(handle: AppHandle, rx: Receiver<Job>) {
    while let Ok(mut job) = rx.recv() {
        // Debounce: keep waiting while more changes arrive.
        while matches!(job, Job::Soon) {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(next) => job = next,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let app = handle.state::<AppState>();
        let Some(result) = run_sync(&app) else { continue };
        let last = match result {
            Ok(r) => LastSync {
                at: now(),
                ok: true,
                message: if r.warnings.is_empty() { "Synced".into() } else { r.warnings.join("; ") },
                written: r.written.len(),
                devices: r.devices,
            },
            Err(e) => LastSync { at: now(), ok: false, message: e, written: 0, devices: vec![] },
        };
        *app.sync.last.lock().unwrap() = Some(last.clone());
        let _ = handle.emit("sync-done", last);
    }
}

/// `None` when sync isn't set up (no token or no repo): nothing to report.
fn run_sync(app: &AppState) -> Option<Result<dexsync_sync::Report, String>> {
    let config = load_config(app).ok()?;
    let repo = config.sync.repo.clone()?;
    let token = tokens().ok()?.load()?;
    let state = match State::load(&app.paths.state) {
        Ok(s) => s,
        Err(e) => return Some(Err(e.to_string())),
    };
    let github = GitHub::new(token);
    let device = device(&config);
    let opts = Options { device: &device, backups: config.sync.backups };
    Some(dexsync_sync::sync(&RepoRemote { github: &github, repo: &repo }, &opts, &state).map_err(err))
}
