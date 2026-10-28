//! When this machine may be switched off: never under running work or a person using it.
//! The coordinator checks first, and kimo checks again before it shuts the machine down.

use super::node::Login;
use serde::Serialize;

/// Something that keeps the machine on.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Blocker {
    /// `session`, `job` or `login`.
    pub kind: &'static str,
    /// What, in plain words: `claude api (running)`, `job train-1`, `jordi on tty2`.
    pub what: String,
}

/// A running or paused terminal blocks (a paused agent still holds its work in memory).
pub fn blockers(sessions: &[(String, bool)], jobs: &[String], logins: &[Login]) -> Vec<Blocker> {
    let mut b: Vec<Blocker> = sessions
        .iter()
        .map(|(name, paused)| Blocker {
            kind: "session",
            what: format!("{name} ({})", if *paused { "paused" } else { "running" }),
        })
        .collect();
    b.extend(jobs.iter().map(|j| Blocker {
        kind: "job",
        what: format!("job {j}"),
    }));
    b.extend(logins.iter().map(|l| Blocker {
        kind: "login",
        what: match &l.from {
            Some(from) => format!("{} on {} from {from}", l.user, l.line),
            None => format!("{} on {}", l.user, l.line),
        },
    }));
    b
}

/// The command that switches the machine off, as `sh -c` runs it.
pub fn default_shutdown_command(os: &str) -> &'static str {
    match os {
        "macos" => "osascript -e 'tell application \"System Events\" to shut down'",
        // A user service has no seat: polkit may refuse, then a sudoers rule for
        // `systemctl poweroff` lets it through without a password.
        _ => "systemctl poweroff || sudo -n systemctl poweroff",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_keeps_the_machine_on() {
        let b = blockers(
            &[("claude api".into(), false), ("codex web".into(), true)],
            &["train-1".into()],
            &[
                Login {
                    user: "jordi".into(),
                    line: "tty2".into(),
                    from: None,
                },
                Login {
                    user: "jordi".into(),
                    line: "pts/0".into(),
                    from: Some("10.147.17.5".into()),
                },
            ],
        );
        let what: Vec<&str> = b.iter().map(|b| b.what.as_str()).collect();
        assert_eq!(
            what,
            [
                "claude api (running)",
                "codex web (paused)",
                "job train-1",
                "jordi on tty2",
                "jordi on pts/0 from 10.147.17.5"
            ]
        );
        assert!(blockers(&[], &[], &[]).is_empty());
        assert!(default_shutdown_command("linux").contains("systemctl poweroff"));
    }
}
