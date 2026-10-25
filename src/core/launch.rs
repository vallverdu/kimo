//! How to launch a session: program, arguments and environment, without touching the system.

use super::models::{Account, Provider, SessionKind, SessionSpec};
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
/// `account` must be the spec's account (already resolved), `resume` continues the last
/// conversation in the session's directory, `shell` is the user's login shell.
pub fn build(
    spec: &SessionSpec,
    account: Option<&Account>,
    data_dir: &Path,
    resume: bool,
    shell: &str,
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
                let args = if resume {
                    vec!["--continue".to_string()]
                } else {
                    vec![]
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
    fn codex_resume_uses_last_session() {
        let l = build(
            &spec(SessionKind::Codex, None),
            None,
            Path::new("/k"),
            true,
            "sh",
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
        let l = build(&s, None, Path::new("/k"), false, "sh").unwrap();
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
        )
        .unwrap();
        assert_eq!((l.program.as_str(), l.args.len()), ("/bin/zsh", 0));
        let mut s = spec(SessionKind::Shell, None);
        s.command = Some(vec!["htop".into(), "-d".into(), "10".into()]);
        let l = build(&s, None, Path::new("/k"), false, "sh").unwrap();
        assert_eq!(
            (l.program.as_str(), l.args.clone()),
            ("htop", vec!["-d".into(), "10".into()])
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
            "sh"
        )
        .is_err());
    }
}
