//! The only module that reads and writes `state.json` (atomic writes).

use crate::core::models::State;
use anyhow::{Context, Result};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub struct Store {
    path: PathBuf,
}

impl Store {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load the state; a missing file is an empty state.
    pub fn load(&self) -> Result<State> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("reading {}", self.path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", self.path.display())),
        }
    }

    pub fn save(&self, state: &State) -> Result<()> {
        if let Some(dir) = self.path.parent() {
            // kimo's data folder holds keys: only its owner may enter it (older installs
            // made it readable by everyone).
            match fs::metadata(dir) {
                Ok(m) if m.permissions().mode() & 0o077 != 0 => {
                    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
                }
                Ok(_) => {}
                Err(_) => fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)?,
            }
        }
        let body = serde_json::to_vec_pretty(state)?;
        let tmp = self.path.with_extension(format!(
            "json.{}-{}.tmp",
            std::process::id(),
            SAVES.fetch_add(1, Ordering::Relaxed)
        ));
        let write = || -> Result<()> {
            let mut file = create_private(&tmp)?;
            file.write_all(&body)?;
            file.sync_all()?;
            Ok(())
        };
        if let Err(e) = write() {
            let _ = fs::remove_file(&tmp);
            return Err(e).with_context(|| format!("writing {}", tmp.display()));
        }
        fs::rename(&tmp, &self.path).with_context(|| format!("writing {}", self.path.display()))
    }
}

static SAVES: AtomicU64 = AtomicU64::new(0);

/// A new file only its owner can read, private from its creation on (before any key is
/// written to it); never an existing file or a symlink.
fn create_private(path: &Path) -> Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .with_context(|| format!("creating {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_and_save_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("sub/state.json"));
        assert_eq!(store.load().unwrap(), State::default());
        let mut state = State::default();
        state.extra.insert("note".into(), "kept".into());
        store.save(&state).unwrap();
        assert_eq!(store.load().unwrap(), state);
    }

    #[test]
    fn state_is_private_from_creation() {
        let dir = tempfile::tempdir().unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        // The temporary file is 0600 before anything is written to it.
        let tmp = dir.path().join("x.tmp");
        let file = create_private(&tmp).unwrap();
        assert_eq!(file.metadata().unwrap().len(), 0);
        assert_eq!(mode(&tmp), 0o600);
        // Never over an existing file or through a symlink.
        assert!(create_private(&tmp).is_err());
        std::os::unix::fs::symlink(dir.path().join("elsewhere"), dir.path().join("link")).unwrap();
        assert!(create_private(&dir.path().join("link")).is_err());
        assert!(!dir.path().join("elsewhere").exists());

        let store = Store::new(dir.path().join("data/state.json"));
        store.save(&State::default()).unwrap();
        assert_eq!(mode(&dir.path().join("data")), 0o700);
        // An older, open data folder is closed on the next save.
        fs::set_permissions(dir.path().join("data"), fs::Permissions::from_mode(0o755)).unwrap();
        store.save(&State::default()).unwrap();
        assert_eq!(mode(&dir.path().join("data")), 0o700);
        assert_eq!(mode(&dir.path().join("data/state.json")), 0o600);
        // No temporary file left behind.
        assert_eq!(fs::read_dir(dir.path().join("data")).unwrap().count(), 1);
    }
}
