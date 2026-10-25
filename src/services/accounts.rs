//! Account config homes on disk: one directory per account with its own credentials and
//! links to the shared conversations/settings of the provider's default home.

use crate::core::launch::{
    account_home, conversations_dir, credentials_file, default_home, shared_entries,
};
use crate::core::models::Account;
use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

/// Create the account home and its shared links (idempotent; existing entries are left alone).
pub fn ensure_home(data_dir: &Path, user_home: &Path, account: &Account) -> Result<()> {
    let home = account_home(data_dir, &account.id);
    fs::create_dir_all(&home).with_context(|| format!("creating {}", home.display()))?;
    let default = default_home(account.provider, user_home);
    let conversations = default.join(conversations_dir(account.provider));
    fs::create_dir_all(&conversations)
        .with_context(|| format!("creating {}", conversations.display()))?;
    for entry in shared_entries(account.provider) {
        let source = default.join(entry);
        let link = home.join(entry);
        if source.exists() && fs::symlink_metadata(&link).is_err() {
            std::os::unix::fs::symlink(&source, &link)
                .with_context(|| format!("linking {} -> {}", link.display(), source.display()))?;
        }
    }
    Ok(())
}

pub fn is_signed_in(data_dir: &Path, account: &Account) -> bool {
    account_home(data_dir, &account.id)
        .join(credentials_file(account.provider))
        .exists()
}

/// Remove the account home (its credentials). Links are removed, never their targets.
pub fn remove_home(data_dir: &Path, account_id: &str) -> Result<()> {
    let home = account_home(data_dir, account_id);
    match fs::remove_dir_all(&home) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Provider;

    #[test]
    fn home_links_shared_entries_and_removal_keeps_targets() {
        let tmp = tempfile::tempdir().unwrap();
        let (data, user) = (tmp.path().join("kimo"), tmp.path().join("home"));
        fs::create_dir_all(user.join(".claude")).unwrap();
        fs::write(user.join(".claude/settings.json"), "{}").unwrap();
        let acc = Account {
            id: "work".into(),
            name: "Work".into(),
            provider: Provider::Claude,
            extra: Default::default(),
        };

        ensure_home(&data, &user, &acc).unwrap();
        ensure_home(&data, &user, &acc).unwrap();
        let home = data.join("accounts/work");
        assert!(fs::symlink_metadata(home.join("projects"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(home.join("settings.json").exists());
        assert!(fs::symlink_metadata(home.join("skills")).is_err());
        assert!(!is_signed_in(&data, &acc));
        fs::write(home.join(".credentials.json"), "{}").unwrap();
        assert!(is_signed_in(&data, &acc));

        remove_home(&data, "work").unwrap();
        assert!(!home.exists());
        assert!(user.join(".claude/settings.json").exists());
        assert!(user.join(".claude/projects").is_dir());
    }
}
