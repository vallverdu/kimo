//! Plain data: accounts, sessions and the persisted state. Unknown JSON fields are preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Which agent CLI an account or a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

/// What runs inside a terminal session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Shell,
    Claude,
    Codex,
}

impl SessionKind {
    pub fn provider(self) -> Option<Provider> {
        match self {
            SessionKind::Shell => None,
            SessionKind::Claude => Some(Provider::Claude),
            SessionKind::Codex => Some(Provider::Codex),
        }
    }
}

/// A subscription login. Its credentials live in its own config home, never in kimo's state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub name: String,
    pub provider: Provider,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Lifecycle of a session.
/// `Paused` keeps the process in memory (SIGSTOP); `Stopped` frees it and resumes the conversation later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionStatus {
    Running,
    Paused,
    Stopped,
    Exited,
}

/// A persisted terminal session definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSpec {
    pub id: String,
    pub name: String,
    pub kind: SessionKind,
    pub cwd: String,
    /// Account id; `None` uses the provider's default login (~/.claude, ~/.codex).
    #[serde(default)]
    pub account: Option<String>,
    /// Extra arguments for the agent CLI.
    #[serde(default)]
    pub args: Vec<String>,
    /// Full command override (program + args), mostly for custom tools and tests.
    #[serde(default)]
    pub command: Option<Vec<String>>,
    /// True once the session has run, so restarting continues the previous conversation.
    #[serde(default)]
    pub started_once: bool,
    /// Claude conversation id owned by this session (`--session-id` / `--resume`), so a
    /// restart never picks up another agent's conversation in the same folder.
    #[serde(default)]
    pub conversation: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Everything kimo persists in `~/.kimo/state.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub accounts: Vec<Account>,
    #[serde(default)]
    pub sessions: Vec<SessionSpec>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_fields_survive_round_trip() {
        let raw = r#"{"accounts":[{"id":"a","name":"Work","provider":"claude","color":"red"}],
            "sessions":[{"id":"s","name":"api","kind":"codex","cwd":"/tmp","pinned":true}],
            "version":3}"#;
        let state: State = serde_json::from_str(raw).unwrap();
        assert_eq!(state.accounts[0].extra["color"], "red");
        assert_eq!(state.sessions[0].extra["pinned"], true);
        assert!(!state.sessions[0].started_once);
        let back = serde_json::to_value(&state).unwrap();
        assert_eq!(back["version"], 3);
        assert_eq!(back["sessions"][0]["pinned"], true);
    }
}
