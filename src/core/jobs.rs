//! Jobs: a command or a headless agent run to the end, detached from kimo. Everything about a
//! job lives in its folder (`~/.kimo/jobs/<id>/`): what to run, its process, its log and how
//! it ended, so a job survives kimo updating, restarting or crashing. These are the pure
//! parts: the files' contents, the state read from them, and the headless agent commands.

use super::launch::pref_args;
use super::models::{AgentPrefs, Provider};
use serde::{Deserialize, Serialize};

/// What the runner (`kimo job-run`) starts: `spec.json` in the job's folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSpec {
    pub id: String,
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub cwd: String,
    /// Added to kimo's environment.
    #[serde(default)]
    pub env: Vec<(String, String)>,
    /// Ended (and counted as timed out) after this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// The agent of a headless agent job (its answer is the job's result).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<Provider>,
    /// Standard output is the result (`result` in the folder), not part of the log.
    #[serde(default)]
    pub stdout_is_result: bool,
    /// Standard input comes from this file of the folder (an agent's prompt).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stdin_file: Option<String>,
    /// Unix time (ms).
    pub created_at: u64,
    /// The checkout it runs in, when kimo made one (`git` in `POST /api/jobs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<Checkout>,
}

/// A checkout kimo made for a job: a worktree of a repository on the machine, or a clone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Checkout {
    /// The repository: a folder on the machine or a URL.
    pub repo: String,
    /// The commit it is at.
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// A worktree of `repo` (removed with the job), not a clone of it.
    #[serde(default)]
    pub worktree: bool,
}

/// Is `repo` a URL to clone rather than a folder on this machine?
pub fn is_remote_repo(repo: &str) -> bool {
    repo.contains("://") || (repo.contains('@') && repo.contains(':') && !repo.starts_with('/'))
}

/// Branch names kimo creates: git's rules, simply (letters, digits, `-_./`, no `..`).
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '/', '.'])
        && !name.ends_with(['/', '.'])
        && !name.contains("..")
        && !name.contains("//")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./".contains(&b))
}

/// How a job ended: `exit.json`, written once, last.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Exit {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default)]
    pub cancelled: bool,
    /// The program could not be started (its error is in the log).
    #[serde(default)]
    pub not_started: bool,
    pub ended_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Created to start later (`"start": false`): its input files are being put in place.
    Staged,
    /// Created, its runner not up yet.
    Starting,
    Running,
    /// Ended with exit code 0.
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    /// Its runner disappeared without writing how it ended (the machine went off).
    Lost,
}

impl JobState {
    pub fn finished(self) -> bool {
        !matches!(
            self,
            JobState::Staged | JobState::Starting | JobState::Running
        )
    }
}

/// How long a new job may wait for its runner before it counts as lost.
pub const START_GRACE_MS: u64 = 30_000;
/// How long a staged job may wait to be started before it counts as lost.
pub const STAGE_GRACE_MS: u64 = 600_000;

/// The state of a job from what its folder shows. `started_at`: when the runner started
/// (`None`: not yet); `runner_alive`: its process exists; `boot_at`: when the machine started
/// (a runner from before a reboot is gone, whatever its pid says now); `staged`: created to
/// start later.
pub fn state(
    exit: Option<&Exit>,
    created_at: u64,
    started_at: Option<u64>,
    runner_alive: bool,
    boot_at: Option<u64>,
    staged: bool,
    now: u64,
) -> JobState {
    if let Some(e) = exit {
        return if e.cancelled {
            JobState::Cancelled
        } else if e.timed_out {
            JobState::TimedOut
        } else if e.code == Some(0) {
            JobState::Succeeded
        } else {
            JobState::Failed
        };
    }
    match started_at {
        Some(at) if runner_alive && boot_at.is_none_or(|b| at >= b) => JobState::Running,
        Some(_) => JobState::Lost,
        None if staged && now.saturating_sub(created_at) < STAGE_GRACE_MS => JobState::Staged,
        None if !staged && now.saturating_sub(created_at) < START_GRACE_MS => JobState::Starting,
        None => JobState::Lost,
    }
}

