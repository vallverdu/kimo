//! How to launch a session: program, arguments and environment, without touching the system.

use super::models::{Account, AgentPrefs, Permissions, Provider, SessionKind, SessionSpec};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: String,
}

/// Environment variable that points each CLI to its config home (credentials + history).
pub fn config_env_var(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "CLAUDE_CONFIG_DIR",
        Provider::Codex => "CODEX_HOME",
    }
}

/// Directory inside a config home holding the conversations; kimo links it to the default
/// one so every account can continue every conversation.
pub fn conversations_dir(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => "projects",
        Provider::Codex => "sessions",
    }
}

/// Entries linked from the default config home into every account home, so all accounts
/// share conversations and user settings but keep their own credentials.
pub fn shared_entries(provider: Provider) -> &'static [&'static str] {
    match provider {
        Provider::Claude => &[
            "projects",
            "settings.json",
            "CLAUDE.md",
            "agents",
            "commands",
            "skills",
        ],
        Provider::Codex => &["sessions", "config.toml", "AGENTS.md", "prompts"],
    }
}

/// File whose presence means the account is signed in.
pub fn credentials_file(provider: Provider) -> &'static str {
    match provider {
        Provider::Claude => ".credentials.json",
        Provider::Codex => "auth.json",
    }
}

/// The account a Claude config (`.claude.json` in the config home) is signed in with: its
/// email, or `Some("")` when signed in without one. On macOS Claude keeps the credentials in
/// the Keychain, so this record is the sign of a login that works on every system.
pub fn claude_login(config_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(config_json).ok()?;
    let account = v.get("oauthAccount").filter(|a| a.is_object())?;
    Some(
        account["emailAddress"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

/// The Keychain entry where Claude Code keeps a login on macOS: plain for the default config
/// folder, suffixed with a hash of `CLAUDE_CONFIG_DIR` for other folders (kimo's accounts).
pub fn claude_keychain_service(config_dir: Option<&str>) -> String {
    match config_dir {
        None => "Claude Code-credentials".into(),
        Some(dir) => format!(
            "Claude Code-credentials-{}",
            &super::sha256::hex(dir.as_bytes())[..8]
        ),
    }
}

/// The OAuth access token in Claude Code's stored credentials (Keychain entry or
/// `.credentials.json`).
pub fn claude_access_token(credentials_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(credentials_json.trim()).ok()?;
    v["claudeAiOauth"]["accessToken"]
        .as_str()
        .filter(|t| !t.is_empty())
        .map(String::from)
}

/// Whether a session is the terminal that signs an account in (`claude auth login`).
pub fn is_login(spec: &SessionSpec) -> bool {
    spec.account.is_some()
        && spec.command.as_ref().is_some_and(|c| {
            [Provider::Claude, Provider::Codex]
                .iter()
                .any(|p| *c == login_command(*p))
        })
}

/// Command that signs an account in (run in a kimo terminal with the account's env).
pub fn login_command(provider: Provider) -> Vec<String> {
    match provider {
        Provider::Claude => vec!["claude".into(), "auth".into(), "login".into()],
        Provider::Codex => vec!["codex".into(), "login".into()],
    }
}

/// Variables of kimo's own environment that must not reach sessions: state of a parent agent
/// session (when kimo was started from inside Claude Code), config homes kimo sets itself per
/// account, and kimo's access token.
pub fn is_private_env(key: &str) -> bool {
    key == "CLAUDECODE"
        || key.starts_with("CLAUDE_CODE_")
        || matches!(
            key,
            "CLAUDE_PID" | "CLAUDE_EFFORT" | "CLAUDE_CONFIG_DIR" | "CODEX_HOME"
        )
        || key.starts_with("KIMO_")
}

/// Default config home of a provider (where the user's normal login lives).
pub fn default_home(provider: Provider, user_home: &Path) -> PathBuf {
    match provider {
        Provider::Claude => user_home.join(".claude"),
        Provider::Codex => user_home.join(".codex"),
    }
}

/// Config home kimo creates for an account.
pub fn account_home(data_dir: &Path, account_id: &str) -> PathBuf {
    data_dir.join("accounts").join(account_id)
}

/// Build the launch for `spec`.
/// `account` must be the spec's account (already resolved), `resume` continues the session's
/// conversation (its own id for Claude, the folder's last one for Codex), `shell` is the
/// user's login shell.
/// Arguments the agent preferences add to every agent session (not to custom commands).
pub fn pref_args(provider: Provider, prefs: &AgentPrefs) -> Vec<String> {
    let mut args = vec![];
    if prefs.permissions == Permissions::Yolo {
        args.push(match provider {
            Provider::Claude => "--dangerously-skip-permissions".into(),
            Provider::Codex => "--dangerously-bypass-approvals-and-sandbox".into(),
        });
    }
    if !prefs.model.trim().is_empty() {
        args.push(match provider {
            Provider::Claude => "--model".into(),
            Provider::Codex => "-m".into(),
        });
        args.push(prefs.model.trim().to_string());
    }
    args.extend(prefs.args.iter().cloned());
    args
}

pub fn build(
    spec: &SessionSpec,
    account: Option<&Account>,
    data_dir: &Path,
    resume: bool,
    shell: &str,
    prefs: Option<&AgentPrefs>,
) -> Result<Launch, String> {
    let mut env = vec![
        ("TERM".to_string(), "xterm-256color".to_string()),
        ("COLORTERM".to_string(), "truecolor".to_string()),
        ("KIMO_SESSION".to_string(), spec.id.clone()),
    ];
    if let Some(acc) = account {
        if spec.kind.provider() != Some(acc.provider) {
            return Err(format!(
                "account '{}' is for {:?}, not for a {:?} session",
                acc.name, acc.provider, spec.kind
            ));
        }
        env.push((
            config_env_var(acc.provider).to_string(),
            account_home(data_dir, &acc.id).display().to_string(),
        ));
    }

    let (program, mut args) = match &spec.command {
        Some(cmd) if !cmd.is_empty() => (cmd[0].clone(), cmd[1..].to_vec()),
        _ => match spec.kind {
            SessionKind::Shell => (shell.to_string(), vec![]),
            SessionKind::Claude => {
                let args = match (&spec.conversation, resume) {
                    (Some(id), true) => vec!["--resume".to_string(), id.clone()],
                    (Some(id), false) => vec!["--session-id".to_string(), id.clone()],
                    (None, true) => vec!["--continue".to_string()],
                    (None, false) => vec![],
                };
                ("claude".to_string(), args)
            }
            SessionKind::Codex => {
                // Codex picks its own id: kimo learns it from Codex's log after the first start.
                let args = match (&spec.conversation, resume) {
                    (Some(id), true) => vec!["resume".to_string(), id.clone()],
                    (None, true) => vec!["resume".to_string(), "--last".to_string()],
                    (_, false) => vec![],
                };
                ("codex".to_string(), args)
            }
        },
    };
    if let (None, Some(provider), Some(prefs)) = (&spec.command, spec.kind.provider(), prefs) {
        args.extend(pref_args(provider, prefs));
    }
    args.extend(spec.args.iter().cloned());
    Ok(Launch {
        program,
        args,
        env,
        cwd: spec.cwd.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn spec(kind: SessionKind, account: Option<&str>) -> SessionSpec {
        SessionSpec {
            id: "s1".into(),
            name: "api".into(),
            kind,
            cwd: "/work".into(),
            account: account.map(String::from),
            args: vec![],
            command: None,
            started_once: false,
            conversation: None,
            resume_on_start: false,
            freed: None,
            extra: Map::new(),
        }
    }

    fn account(id: &str, provider: Provider) -> Account {
        Account {
            id: id.into(),
            name: id.into(),
            provider,
            extra: Map::new(),
        }
    }

    #[test]
    fn claude_with_account_gets_its_config_dir_and_continue() {
        let acc = account("work", Provider::Claude);
        let l = build(
            &spec(SessionKind::Claude, Some("work")),
            Some(&acc),
            Path::new("/k"),
            true,
            "sh",
            None,
        )
        .unwrap();
        assert_eq!(l.program, "claude");
        assert_eq!(l.args, vec!["--continue"]);
        assert!(l
            .env
            .contains(&("CLAUDE_CONFIG_DIR".into(), "/k/accounts/work".into())));
        assert_eq!(l.cwd, "/work");
    }

    #[test]
    fn claude_owns_its_conversation_id() {
        let mut s = spec(SessionKind::Claude, None);
        s.conversation = Some("c0ffee".into());
        let fresh = build(&s, None, Path::new("/k"), false, "sh", None).unwrap();
        assert_eq!(fresh.args, vec!["--session-id", "c0ffee"]);
        let again = build(&s, None, Path::new("/k"), true, "sh", None).unwrap();
        assert_eq!(again.args, vec!["--resume", "c0ffee"]);
    }

    #[test]
    fn codex_resumes_its_own_conversation_once_known() {
        let mut s = spec(SessionKind::Codex, None);
        s.conversation = Some("019a-codex-id".into());
        let l = build(&s, None, Path::new("/k"), true, "sh", None).unwrap();
        assert_eq!(l.args, vec!["resume", "019a-codex-id"]);
        assert!(build(&s, None, Path::new("/k"), false, "sh", None)
            .unwrap()
            .args
            .is_empty());
    }

    #[test]
    fn codex_resume_uses_last_session() {
        let l = build(
            &spec(SessionKind::Codex, None),
            None,
            Path::new("/k"),
            true,
            "sh",
            None,
        )
        .unwrap();
        assert_eq!(
            (l.program.as_str(), l.args.clone()),
            ("codex", vec!["resume".into(), "--last".into()])
        );
        assert!(!l.env.iter().any(|(k, _)| k == "CODEX_HOME"));
    }

    #[test]
    fn fresh_start_has_no_resume_and_keeps_extra_args() {
        let mut s = spec(SessionKind::Claude, None);
        s.args = vec!["--model".into(), "opus".into()];
        let l = build(&s, None, Path::new("/k"), false, "sh", None).unwrap();
        assert_eq!(l.args, vec!["--model", "opus"]);
    }

    #[test]
    fn shell_and_command_override() {
        let l = build(
            &spec(SessionKind::Shell, None),
            None,
            Path::new("/k"),
            true,
            "/bin/zsh",
            None,
        )
        .unwrap();
        assert_eq!((l.program.as_str(), l.args.len()), ("/bin/zsh", 0));
        let mut s = spec(SessionKind::Shell, None);
        s.command = Some(vec!["htop".into(), "-d".into(), "10".into()]);
        let l = build(&s, None, Path::new("/k"), false, "sh", None).unwrap();
        assert_eq!(
            (l.program.as_str(), l.args.clone()),
            ("htop", vec!["-d".into(), "10".into()])
        );
    }

    #[test]
    fn parent_agent_state_and_token_are_private() {
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
            "KIMO_TOKEN",
        ] {
            assert!(is_private_env(key), "{key}");
        }
        for key in [
            "PATH",
            "HOME",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "CLAUDE",
        ] {
            assert!(!is_private_env(key), "{key}");
        }
    }

    #[test]
    fn agent_preferences_add_permissions_model_and_args() {
        let prefs = AgentPrefs {
            permissions: Permissions::Yolo,
            model: "opus".into(),
            args: vec!["--verbose".into()],
            ..AgentPrefs::default()
        };
        let mut s = spec(SessionKind::Claude, None);
        s.args = vec!["--x".into()];
        let l = build(&s, None, Path::new("/k"), false, "sh", Some(&prefs)).unwrap();
        assert_eq!(
            l.args,
            vec![
                "--dangerously-skip-permissions",
                "--model",
                "opus",
                "--verbose",
                "--x"
            ]
        );
        let codex = build(
            &spec(SessionKind::Codex, None),
            None,
            Path::new("/k"),
            true,
            "sh",
            Some(&prefs),
        )
        .unwrap();
        assert_eq!(
            codex.args,
            vec![
                "resume",
                "--last",
                "--dangerously-bypass-approvals-and-sandbox",
                "-m",
                "opus",
                "--verbose"
            ]
        );
        // Custom commands (logins, clones) never get agent preferences.
        let mut login = spec(SessionKind::Claude, None);
        login.command = Some(vec!["claude".into(), "auth".into(), "login".into()]);
        assert_eq!(
            build(&login, None, Path::new("/k"), false, "sh", Some(&prefs))
                .unwrap()
                .args,
            vec!["auth", "login"]
        );
    }

    #[test]
    fn provider_mismatch_is_refused() {
        let acc = account("c", Provider::Codex);
        assert!(build(
            &spec(SessionKind::Claude, Some("c")),
            Some(&acc),
            Path::new("/k"),
            false,
            "sh",
            None,
        )
        .is_err());
    }

    #[test]
    fn claude_logins_in_the_keychain() {
        assert_eq!(claude_keychain_service(None), "Claude Code-credentials");
        let s = claude_keychain_service(Some("/k/accounts/work"));
        assert!(
            s.starts_with("Claude Code-credentials-") && s.len() == 32,
            "{s}"
        );
        assert_eq!(
            claude_access_token(r#"{"claudeAiOauth":{"accessToken":"t1","refreshToken":"r"}}"#),
            Some("t1".into())
        );
        assert_eq!(claude_access_token("{}"), None);
    }

    #[test]
    fn login_sessions() {
        let mut s = spec(SessionKind::Claude, Some("work"));
        assert!(!is_login(&s));
        s.command = Some(login_command(Provider::Claude));
        assert!(is_login(&s));
        s.account = None;
        assert!(!is_login(&s));
    }
}
