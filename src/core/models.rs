//! Plain data: accounts, sessions and the persisted state. Unknown JSON fields are preserved.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Which agent CLI an account or a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// Running when kimo restarted for an update: started again (continuing) once it is back.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub resume_on_start: bool,
    /// Set when kimo stopped this agent to free memory (cleared when it starts again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freed: Option<Freed>,
    /// This agent's model (else the agent settings' one, else the CLI's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// This agent's effort level (else the agent settings' one, else the CLI's default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
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
    /// Colour of the project's mark in the sidebar (`#rrggbb`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
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
    /// Effort level passed on start (Claude `--effort`, Codex reasoning effort); empty = the
    /// CLI's default.
    #[serde(default)]
    pub effort: String,
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
            effort: String::new(),
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub agents: AgentsSettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub quick_commands: Vec<QuickCommand>,
    /// Light or dark colours for the window and the web client.
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub memory: MemorySettings,
    /// The subscription new agents use, per provider (`None` = this computer's own login).
    #[serde(default)]
    pub active_accounts: ActiveAccounts,
    /// Read Claude's plan limits (5-hour and weekly use) from Claude's API. On macOS this asks
    /// once to read Claude's login from the Keychain, so it waits for a yes there.
    #[serde(default = "claude_limits_default")]
    pub claude_limits: bool,
    #[serde(default)]
    pub updates: UpdateSettings,
    /// Keep the computer from sleeping: off, while agents work, or always.
    #[serde(default)]
    pub keep_awake: crate::core::awake::KeepAwake,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// New kimo versions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateSettings {
    /// A system notification when a new version is out (once per version).
    #[serde(default = "yes")]
    pub notify: bool,
    /// Install new versions and restart by itself, when no agent is working.
    #[serde(default)]
    pub auto: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        serde_json::from_str("{}").expect("every update setting has a default")
    }
}

impl Default for Settings {
    /// The same defaults as a state file without settings.
    fn default() -> Self {
        serde_json::from_str("{}").expect("every settings field has a default")
    }
}

fn claude_limits_default() -> bool {
    !cfg!(target_os = "macos")
}

/// The subscription new agents start on, per provider.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ActiveAccounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<String>,
}

impl ActiveAccounts {
    pub fn get(&self, provider: Provider) -> Option<&String> {
        match provider {
            Provider::Claude => self.claude.as_ref(),
            Provider::Codex => self.codex.as_ref(),
        }
    }

    pub fn set(&mut self, provider: Provider, account: Option<String>) {
        match provider {
            Provider::Claude => self.claude = account,
            Provider::Codex => self.codex = account,
        }
    }
}

/// Why and when kimo stopped an agent to free memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Freed {
    /// Unix ms.
    pub at: u64,
    pub bytes: u64,
    /// "low memory", "idle" or "asked".
    pub reason: String,
}

/// What kimo does about memory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemorySettings {
    /// When memory is critical, stop idle Claude and Codex agents (biggest first); they
    /// continue their conversation when opened.
    #[serde(default = "yes")]
    pub free_when_low: bool,
    /// Stop agents idle for longer than this many minutes (0 = never).
    #[serde(default)]
    pub stop_idle_after_minutes: u32,
    /// Agents using more than this many MB are shown as heavy.
    #[serde(default = "default_heavy_mb")]
    pub heavy_mb: u32,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn default_heavy_mb() -> u32 {
    1500
}

impl Default for MemorySettings {
    fn default() -> Self {
        Self {
            free_when_low: true,
            stop_idle_after_minutes: 0,
            heavy_mb: default_heavy_mb(),
            extra: Map::new(),
        }
    }
}

/// Colour scheme: the system's, or always light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

/// Another kimo this one is paired with (both directions): its agents are reachable as `name/id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    /// Host prefix in agent ids (`gpu-lab/api-1a2b`).
    pub id: String,
    pub name: String,
    pub url: String,
    /// Device key this kimo uses on the peer.
    pub token: String,
    /// How the peer knows this kimo (the prefix of our agents over there).
    pub self_name: String,
    /// The device (key) the peer uses on this kimo: revoked when the peer is forgotten.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One start-to-stop of an agent session under one account (for usage per subscription).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub session: String,
    pub provider: Provider,
    /// `None` = the provider's default login.
    #[serde(default)]
    pub account: Option<String>,
    pub cwd: String,
    /// Unix milliseconds.
    pub started_at: u64,
    #[serde(default)]
    pub ended_at: Option<u64>,
    /// The agent's conversation in this run (Codex: once its id is known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    #[serde(default)]
    pub clients: Vec<Client>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub devices: Vec<Device>,
    #[serde(default)]
    pub settings: Settings,
    /// Recent agent runs, newest last (capped).
    #[serde(default)]
    pub runs: Vec<Run>,
    #[serde(default)]
    pub peers: Vec<Peer>,
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

    #[test]
    fn updates_notify_but_do_not_install_by_default() {
        let settings = Settings::default();
        assert!(settings.updates.notify);
        assert!(!settings.updates.auto);
        let older: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(older.updates, UpdateSettings::default());
    }
}
