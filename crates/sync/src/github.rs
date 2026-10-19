//! Minimal GitHub client: device-flow sign-in, user/installation lookups and
//! the Contents API. Blocking; callers run it off the UI thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use ureq::Agent;

use crate::SyncError;

const API: &str = "https://api.github.com";
const USER_AGENT: &str = concat!("dexsync/", env!("CARGO_PKG_VERSION"));

fn agent() -> Agent {
    Agent::config_builder().http_status_as_error(false).timeout_global(Some(Duration::from_secs(30))).build().into()
}

fn http(e: ureq::Error) -> SyncError {
    SyncError::Http(e.to_string())
}

// ── Device flow ─────────────────────────────────────────────────────────

/// What the user needs to see: go to `verification_uri`, enter `user_code`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub fn start_device_flow(client_id: &str) -> Result<DeviceCode, SyncError> {
    let mut resp = agent()
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", USER_AGENT)
        .send_form([("client_id", client_id)])
        .map_err(http)?;
    let body: Value = resp.body_mut().read_json().map_err(http)?;
    if let Some(e) = body.get("error").and_then(Value::as_str) {
        return Err(SyncError::Auth(format!("{e}: {}", body["error_description"].as_str().unwrap_or(""))));
    }
    serde_json::from_value(body).map_err(|e| SyncError::Http(e.to_string()))
}

