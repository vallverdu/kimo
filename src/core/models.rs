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

/// What an app key may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Watch sessions, accounts and events; no terminals, no changes.
    Read,
    /// Also drive sessions: create, start/pause/stop, switch accounts, type, open terminals.
    Control,
}

/// Another PWA or tool allowed to use kimo, with its own revocable key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub name: String,
    pub token: String,
    /// Browser origins the key may be used from, e.g. `https://pm.example.ts.net`.
    #[serde(default)]
    pub origins: Vec<String>,
    pub scope: Scope,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A folder agents work in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A phone, tablet or computer paired with kimo (owner access, revocable).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub token: String,
    /// Unix time (ms) of pairing.
    pub paired_at: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// How much an agent may do without asking.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permissions {
    /// The agent's own approval prompts.
    #[default]
    Manual,
    /// Skip every prompt (`--dangerously-skip-permissions`, `--dangerously-bypass-approvals-and-sandbox`).
    Yolo,
}

/// Launch preferences for one agent CLI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPrefs {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub permissions: Permissions,
    /// Model passed on start (`--model` / `-m`); empty = the CLI's default.
    #[serde(default)]
    pub model: String,
    /// Extra arguments for every session of this agent.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn yes() -> bool {
    true
}

impl Default for AgentPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            permissions: Permissions::Manual,
            model: String::new(),
            args: vec![],
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AgentsSettings {
    #[serde(default)]
    pub claude: AgentPrefs,
    #[serde(default)]
    pub codex: AgentPrefs,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl AgentsSettings {
    pub fn get(&self, provider: Provider) -> &AgentPrefs {
        match provider {
            Provider::Claude => &self.claude,
            Provider::Codex => &self.codex,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerminalSettings {
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_font_size() -> f32 {
    13.0
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: default_font_size(),
            extra: Map::new(),
        }
    }
}

/// A saved prompt or command, sent to a terminal with one tap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuickCommand {
    pub name: String,
    pub text: String,
    /// Project id; `None` = every project.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// kimo's settings, edited from the app (and any owner client).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub agents: AgentsSettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub quick_commands: Vec<QuickCommand>,
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
    #[serde(default)]
    pub clients: Vec<Client>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub settings: Settings,
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