/// Job ids name a folder: letters, digits, `-` and `_`, 1 to 64 of them.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The headless agent run for `provider` (program, arguments, stdout is the result). The
/// prompt goes in on standard input, so it is never read as an option nor shown by `ps`.
/// `result_file`: where Codex writes its last message.
pub fn headless_agent(
    provider: Provider,
    prefs: &AgentPrefs,
    result_file: &str,
) -> (String, Vec<String>, bool) {
    match provider {
        // `claude -p` reads the prompt from stdin and prints one JSON object at the end.
        Provider::Claude => {
            let mut args = vec!["-p".into(), "--output-format".into(), "json".into()];
            args.extend(pref_args(provider, prefs));
            ("claude".into(), args, true)
        }
        // `codex exec -` reads the prompt from stdin; its last message goes to a file.
        Provider::Codex => {
            let mut args = vec!["exec".into()];
            args.extend(pref_args(provider, prefs));
            args.extend([
                "--skip-git-repo-check".into(),
                "--output-last-message".into(),
                result_file.into(),
                "-".into(),
            ]);
            ("codex".into(), args, false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::Permissions;

    fn exit(code: i32) -> Exit {
        Exit {
            code: Some(code),
            ended_at: 5,
            ..Default::default()
        }
    }

    #[test]
    fn states_from_the_folder() {
        let now = 100_000;
        assert_eq!(
            state(Some(&exit(0)), 0, Some(1), false, None, false, now),
            JobState::Succeeded
        );
        assert_eq!(
            state(Some(&exit(3)), 0, Some(1), false, None, false, now),
            JobState::Failed
        );
        let cancelled = Exit {
            cancelled: true,
            signal: Some(15),
            ..exit(0)
        };
        assert_eq!(
            state(Some(&cancelled), 0, Some(1), false, None, false, now),
            JobState::Cancelled
        );
        let late = Exit {
            timed_out: true,
            ..exit(124)
        };
        assert_eq!(
            state(Some(&late), 0, Some(1), false, None, false, now),
            JobState::TimedOut
        );
        assert_eq!(
            state(None, 90_000, Some(91_000), true, Some(50_000), false, now),
            JobState::Running
        );
        // The machine restarted since: the pid is someone else's now.
        assert_eq!(
            state(None, 10_000, Some(11_000), true, Some(50_000), false, now),
            JobState::Lost
        );
        assert_eq!(
            state(None, 0, Some(1), false, None, false, now),
            JobState::Lost
        );
        assert_eq!(
            state(None, 90_000, None, false, None, false, now),
            JobState::Starting
        );
        assert_eq!(
            state(None, 0, None, false, None, false, now),
            JobState::Lost
        );
        assert_eq!(
            state(None, 90_000, None, false, None, true, now),
            JobState::Staged
        );
        assert_eq!(
            state(None, 0, None, false, None, true, STAGE_GRACE_MS + 1),
            JobState::Lost
        );
        assert!(!JobState::Staged.finished());
        assert!(JobState::Lost.finished() && !JobState::Running.finished());
    }

    #[test]
    fn repositories_and_branches() {
        assert!(is_remote_repo("https://github.com/vallverdu/kimo"));
        assert!(is_remote_repo("git@github.com:vallverdu/kimo.git"));
        assert!(is_remote_repo("ssh://pi4/srv/git/seg.git"));
        assert!(!is_remote_repo("/home/jordi/dev/seg"));
        assert!(!is_remote_repo("~/dev/seg"));
        assert!(valid_branch("kimo/develop-model"));
        for bad in ["", "-x", "a..b", "a b", "a/", "/a", ".a", "a//b"] {
            assert!(!valid_branch(bad), "{bad}");
        }
    }

    #[test]
    fn ids() {
        assert!(valid_id("train-model_2"));
        for bad in ["", "-x", "a/b", "..", "a b", &"x".repeat(65)] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn headless_agents() {
        let prefs = AgentPrefs {
            permissions: Permissions::Yolo,
            model: "sonnet".into(),
            ..Default::default()
        };
        let (p, args, result) = headless_agent(Provider::Claude, &prefs, "/j/result");
        assert_eq!(p, "claude");
        assert_eq!(
            args,
            [
                "-p",
                "--output-format",
                "json",
                "--dangerously-skip-permissions",
                "--model",
                "sonnet"
            ]
        );
        assert!(result);
        let (p, args, result) =
            headless_agent(Provider::Codex, &AgentPrefs::default(), "/j/result");
        assert_eq!(p, "codex");
        assert_eq!(
            args,
            [
                "exec",
                "--skip-git-repo-check",
                "--output-last-message",
                "/j/result",
                "-"
            ]
        );
        assert!(!result);
    }
}
