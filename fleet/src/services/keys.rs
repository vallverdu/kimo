//! The keys the coordinator holds for each machine's kimo (`keys.json`, mode 600): fleet
//! keys from one-way pairing, never in the hand-edited registry.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct Keys {
    path: PathBuf,
}

impl Keys {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("keys.json"),
        }
    }

    pub fn load(&self) -> BTreeMap<String, String> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Write them whole, readable only by this user.
    pub fn save(&self, keys: &BTreeMap<String, String>) -> Result<()> {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = self
            .path
            .with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.write_all(&serde_json::to_vec_pretty(keys)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_whole() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let keys = Keys::new(dir.path());
        assert!(keys.load().is_empty());
        let mut k = BTreeMap::new();
        k.insert("nuc".to_string(), "kimo_fleet_x".to_string());
        keys.save(&k).unwrap();
        assert_eq!(keys.load(), k);
        let mode = std::fs::metadata(dir.path().join("keys.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