/// Poll until the user approves (returns the access token), denies, the
/// code expires, or `cancel` is set.
pub fn poll_device_flow(client_id: &str, code: &DeviceCode, cancel: &AtomicBool) -> Result<String, SyncError> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = code.interval.max(5);
    loop {
        // Sleep in small steps so cancelling is quick.
        let wake = Instant::now() + Duration::from_secs(interval);
        while Instant::now() < wake {
            if cancel.load(Ordering::Relaxed) {
                return Err(SyncError::Cancelled);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        if Instant::now() > deadline {
            return Err(SyncError::Auth("the code expired; start again".into()));
        }
        let mut resp = agent()
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("User-Agent", USER_AGENT)
            .send_form([
                ("client_id", client_id),
                ("device_code", code.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .map_err(http)?;
        let body: Value = resp.body_mut().read_json().map_err(http)?;
        if let Some(token) = body.get("access_token").and_then(Value::as_str) {
            return Ok(token.to_string());
        }
        match body.get("error").and_then(Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("expired_token") => return Err(SyncError::Auth("the code expired; start again".into())),
            Some("access_denied") => return Err(SyncError::Auth("access was denied on GitHub".into())),
            other => return Err(SyncError::Auth(format!("unexpected response: {}", other.unwrap_or("no error field")))),
        }
    }
}

// ── API client ──────────────────────────────────────────────────────────

/// A file as stored in the repo.
#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub sha: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: String,
}

pub struct GitHub {
    agent: Agent,
    token: String,
}

impl GitHub {
    pub fn new(token: impl Into<String>) -> Self {
        GitHub { agent: agent(), token: token.into() }
    }

    fn get(&self, path: &str) -> Result<ureq::http::Response<ureq::Body>, SyncError> {
        self.agent
            .get(format!("{API}{path}"))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", USER_AGENT)
            .call()
            .map_err(http)
    }

    fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, SyncError> {
        let mut resp = self.get(path)?;
        check(resp.status().as_u16(), path)?;
        resp.body_mut().read_json().map_err(http)
    }

    /// The signed-in user's login name.
    pub fn login(&self) -> Result<String, SyncError> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        Ok(self.get_json::<User>("/user")?.login)
    }

    /// `owner/name` of every repo the user installed the app on.
    /// Only works with GitHub App user tokens, not personal access tokens.
    pub fn installed_repos(&self) -> Result<Vec<String>, SyncError> {
        #[derive(Deserialize)]
        struct Installations {
            installations: Vec<Installation>,
        }
        #[derive(Deserialize)]
        struct Installation {
            id: u64,
        }
        #[derive(Deserialize)]
        struct Repos {
            repositories: Vec<Repo>,
        }
        #[derive(Deserialize)]
        struct Repo {
            full_name: String,
        }
        let mut out = Vec::new();
        for i in self.get_json::<Installations>("/user/installations?per_page=100")?.installations {
            let repos: Repos = self.get_json(&format!("/user/installations/{}/repositories?per_page=100", i.id))?;
            out.extend(repos.repositories.into_iter().map(|r| r.full_name));
        }
        out.sort();
        Ok(out)
    }

    /// Read a file, or `None` if it doesn't exist (or the repo is empty).
    pub fn read_file(&self, repo: &str, path: &str) -> Result<Option<RemoteFile>, SyncError> {
        let url = format!("/repos/{repo}/contents/{path}");
        let mut resp = self.get(&url)?;
        if resp.status().as_u16() == 404 {
            return Ok(None);
        }
        check(resp.status().as_u16(), &url)?;
        let body: Value = resp.body_mut().read_json().map_err(http)?;
        let sha = body["sha"].as_str().unwrap_or_default().to_string();
        let b64: String = body["content"].as_str().unwrap_or_default().split_whitespace().collect();
        let bytes = B64.decode(b64).map_err(|e| SyncError::Http(format!("{path}: bad base64: {e}")))?;
        Ok(Some(RemoteFile { sha, bytes }))
    }

    /// List a folder, or an empty list if it doesn't exist yet.
    pub fn list_dir(&self, repo: &str, path: &str) -> Result<Vec<DirEntry>, SyncError> {
        let url = format!("/repos/{repo}/contents/{path}");
        let mut resp = self.get(&url)?;
        if resp.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        check(resp.status().as_u16(), &url)?;
        resp.body_mut().read_json().map_err(http)
    }

    /// Create or update a file. `sha` is the current blob sha when updating.
    pub fn write_file(&self, repo: &str, path: &str, bytes: &[u8], sha: Option<&str>, message: &str) -> Result<(), SyncError> {
        let url = format!("/repos/{repo}/contents/{path}");
        let mut body = json!({ "message": message, "content": B64.encode(bytes) });
        if let Some(sha) = sha {
            body["sha"] = json!(sha);
        }
        let resp = self
            .agent
            .put(format!("{API}{url}"))
            .header("Accept", "application/vnd.github+json")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("User-Agent", USER_AGENT)
            .send_json(&body)
            .map_err(http)?;
        match resp.status().as_u16() {
            // 409/422: the sha we sent is stale (someone else wrote first).
            409 | 422 if sha.is_some() => Err(SyncError::Conflict(path.into())),
            s => check(s, &url),
        }
    }

    /// Write only if the content differs; retries once on a stale sha.
    /// Returns whether anything was written.
    pub fn put_if_changed(&self, repo: &str, path: &str, bytes: &[u8], message: &str) -> Result<bool, SyncError> {
        for _ in 0..2 {
            let current = self.read_file(repo, path)?;
            if current.as_ref().is_some_and(|f| f.sha == blob_sha(bytes)) {
                return Ok(false);
            }
            match self.write_file(repo, path, bytes, current.as_ref().map(|f| f.sha.as_str()), message) {
                Err(SyncError::Conflict(_)) => continue,
                other => return other.map(|_| true),
            }
        }
        Err(SyncError::Conflict(path.into()))
    }
}

fn check(status: u16, what: &str) -> Result<(), SyncError> {
    match status {
        200..=299 => Ok(()),
        401 => Err(SyncError::Auth("GitHub rejected the token; connect again".into())),
        403 | 404 => Err(SyncError::NoAccess(what.into())),
        s => Err(SyncError::Status(s, what.into())),
    }
}

/// Git's blob hash, so we can tell if a file changed without re-uploading it.
pub fn blob_sha(bytes: &[u8]) -> String {
    let mut h = Sha1::new();
    h.update(format!("blob {}\0", bytes.len()).as_bytes());
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_sha_matches_git() {
        // `printf 'hello\n' | git hash-object --stdin`
        assert_eq!(blob_sha(b"hello\n"), "ce013625030ba8dba906f756967f9e9ca394464a");
        assert_eq!(blob_sha(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }
}
