//! The only module that reads and writes `state.json` (atomic writes).

use crate::core::models::State;
use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

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
            fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
        // App keys live here: only the owner may read it.
        fs::set_permissions(&tmp, std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        fs::rename(&tmp, &self.path).with_context(|| format!("writing {}", self.path.display()))
    }
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
}
