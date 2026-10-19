//! Where the GitHub token lives: the OS keychain (Keychain, Credential
//! Manager, Secret Service). If no keychain is available (e.g. a headless
//! Linux box), fall back to a file only the user can read.

use std::path::PathBuf;

const SERVICE: &str = "dexsync";
const USER: &str = "github";

pub struct TokenStore {
    fallback: PathBuf,
}

impl TokenStore {
    /// `fallback` is used only when the keychain isn't available.
    pub fn new(fallback: PathBuf) -> Self {
        TokenStore { fallback }
    }

    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, USER).ok()
    }

    pub fn load(&self) -> Option<String> {
        if let Some(t) = Self::entry().and_then(|e| e.get_password().ok()) {
            return Some(t);
        }
        std::fs::read_to_string(&self.fallback).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }

    pub fn save(&self, token: &str) -> std::io::Result<()> {
        if Self::entry().is_some_and(|e| e.set_password(token).is_ok()) {
            let _ = std::fs::remove_file(&self.fallback);
            return Ok(());
        }
        if let Some(dir) = self.fallback.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_private(&self.fallback, token)
    }

    pub fn clear(&self) {
        if let Some(e) = Self::entry() {
            let _ = e.delete_credential();
        }
        let _ = std::fs::remove_file(&self.fallback);
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    // The user's AppData folder is already private to them on Windows.
    std::fs::write(path, text)
}
