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
                let args = if resume {
                    vec!["resume".to_string(), "--last".to_string()]
                } else {
                    vec![]
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
}
