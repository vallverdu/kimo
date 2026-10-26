//! Account config homes on disk: one directory per account with its own credentials and
//! links to the shared conversations/settings of the provider's default home.

use crate::core::launch::{
    account_home, claude_login, conversations_dir, credentials_file, default_home, shared_entries,
};
use crate::core::models::{Account, Provider};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

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

/// Whether the account is signed in, and as whom (Claude records the email).
pub fn login(data_dir: &Path, account: &Account) -> (bool, Option<String>) {
    let home = account_home(data_dir, &account.id);
    login_in(&home, &home.join(".claude.json"), account.provider)
}

/// This computer's own login of a provider (`~/.claude.json` + `~/.claude`, `~/.codex`).
pub fn system_login(user_home: &Path, provider: Provider) -> (bool, Option<String>) {
    login_in(
        &default_home(provider, user_home),
        &user_home.join(".claude.json"),
        provider,
    )
}

/// The login of a config home: credentials file, or (Claude) the account in its config file.
/// Claude's config file can be large: it is read again only when it changes.
fn login_in(home: &Path, claude_json: &Path, provider: Provider) -> (bool, Option<String>) {
    type Seen = HashMap<PathBuf, (SystemTime, Option<String>)>;
    static SEEN: Mutex<Option<Seen>> = Mutex::new(None);
    let email = match provider {
        Provider::Claude => {
            let modified = fs::metadata(claude_json).and_then(|m| m.modified()).ok();
            let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
            let seen = seen.get_or_insert_with(HashMap::new);
            match (modified, seen.get(claude_json)) {
                (Some(m), Some((at, email))) if *at == m => email.clone(),
                (Some(m), _) => {
                    let email = fs::read_to_string(claude_json)
                        .ok()
                        .and_then(|text| claude_login(&text));
                    seen.insert(claude_json.to_path_buf(), (m, email.clone()));
                    email
                }
                (None, _) => None,
            }
        }
        Provider::Codex => None,
    };
    let file = home.join(credentials_file(provider)).exists();
    (file || email.is_some(), email.filter(|e| !e.is_empty()))
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
        assert!(!login(&data, &acc).0);
        fs::write(home.join(".credentials.json"), "{}").unwrap();
        assert!(login(&data, &acc).0);
        fs::remove_file(home.join(".credentials.json")).unwrap();
        // macOS: credentials in the Keychain, the login recorded in .claude.json.
        fs::write(
            home.join(".claude.json"),
            r#"{"oauthAccount":{"emailAddress":"me@example.com"}}"#,
        )
        .unwrap();
        assert_eq!(login(&data, &acc), (true, Some("me@example.com".into())));

        remove_home(&data, "work").unwrap();
        assert!(!home.exists());
        assert!(user.join(".claude/settings.json").exists());
        assert!(user.join(".claude/projects").is_dir());
    }
}
