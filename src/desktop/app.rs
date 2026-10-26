//! The kimo window: projects (drop a folder to add one), terminals, subscriptions, skills,
//! settings and devices. It talks to the kimo server only through the API, like any app.

use super::terminal::TerminalView;
use super::theme::{self, dot, primary, section};
use crate::client::Client;
use crate::core::models::{Permissions, QuickCommand, Settings, Theme};
use crate::core::update::is_newer;

#[path = "details.rs"]
mod details;
use anyhow::Result;
use eframe::egui;
use eframe::egui::{Align2, Color32, RichText, Ui};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Terminals,
    Usage,
    Subscriptions,
    Skills,
    Settings,
    Devices,
}

/// Results of background calls.
enum Msg {
    Projects(Value),
    Status(Value),
    Settings(Box<Settings>),
    Agents(Value),
    Skills(Value),
    Devices(Value),
    Network(Value),
    Apps(Value),
    Usage(Value),
    /// Daily usage report (`/usage/daily`).
    Daily(Value),
    /// Paired kimos with keys (from this machine's kimo): hosts the window connects to.
    Hosts(Value),
    /// Paired kimos with online state (Devices view).
    Peers(Value),
    Pairing(String, u64),
    /// Folder listing for the project dialog's picker.
    Folder(Value),
    /// Earlier conversations of a project: (project name, folder, list).
    History(String, String, Value),
    /// A project was added; open what it continued.
    Added(Value),
    Open(Value),
    Toast(String),
    /// A toast without refreshing the view (keeps forms being edited).
    Note(String),
    /// The latest release (`/update`).
    Release(Value),
    /// Listening ports (`/ports`) of the shown kimo.
    Ports(Value),
    /// Type text into a session's open terminal: (session id, text).
    Type(String, String),
    /// Files, git and agents for the right panel, and opened files.
    Details(details::Loaded),
    /// Memory report (`/memory`).
    Memory(Value),
    /// What a session's memory is made of: (session id, parts).
    Parts(String, Value),
    /// This machine's kimo server version (`/status`), to follow it after updates.
    LocalVersion(String),
    /// The kimo server restarted (after an update): its version now.
    ServerRestarted(String),
    /// An update step failed.
    UpdateFailed(String),
    Error(String),
}

/// Sessions and accounts, kept current by the event stream.
#[derive(Default)]
struct Live {
    sessions: Vec<Value>,
    accounts: Vec<Value>,
    /// The first update arrived (before it, an empty list means "not known yet").
    loaded: bool,
}

#[derive(Default)]
struct ProjectForm {
    mode: String,
    name: String,
    path: String,
    git: bool,
    url: String,
    /// Folder listing from the server for the picker.
    listing: Option<Value>,
    /// Name typed for a new folder in the picker.
    new_folder: String,
}

struct SkillForm {
    source: String,
    skill: String,
    claude: bool,
    codex: bool,
    shared: bool,
    project: String,
    replace: bool,
}

struct Pairing {
    /// The one-time code (the QR code is redrawn for another address without a new one).
    code: String,
    until: Instant,
}

/// The project settings dialog.
struct ProjectSettings {
    id: String,
    name: String,
    path: String,
    /// `#rrggbb` or empty.
    color: String,
    confirm_remove: bool,
}

/// The "new worktree" dialog.
struct WorktreeForm {
    project: String,
    project_name: String,
    name: String,
    /// `claude`, `codex`, `shell` or empty for none.
    agent: String,
}

/// An action that waits for a yes.
#[allow(clippy::enum_variant_names)]
enum Confirm {
    /// Close a file with unsaved changes.
    CloseFile {
        index: usize,
    },
    RemoveSession {
        id: String,
        name: String,
    },
    RemoveProject {
        id: String,
        name: String,
    },
    RemoveWorktree {
        project: String,
        path: String,
    },
    RemoveAccount {
        id: String,
        name: String,
    },
}

/// What a session's right-click menu asked for.
enum SessionAct {
    Open(String),
    Stop(String),
    Remove { id: String, name: String },
}

/// Making another subscription the active one: waits for a yes.
struct Switch {
    provider: String,
    /// Account id, `system` for this computer's own login.
    id: String,
    name: String,
    /// Agents of the provider on other subscriptions (moved too when `move_agents`).
    agents: usize,
    move_agents: bool,
}

/// Updating kimo: where it is.
#[derive(Clone, PartialEq)]
enum Updating {
    Idle,
    Busy(&'static str),
}

/// A git worktree of a project (the project's own folder first).
struct Worktree {
    path: String,
    branch: Option<String>,
    primary: bool,
}

/// Colours offered for projects (the sidebar mark).
const PROJECT_COLORS: [&str; 8] = [
    "#8a8a8a", "#e5484d", "#f08c28", "#e6b422", "#2f9e57", "#12939a", "#8a5cf6", "#e05297",
];

/// A kimo the window talks to: its client and its live sessions.
struct Host {
    id: String,
    name: String,
    client: Client,
    live: Arc<Mutex<Live>>,
}

impl Host {
    fn connect(id: &str, name: &str, client: Client, ctx: &egui::Context) -> Self {
        let live = Arc::new(Mutex::new(Live::default()));
        let (l2, ctx) = (live.clone(), ctx.clone());
        client.subscribe(move |update| {
            if let Ok(mut l) = l2.lock() {
                l.sessions = update["sessions"].as_array().cloned().unwrap_or_default();
                l.accounts = update["accounts"].as_array().cloned().unwrap_or_default();
                l.loaded = true;
            }
            ctx.request_repaint();
        });
        Self {
            id: id.into(),
            name: name.into(),
            client,
            live,
        }
    }

    fn sessions(&self) -> Vec<Value> {
        self.live
            .lock()
            .map(|l| l.sessions.clone())
            .unwrap_or_default()
    }

    fn loaded(&self) -> bool {
        self.live.lock().map(|l| l.loaded).unwrap_or(false)
    }
}

pub struct KimoApp {
    ctx: egui::Context,
    logo: egui::TextureHandle,
    /// This kimo first, then the paired ones; `current` is the one the views show.
    hosts: Vec<Host>,
    current: usize,
    last_hosts: Instant,
    /// Paired kimos as the local server reports them (Devices view).
    peer_info: Vec<Value>,
    peer_url: String,
    peer_code: String,
    /// The history dialog: project name, folder, conversations.
    history: Option<(String, String, Vec<Value>)>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    view: View,
    projects: Vec<Value>,
    status: Value,
    settings: Option<Settings>,
    agents: Vec<Value>,
    skills: Vec<Value>,
    devices: Vec<Value>,
    network: Value,
    apps: Vec<Value>,
    usage: Vec<Value>,
    daily: Value,
    /// Days the Usage view shows.
    daily_days: u32,
    terminals: Vec<TerminalView>,
    active: usize,
    toasts: Vec<(String, bool, Instant)>,
    project_form: Option<ProjectForm>,
    account_name: String,
    skill_form: SkillForm,
    pairing: Option<Pairing>,
    /// When a pairing code was last asked for (asked again after a failure, not every frame).
    pairing_asked: Option<Instant>,
    pair_address: String,
    message: String,
    last_status: Instant,
    last_projects: Instant,
    /// Colour scheme from this machine's settings.
    theme: Theme,
    /// Projects shown folded (just their header), kept in `prefs`.
    collapsed: std::collections::HashSet<String>,
    /// The projects panel is shown (hidden: the terminal takes the whole window), in `prefs`.
    sidebar_open: bool,
    /// Where the projects panel was drawn last frame (dropping folders there adds projects).
    sidebar_rect: Option<egui::Rect>,
    /// The project whose terminals the tabs show (`None`: sessions outside projects).
    project: Option<String>,
    /// The tab last shown per project, to come back to it.
    last_tab: std::collections::HashMap<Option<String>, String>,
    /// Subscriptions: the provider whose "add an account" field is open.
    adding_account: Option<String>,
    prefs: PathBuf,
    project_settings: Option<ProjectSettings>,
    worktree_form: Option<WorktreeForm>,
    confirm: Option<Confirm>,
    switch: Option<Switch>,
    /// The latest release as this machine's kimo reports it (`/update`).
    release: Value,
    /// Memory report: pressure, biggest agents, agents kimo stopped to free memory.
    memory: Value,
    /// The memory card was closed at this pressure ("low", "critical").
    memory_dismissed: Option<String>,
    /// Newest "stopped to free memory" already announced (unix ms).
    seen_freed: Option<u64>,
    /// Memory breakdown shown under the toolbar: (session id, parts).
    parts: Option<(String, Value)>,
    last_release: Instant,
    /// The update card that was closed (`update:0.1.7`, `restart:0.1.7`, `behind:0.1.1`).
    dismissed: Option<String>,
    updating: Updating,
    /// This machine's kimo server version, as it last answered.
    server_version: Option<String>,
    /// The app icon's style (Settings → Appearance), kept with the window's preferences.
    dark_icon: bool,
    /// The right panel: the shown project's files, changes and agents.
    details: details::Details,
    /// Files open in tabs, and the one shown (instead of a terminal).
    editors: Vec<details::Editor>,
    editor_active: Option<usize>,
    /// Listening ports of the shown kimo, for the bottom bar.
    ports: Vec<Value>,
    last_ports: Instant,
    last_usage: Instant,
    /// CPU of each project over the last minutes (bottom bar, resources).
    cpu_history: std::collections::HashMap<String, std::collections::VecDeque<f32>>,
    /// The update the Dock icon already bounced for.
    attention_for: Option<String>,
    /// The server version this window already tried to restart onto.
    followed: Option<String>,
}

/// What the update card offers.
#[derive(Clone, PartialEq)]
enum Offer {
    /// A newer release to download and install.
    Install,
    /// A newer kimo is on disk already: restarting the server runs it.
    Restart,
}

/// An update the card shows: its key (closing it hides that one), title, text and action.
struct Pending {
    key: String,
    title: &'static str,
    text: String,
    offer: Offer,
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn mb(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", bytes >> 20)
    }
}

fn home(path: &str) -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| path.starts_with(h.as_str()))
        .map_or(path.to_string(), |h| format!("~{}", &path[h.len()..]))
}

/// `~/…` path, shortened to its last folders when long.
fn short_path(path: &str, max: usize) -> String {
    let p = home(path);
    if p.chars().count() <= max {
        return p;
    }
    let mut parts: Vec<&str> = p.split('/').collect();
    let mut out = parts.pop().unwrap_or("").to_string();
    while let Some(last) = parts.last() {
        if last.chars().count() + out.chars().count() + 3 > max {
            break;
        }
        out = format!("{}/{out}", parts.pop().unwrap_or(""));
    }
    format!("…/{out}")
}

/// 1.2M, 340k, 900.
pub fn tokens(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.0}k", n as f64 / 1e3),
        _ => format!("{:.1}M", n as f64 / 1e6),
    }
}

/// "3 d 4 h", "2 h 10 min", "5 min" until a unix-seconds time.
pub fn until(unix_secs: u64, now_secs: u64) -> String {
    let left = unix_secs.saturating_sub(now_secs);
    let (d, h, m) = (left / 86_400, left % 86_400 / 3600, left % 3600 / 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} min")
    } else {
        format!("{m} min")
    }
}

/// One line describing an account's usage.
fn usage_line(u: &Value) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let limits: Vec<String> = u["limits"]["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let window = match l["window_minutes"].as_u64().unwrap_or(0) {
                10_080 => "weekly".to_string(),
                300 => "5-hour".to_string(),
                m => format!("{} h", m / 60),
            };
            format!(
                "{:.0}% of {window} limit, resets in {}",
                l["used_percent"].as_f64().unwrap_or(0.0),
                until(l["resets_at"].as_u64().unwrap_or(0), now)
            )
        })
        .collect();
    if !limits.is_empty() {
        return limits.join(" · ");
    }
    let (h5, d7) = (
        u["tokens_5h"].as_u64().unwrap_or(0),
        u["tokens_7d"].as_u64().unwrap_or(0),
    );
    if d7 == 0 {
        "no usage through kimo in 7 days".into()
    } else {
        format!("{} tokens in 5 h · {} in 7 days", tokens(h5), tokens(d7))
    }
}

/// The folder a dropped file or folder stands for (a dropped file means its folder).
pub fn drop_target(path: &Path) -> Option<PathBuf> {
    if path.is_dir() {
        Some(path.to_path_buf())
    } else {
        path.parent().filter(|p| p.is_dir()).map(Path::to_path_buf)
    }
}

impl KimoApp {
    /// `prefs`: the window's own preferences file (folded projects).
    pub fn new(ctx: egui::Context, client: Client, prefs: PathBuf) -> Self {
        let (tx, rx) = channel();
        let saved: Value = std::fs::read_to_string(&prefs)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let local = Host::connect("local", "this machine", client, &ctx);
        // The logo is one colour: kept white here and tinted with the scheme's ink.
        let white: Vec<u8> = include_bytes!("../../assets/icons/logo-64.rgba")
            .chunks(4)
            .flat_map(|p| [255, 255, 255, p[3]])
            .collect();
        let logo = ctx.load_texture(
            "kimo-logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], &white),
            egui::TextureOptions::LINEAR,
        );
        let app = Self {
            hosts: vec![local],
            current: 0,
            last_hosts: Instant::now() - Duration::from_secs(60),
            peer_info: vec![],
            peer_url: String::new(),
            peer_code: String::new(),
            history: None,
            logo,
            ctx,
            tx,
            rx,
            view: View::Terminals,
            projects: vec![],
            status: Value::Null,
            settings: None,
            agents: vec![],
            skills: vec![],
            devices: vec![],
            network: Value::Null,
            apps: vec![],
            usage: vec![],
            daily: Value::Null,
            daily_days: 14,
            terminals: vec![],
            active: 0,
            toasts: vec![],
            project_form: None,
            account_name: String::new(),
            skill_form: SkillForm {
                source: String::new(),
                skill: String::new(),
                claude: true,
                codex: false,
                shared: false,
                project: String::new(),
                replace: false,
            },
            pairing: None,
            pairing_asked: None,
            pair_address: String::new(),
            message: String::new(),
            last_status: Instant::now() - Duration::from_secs(60),
            last_projects: Instant::now() - Duration::from_secs(60),
            theme: Theme::System,
            collapsed: serde_json::from_value(saved["collapsed"].clone()).unwrap_or_default(),
            sidebar_open: saved["sidebar"].as_bool().unwrap_or(true),
            dark_icon: saved["icon"] == "dark",
            details: details::Details {
                open: saved["details"].as_bool().unwrap_or(false),
                ..Default::default()
            },
            editors: Vec::new(),
            editor_active: None,
            sidebar_rect: None,
            project: None,
            last_tab: Default::default(),
            adding_account: None,
            prefs,
            project_settings: None,
            worktree_form: None,
            confirm: None,
            switch: None,
            release: Value::Null,
            memory: Value::Null,
            memory_dismissed: None,
            seen_freed: None,
            parts: None,
            // The first check a few seconds after start, then every 6 hours.
            last_release: Instant::now() - Duration::from_secs(600 - 5),
            dismissed: None,
            updating: Updating::Idle,
            server_version: None,
            ports: Vec::new(),
            last_ports: Instant::now() - Duration::from_secs(60),
            last_usage: Instant::now() - Duration::from_secs(600),
            cpu_history: Default::default(),
            attention_for: None,
            followed: None,
        };
        app.job(|c| Ok(Msg::Settings(Box::new(c.get("/settings")?))));
        app
    }

    /// The window's own preferences: folded projects, the projects panel.
    fn save_prefs(&self) {
        let prefs = json!({
            "collapsed": self.collapsed,
            "sidebar": self.sidebar_open,
            "icon": if self.dark_icon { "dark" } else { "light" },
            "details": self.details.open,
        });
        if let Err(e) = std::fs::write(&self.prefs, prefs.to_string()) {
            tracing::warn!("cannot save {}: {e}", self.prefs.display());
        }
    }

    fn close_editor(&mut self, j: usize) {
        if j >= self.editors.len() {
            return;
        }
        self.editors.remove(j);
        self.editor_active = match self.editor_active {
            Some(a) if a == j => None,
            Some(a) if a > j => Some(a - 1),
            other => other,
        };
    }

    fn toggle_details(&mut self) {
        self.details.open = !self.details.open;
        self.details.scope = None;
        self.save_prefs();
    }

    fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        self.save_prefs();
    }

    /// Run an API call off the UI thread; its result comes back as a message.
    fn job(&self, f: impl FnOnce(&Client) -> Result<Msg> + Send + 'static) {
        let (client, tx, ctx) = (self.client().clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let msg = f(&client).unwrap_or_else(|e| Msg::Error(e.to_string()));
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// An action followed by a refresh of what it may have changed.
    fn action(
        &self,
        done: &'static str,
        f: impl FnOnce(&Client) -> Result<Value> + Send + 'static,
    ) {
        self.job(move |c| {
            f(c)?;
            Ok(Msg::Toast(done.into()))
        });
    }

    fn toast(&mut self, text: impl Into<String>, error: bool) {
        self.toasts.push((text.into(), error, Instant::now()));
    }

    fn client(&self) -> &Client {
        &self.hosts[self.current].client
    }

    fn host(&self) -> &Host {
        &self.hosts[self.current]
    }

    fn sessions(&self) -> Vec<Value> {
        self.host().sessions()
    }

    fn accounts(&self) -> Vec<Value> {
        self.host()
            .live
            .lock()
            .map(|l| l.accounts.clone())
            .unwrap_or_default()
    }

    /// Show another kimo: its projects, sessions, subscriptions, skills and settings.
    fn switch_host(&mut self, index: usize) {
        if index == self.current || index >= self.hosts.len() {
            return;
        }
        self.current = index;
        self.projects.clear();
        self.project = None;
        self.pairing = None;
        self.pair_address.clear();
        self.network = Value::Null;
        self.skills.clear();
        self.usage.clear();
        self.ports.clear();
        self.cpu_history.clear();
        self.last_ports = Instant::now() - Duration::from_secs(60);
        self.last_usage = Instant::now() - Duration::from_secs(600);
        self.settings = None;
        self.job(|c| Ok(Msg::Settings(Box::new(c.get("/settings")?))));
        self.refresh_view();
    }

    /// Run a call on this machine's kimo whatever host is shown (pairing kimos).
    fn local_job(&self, f: impl FnOnce(&Client) -> Result<Msg> + Send + 'static) {
        let (client, tx, ctx) = (
            self.hosts[0].client.clone(),
            self.tx.clone(),
            self.ctx.clone(),
        );
        std::thread::spawn(move || {
            let msg = f(&client).unwrap_or_else(|e| Msg::Error(e.to_string()));
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    fn open_session(&mut self, id: &str) {
        self.view = View::Terminals;
        self.editor_active = None;
        // Agents kimo stopped to free memory continue as soon as they are opened.
        if self
            .sessions()
            .iter()
            .any(|x| s(x, "id") == id && x["freed"].is_object() && s(x, "status") == "stopped")
        {
            let id = id.to_string();
            self.action("Continuing", move |c| {
                c.post(
                    &format!("/sessions/{id}/action"),
                    json!({ "action": "start" }),
                )
            });
        }
        // The tabs follow the project the session belongs to (a brand-new session, not listed
        // yet, stays in the project shown).
        let sessions = self.sessions();
        if let Some(x) = sessions.iter().find(|x| s(x, "id") == id) {
            if !is_login_session(x) {
                self.project = self.project_of(x);
            }
        }
        self.last_tab.insert(self.project.clone(), id.to_string());
        let host = self.host().id.clone();
        if let Some(i) = self
            .terminals
            .iter()
            .position(|t| t.session_id == id && t.host == host)
        {
            self.active = i;
            return;
        }
        self.terminals
            .push(TerminalView::open(self.client(), &host, id, &self.ctx));
        self.active = self.terminals.len() - 1;
    }

    /// The worktree folder a session works in: the longest project or worktree path its
    /// folder is in, with the project's id.
    fn home_of(&self, session: &Value) -> Option<(String, String)> {
        let cwd = s(session, "cwd");
        self.projects
            .iter()
            .flat_map(|p| {
                worktrees_of(p)
                    .into_iter()
                    .map(move |w| (s(p, "id").to_string(), w.path))
            })
            .filter(|(_, f)| cwd == f.as_str() || cwd.starts_with(&format!("{f}/")))
            .max_by_key(|(_, f)| f.len())
    }

    fn project_of(&self, session: &Value) -> Option<String> {
        self.home_of(session).map(|(p, _)| p)
    }

    /// Start an agent or a terminal in a folder; it opens when the server answers.
    fn start_agent(&self, label: &str, cwd: &str, kind: &str) {
        let taken: Vec<String> = self
            .sessions()
            .iter()
            .map(|x| s(x, "name").to_string())
            .collect();
        let what = if kind == "shell" { "terminal" } else { kind };
        let base = if label.is_empty() {
            what.to_string()
        } else {
            format!("{label} {what}")
        };
        let mut name = base.clone();
        let mut n = 2;
        while taken.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        let (cwd, kind) = (cwd.to_string(), kind.to_string());
        self.job(move |c| {
            Ok(Msg::Open(c.post(
                "/sessions",
                json!({ "name": name, "kind": kind, "cwd": cwd }),
            )?))
        });
    }

    fn handle_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Projects(v) => self.projects = v.as_array().cloned().unwrap_or_default(),
                Msg::Status(v) => self.status = v,
                Msg::Settings(v) => {
                    if self.current == 0 {
                        self.theme = v.theme;
                    }
                    self.settings = Some(*v);
                }
                Msg::Agents(v) => self.agents = v.as_array().cloned().unwrap_or_default(),
                Msg::Skills(v) => self.skills = v.as_array().cloned().unwrap_or_default(),
                Msg::Devices(v) => self.devices = v.as_array().cloned().unwrap_or_default(),
                Msg::Network(v) => {
                    if self.pair_address.is_empty() {
                        if let Some(a) = v["addresses"]
                            .as_array()
                            .and_then(|a| a.iter().find(|a| a["kind"] != "localhost"))
                        {
                            self.pair_address = s(a, "url").to_string();
                        }
                    }
                    self.network = v;
                }
                Msg::Apps(v) => self.apps = v.as_array().cloned().unwrap_or_default(),
                Msg::Usage(v) => self.usage = v.as_array().cloned().unwrap_or_default(),
                Msg::Daily(v) => self.daily = v,
                Msg::Peers(v) => self.peer_info = v.as_array().cloned().unwrap_or_default(),
                Msg::Hosts(v) => {
                    let peers = v.as_array().cloned().unwrap_or_default();
                    let current = self.host().id.clone();
                    // Forget removed peers, connect new ones.
                    self.hosts
                        .retain(|h| h.id == "local" || peers.iter().any(|p| s(p, "id") == h.id));
                    for p in &peers {
                        if !self.hosts.iter().any(|h| h.id == s(p, "id")) {
                            let client =
                                Client::new(s(p, "url"), p["token"].as_str().map(String::from));
                            self.hosts.push(Host::connect(
                                s(p, "id"),
                                s(p, "name"),
                                client,
                                &self.ctx,
                            ));
                        }
                    }
                    self.current = self.hosts.iter().position(|h| h.id == current).unwrap_or(0);
                }
                Msg::History(name, folder, list) => {
                    self.history =
                        Some((name, folder, list.as_array().cloned().unwrap_or_default()));
                }
                Msg::Added(made) => {
                    let continued = made["continued"].as_array().cloned().unwrap_or_default();
                    let name = s(&made["project"], "name").to_string();
                    let reopened = made["reopened"] == true;
                    self.toast(
                        match continued.len() {
                            0 if reopened => format!("{name} has no conversations to continue yet"),
                            _ if reopened => format!("Opened {name}"),
                            0 => format!("Project added: {name}"),
                            1 => {
                                format!("Project added: {name} · continuing its last conversation")
                            }
                            n => format!(
                                "Project added: {name} · continuing its last {n} conversations"
                            ),
                        },
                        false,
                    );
                    self.last_projects = Instant::now() - Duration::from_secs(60);
                    // Open the conversation that was resumed (or the clone terminal).
                    let open = made["session"]
                        .as_object()
                        .map(|_| made["session"].clone())
                        .or_else(|| continued.first().cloned());
                    if let Some(session) = open {
                        let id = s(&session, "id").to_string();
                        self.open_session(&id);
                    }
                }
                Msg::Folder(listing) => {
                    if let Some(form) = self.project_form.as_mut() {
                        form.listing = Some(listing);
                    }
                }
                Msg::Pairing(code, secs) => {
                    self.pairing_asked = None;
                    self.pairing = Some(Pairing {
                        code,
                        until: Instant::now() + Duration::from_secs(secs),
                    })
                }
                Msg::Open(session) => {
                    let id = s(&session, "id").to_string();
                    self.open_session(&id);
                    self.last_projects = Instant::now() - Duration::from_secs(60);
                }
                Msg::Toast(t) => {
                    self.toast(t, false);
                    self.refresh_view();
                }
                Msg::Note(t) => self.toast(t, false),
                Msg::Release(v) => {
                    self.release = v;
                    self.ask_attention();
                }
                Msg::LocalVersion(v) => self.follow_server(v),
                Msg::Ports(v) => self.ports = v.as_array().cloned().unwrap_or_default(),
                Msg::Details(loaded) => self.on_loaded(loaded),
                Msg::Type(id, text) => {
                    if let Some(t) = self.terminals.iter().find(|t| t.session_id == id) {
                        t.send(&text);
                    }
                }
                Msg::ServerRestarted(v) => {
                    self.updating = Updating::Idle;
                    self.toast(format!("kimo {v} is running"), false);
                    self.local_job(|c| Ok(Msg::Release(c.get("/update")?)));
                    self.follow_server(v);
                }
                Msg::Memory(v) => {
                    // Announce agents kimo stopped to free memory since the last report.
                    let freed = v["freed"].as_array().cloned().unwrap_or_default();
                    let newest = freed
                        .iter()
                        .filter_map(|f| f["at"].as_u64())
                        .max()
                        .unwrap_or(0);
                    if let Some(seen) = self.seen_freed {
                        // Stops you asked for are announced by their own toast.
                        for f in freed.iter().filter(|f| {
                            f["at"].as_u64().unwrap_or(0) > seen && s(f, "reason") != "asked"
                        }) {
                            let why = if s(f, "reason") == "idle" {
                                "idle for a while"
                            } else {
                                "memory was low"
                            };
                            self.toasts.push((
                                format!(
                                    "Stopped {} to free {} ({why}). It continues when you open it.",
                                    s(f, "name"),
                                    mb(f["bytes"].as_u64().unwrap_or(0))
                                ),
                                false,
                                Instant::now(),
                            ));
                        }
                    }
                    self.seen_freed = Some(newest.max(self.seen_freed.unwrap_or(0)));
                    if v["pressure"] == "ok" {
                        self.memory_dismissed = None;
                    }
                    self.memory = v;
                }
                Msg::Parts(id, v) => self.parts = Some((id, v)),
                Msg::UpdateFailed(e) => {
                    self.updating = Updating::Idle;
                    self.toast(e, true);
                }
                Msg::Error(e) => self.toast(e, true),
            }
        }
    }

    /// Fetch what the current view shows.
    fn refresh_view(&mut self) {
        self.last_projects = Instant::now() - Duration::from_secs(60);
        match self.view {
            View::Terminals => {}
            View::Usage => {
                let days = self.daily_days;
                self.job(move |c| Ok(Msg::Daily(c.get(&format!("/usage/daily?days={days}"))?)));
                self.job(|c| Ok(Msg::Usage(c.get("/usage")?)));
            }
            View::Subscriptions => {
                self.job(|c| Ok(Msg::Usage(c.get("/usage")?)));
                self.job(|c| Ok(Msg::Settings(Box::new(c.get("/settings")?))));
            }
            View::Skills => self.job(|c| Ok(Msg::Skills(c.get("/skills")?))),
            View::Settings => {
                self.job(|c| Ok(Msg::Settings(Box::new(c.get("/settings")?))));
                self.job(|c| Ok(Msg::Agents(c.get("/agents")?)));
            }
            View::Devices => {
                self.local_job(|c| Ok(Msg::Peers(c.get("/peers")?)));
                self.job(|c| Ok(Msg::Devices(c.get("/devices")?)));
                self.job(|c| Ok(Msg::Network(c.get("/network")?)));
                self.job(|c| Ok(Msg::Apps(c.get("/clients")?)));
            }
        }
    }

    fn poll(&mut self) {
        if self.last_status.elapsed() > Duration::from_secs(3) {
            self.last_status = Instant::now();
            self.job(|c| Ok(Msg::Status(c.get("/status")?)));
            self.job(|c| Ok(Msg::Memory(c.get("/memory")?)));
            self.record_cpu();
        }
        self.details_poll();
        // The bottom bar: ports and plan limits (kimos before 0.1.8 have no /ports).
        if self.last_ports.elapsed() > Duration::from_secs(10) {
            self.last_ports = Instant::now();
            self.job(|c| Ok(Msg::Ports(c.get("/ports").unwrap_or(Value::Null))));
        }
        if self.last_usage.elapsed() > Duration::from_secs(120) {
            self.last_usage = Instant::now();
            self.job(|c| Ok(Msg::Usage(c.get("/usage").unwrap_or(Value::Null))));
        }
        if self.last_hosts.elapsed() > Duration::from_secs(15) {
            self.last_hosts = Instant::now();
            self.local_job(|c| Ok(Msg::Hosts(c.get("/peers/keys")?)));
            if self.updating == Updating::Idle {
                self.local_job(|c| {
                    let st: Value = c.get("/status")?;
                    Ok(Msg::LocalVersion(s(&st, "version").to_string()))
                });
            }
        }
        if self.last_projects.elapsed() > Duration::from_secs(10) {
            self.last_projects = Instant::now();
            self.job(|c| Ok(Msg::Projects(c.get("/projects")?)));
        }
        // The server asks GitHub every hour; this only reads its answer.
        if self.last_release.elapsed() > Duration::from_secs(600) {
            self.last_release = Instant::now();
            // Servers older than 0.1.2 have no /update: the version check covers them.
            self.local_job(|c| Ok(Msg::Release(c.get("/update").unwrap_or(Value::Null))));
        }
        // Wake up only for the next status poll; live data (events, terminal output) repaints by itself.
        self.ctx.request_repaint_after(Duration::from_secs(3));
    }

    /// Folders dropped on the projects panel become projects; anything dropped elsewhere
    /// while a terminal is open is typed into it as paths (Claude Code and Codex read images
    /// and files given by path). With no terminal open, a drop anywhere adds a project.
    fn handle_drops(&mut self, ui: &Ui) {
        let ctx = ui.ctx().clone();
        let (dropped, hovering) =
            ui.input(|i| (i.raw.dropped_files.clone(), !i.raw.hovered_files.is_empty()));
        if dropped.is_empty() && !hovering {
            return;
        }
        let full = ctx.content_rect();
        let typing = self.view == View::Terminals && !self.terminals.is_empty();
        let sidebar = self.sidebar_rect;
        let over_sidebar = sidebar
            .zip(drop_pointer(&ctx))
            .is_some_and(|(r, p)| r.contains(p));
        let to_terminal = typing && !over_sidebar;
        if !dropped.is_empty() {
            if to_terminal {
                let paths: Vec<String> = dropped
                    .iter()
                    .map(|f| shell_quote(&f.path().display().to_string()))
                    .filter(|p| !p.is_empty())
                    .collect();
                if !paths.is_empty() {
                    self.terminals[self.active].send(&format!("{} ", paths.join(" ")));
                }
            } else {
                for file in dropped {
                    let Some(dir) = drop_target(file.path()) else {
                        continue;
                    };
                    let path = dir.display().to_string();
                    self.job(move |c| {
                        Ok(Msg::Added(c.post(
                            "/projects",
                            json!({ "mode": "existing", "path": path }),
                        )?))
                    });
                }
            }
        }
        if hovering {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop"),
            ));
            let zone = |rect: egui::Rect, text: &str, active: bool| {
                painter.rect_filled(
                    rect,
                    0.0,
                    theme::background().gamma_multiply(if active { 0.94 } else { 0.8 }),
                );
                painter.rect_stroke(
                    rect.shrink(12.0),
                    8.0,
                    egui::Stroke::new(
                        if active { 2.0 } else { 1.0 },
                        if active { theme::ink() } else { theme::muted() },
                    ),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(if active { 20.0 } else { 16.0 }),
                    if active { theme::ink() } else { theme::muted() },
                );
            };
            match (typing, sidebar) {
                (true, Some(side)) => {
                    let rest =
                        egui::Rect::from_min_max(egui::pos2(side.right(), full.min.y), full.max);
                    zone(side, "Drop a folder:\nadd it as a project", over_sidebar);
                    zone(
                        rest,
                        "Drop files here:\ntype their paths in the terminal",
                        !over_sidebar,
                    );
                }
                (true, None) => zone(full, "Drop files:\ntype their paths in the terminal", true),
                (false, _) => zone(full, "Drop a folder to add it as a project", true),
            }
            // No pointer events arrive during a drag: look again soon to follow it.
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    // ----- layout -----

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let shortcut = if cfg!(target_os = "macos") {
                "⌘B"
            } else {
                "Ctrl+B"
            };

            ui.add(
                egui::Image::new(egui::load::SizedTexture::new(self.logo.id(), [30.0, 30.0]))
                    .tint(theme::ink()),
            );
            if ui
                .add(
                    egui::Button::new(RichText::new("kimo").size(20.0).color(theme::ink()))
                        .frame(false),
                )
                .on_hover_text("the terminals")
                .clicked()
                && self.view != View::Terminals
            {
                self.view = View::Terminals;
                self.refresh_view();
            }
            ui.label(
                RichText::new("AGENT ENVIRONMENT")
                    .size(10.0)
                    .color(theme::muted()),
            );
            // Always here, so it opens the panel from where it closed it.
            ui.add_space(6.0);
            if panel_button(ui, self.sidebar_open)
                .on_hover_text(format!(
                    "{} the projects panel ({shortcut})",
                    if self.sidebar_open { "hide" } else { "show" }
                ))
                .clicked()
            {
                self.toggle_sidebar();
            }
            ui.add_space(16.0);
            ui.add_space(16.0);
            // The views as one row of buttons; the open one filled. The terminals are the
            // projects' own place (open a project or an agent); clicking the open view's
            // button again, or the logo, goes back to them.
            ui.spacing_mut().item_spacing.x = 4.0;
            for (view, label) in [
                (View::Usage, "Usage"),
                (View::Subscriptions, "Subscriptions"),
                (View::Skills, "Skills"),
                (View::Settings, "Settings"),
                (View::Devices, "Devices"),
            ] {
                let selected = self.view == view;
                let text = RichText::new(label).size(13.0).color(if selected {
                    theme::background()
                } else {
                    theme::ink()
                });
                let button = egui::Button::new(text)
                    .fill(if selected {
                        theme::ink()
                    } else {
                        theme::surface()
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if selected {
                            theme::ink()
                        } else {
                            theme::line()
                        },
                    ))
                    .corner_radius(6)
                    .min_size(egui::vec2(0.0, 28.0));
                let r = ui.add(button);
                if selected {
                    r.clone().on_hover_text("back to the terminals");
                }
                if r.clicked() {
                    self.view = if selected { View::Terminals } else { view };
                    self.refresh_view();
                }
            }
            ui.spacing_mut().item_spacing.x = 8.0;
            // An update stays one click away after its card was closed.
            if let Some(p) = self.pending_update() {
                if self.dismissed.as_deref() == Some(p.key.as_str()) {
                    ui.add_space(8.0);
                    let label = match p.offer {
                        Offer::Install => "↑ Update",
                        Offer::Restart => "↻ Restart to update",
                    };
                    let button = egui::Button::new(
                        RichText::new(label).size(12.5).color(theme::background()),
                    )
                    .fill(theme::teal())
                    .stroke(egui::Stroke::new(1.0, theme::teal()))
                    .corner_radius(6)
                    .min_size(egui::vec2(0.0, 28.0));
                    if ui.add(button).on_hover_text(&p.text).clicked() {
                        self.dismissed = None;
                    }
                }
            }
            let waiting: Vec<Value> = self
                .sessions()
                .into_iter()
                .filter(|x| s(x, "activity") == "waiting")
                .collect();
            let mut details = false;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let shortcut = if cfg!(target_os = "macos") {
                    "⌘⇧E"
                } else {
                    "Ctrl+Shift+E"
                };
                details = details_button(ui, self.details.open)
                    .on_hover_text(format!(
                        "{} files, changes and agents of the project ({shortcut})",
                        if self.details.open { "hide" } else { "show" }
                    ))
                    .clicked();
            });
            if details {
                self.toggle_details();
            }
            if !waiting.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let text = format!(
                        "● {} need{} you",
                        waiting.len(),
                        if waiting.len() == 1 { "s" } else { "" }
                    );
                    let button = egui::Button::new(RichText::new(text).color(theme::background()))
                        .fill(theme::accent())
                        .stroke(egui::Stroke::new(1.0, theme::accent()));
                    let names: Vec<&str> = waiting.iter().map(|x| s(x, "name")).collect();
                    if ui.add(button).on_hover_text(names.join("\n")).clicked() {
                        let id = s(&waiting[0], "id").to_string();
                        self.open_session(&id);
                    }
                });
            }
        });
    }

    /// Each project's CPU now, kept for the resources sparklines (a few minutes).
    fn record_cpu(&mut self) {
        let mut now: std::collections::HashMap<String, f32> = Default::default();
        for x in self.sessions() {
            let name = self.project_name_of(&x);
            *now.entry(name).or_default() +=
                x["usage"]["cpu_percent"].as_f64().unwrap_or(0.0) as f32;
        }
        self.cpu_history.retain(|k, _| now.contains_key(k));
        for (name, cpu) in now {
            let h = self.cpu_history.entry(name).or_default();
            h.push_back(cpu);
            while h.len() > 60 {
                h.pop_front();
            }
        }
    }

    /// The project a session works in, by name ("Not in a project" otherwise).
    fn project_name_of(&self, session: &Value) -> String {
        self.project_of(session)
            .and_then(|id| self.projects.iter().find(|p| s(p, "id") == id))
            .map(|p| s(p, "name").to_string())
            .unwrap_or_else(|| "Not in a project".into())
    }

    /// The bottom bar: plan use of the active subscriptions on the left; keeping the
    /// computer awake, resources, ports and the kimos on the right. Each opens a panel.
    fn status_bar(&mut self, ui: &mut Ui) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let accounts = self.accounts();
        let mut go_view = None;
        let mut open = None;
        let mut host = None;
        let mut awake_to = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Plan use per provider: its active subscription's limit closest to running out.
            let mut chips = Vec::new();
            for provider in ["claude", "codex"] {
                let active = accounts
                    .iter()
                    .find(|a| s(a, "provider") == provider && a["active"] == true)
                    .or_else(|| accounts.iter().find(|a| s(a, "provider") == provider && a["system"] == true));
                let text = active.and_then(|a| usage_of(&self.usage, a)).map(|u| limits_short(u, now));
                if let Some(text) = text.filter(|t| !t.is_empty()) {
                    let label = if provider == "claude" { "Claude" } else { "Codex" };
                    chips.push(format!("{label} {text}"));
                }
            }
            let text = if chips.is_empty() { "Used".to_string() } else { format!("Used   {}", chips.join("   ")) };
            let r = bar_button(ui, &text).on_hover_text("plan use of the active subscriptions");
            egui::Popup::menu(&r).align(egui::RectAlign::TOP_START).width(320.0).show(|ui| {
                ui.label(RichText::new("Usage").strong().color(theme::ink()));
                for provider in ["claude", "codex"] {
                    let list: Vec<&Value> = accounts.iter().filter(|a| s(a, "provider") == provider).collect();
                    if list.is_empty() {
                        continue;
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new(if provider == "claude" { "CLAUDE" } else { "CODEX" }).size(10.5).color(theme::muted()));
                    for a in list {
                        ui.horizontal(|ui| {
                            ui.add(egui::Label::new(RichText::new(s(a, "name")).color(theme::ink())).truncate());
                            if a["active"] == true {
                                ui.label(RichText::new("active").size(10.5).color(theme::teal()));
                            }
                        });
                        // Each limit on its own line under the name: use, reset.
                        let u = usage_of(&self.usage, a);
                        let limits = u.and_then(|u| u["limits"]["limits"].as_array()).cloned().unwrap_or_default();
                        if limits.is_empty() {
                            let why = u.and_then(|u| u["limits_note"].as_str()).unwrap_or("no limits known yet");
                            ui.label(RichText::new(why).size(11.0).color(theme::muted()));
                        }
                        for l in &limits {
                            let used = l["used_percent"].as_f64().unwrap_or(0.0);
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 5.0), egui::Sense::hover());
                                ui.painter().rect_filled(rect, 2.0, theme::line());
                                let mut done = rect;
                                done.set_width(rect.width() * (used / 100.0).clamp(0.0, 1.0) as f32);
                                ui.painter().rect_filled(done, 2.0, if used >= 90.0 { theme::bad() } else if used >= 70.0 { theme::warn() } else { theme::teal() });
                                ui.label(RichText::new(format!(
                                    "{} {used:.0}% · resets in {}",
                                    s(l, "name"),
                                    until(l["resets_at"].as_u64().unwrap_or(0), now)
                                )).size(11.5).color(theme::text()));
                            });
                        }
                        ui.add_space(2.0);
                    }
                }
                ui.separator();
                if ui.button("Usage details and history").clicked() {
                    go_view = Some(View::Usage);
                }
                if ui.button("Manage subscriptions…").clicked() {
                    go_view = Some(View::Subscriptions);
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // The kimos: this machine and the paired ones.
                let r = bar_button(ui, &format!("{} kimo{}", self.hosts.len(), if self.hosts.len() == 1 { "" } else { "s" }))
                    .on_hover_text("the kimos this window shows");
                egui::Popup::menu(&r).align(egui::RectAlign::TOP_END).width(280.0).show(|ui| {
                    ui.label(RichText::new("KIMOS").size(10.5).color(theme::muted()));
                    for (i, h) in self.hosts.iter().enumerate() {
                        let online = h.loaded();
                        let shown = if i == self.current { "  · shown" } else { "" };
                        let what = if i == 0 { "this machine" } else { "paired kimo" };
                        let text = format!("{} {}\n{what} · {}{shown}", if online { "●" } else { "○" }, h.name, if online { "online" } else { "offline" });
                        if ui.button(RichText::new(text).color(if online { theme::ink() } else { theme::muted() })).clicked() {
                            host = Some(i);
                        }
                    }
                    ui.separator();
                    if ui.button("Pair another kimo…").clicked() {
                        host = Some(0);
                        go_view = Some(View::Devices);
                    }
                });

                // Listening ports: those of agents and terminals first, the others folded.
                let mine: Vec<&Value> = self.ports.iter().filter(|p| !p["session"].is_null()).collect();
                let others: Vec<&Value> = self.ports.iter().filter(|p| p["session"].is_null()).collect();
                let r = bar_button(ui, &format!("{} port{}", mine.len(), if mine.len() == 1 { "" } else { "s" }))
                    .on_hover_text(format!("{} from agents and terminals, {} others", mine.len(), others.len()));
                egui::Popup::menu(&r)
                    .align(egui::RectAlign::TOP_END)
                    .width(340.0)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Ports").strong().color(theme::ink()));
                            ui.label(RichText::new(format!("{} from agents · {} others", mine.len(), others.len())).size(11.0).color(theme::muted()));
                        });
                        egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                            let mut names: Vec<&str> = mine.iter().map(|p| s(p, "session_name")).collect();
                            names.dedup();
                            for name in names {
                                ui.add_space(4.0);
                                ui.label(RichText::new(name).color(theme::ink()));
                                for p in mine.iter().filter(|p| s(p, "session_name") == name) {
                                    if port_row(ui, p) && self.current == 0 {
                                        open = Some(format!("http://localhost:{}", p["port"]));
                                    }
                                }
                            }
                            if mine.is_empty() {
                                ui.label(RichText::new("No agent or terminal listens on a port.").size(11.5).color(theme::muted()));
                            }
                            ui.add_space(4.0);
                            egui::CollapsingHeader::new(RichText::new(format!("OTHER PORTS  {}", others.len())).size(10.5).color(theme::muted()))
                                .id_salt("other-ports")
                                .show(ui, |ui| {
                                    for p in &others {
                                        port_row(ui, p);
                                    }
                                });
                        });
                    });

                // Resources: CPU and memory per project, with a minute-long CPU line.
                let sessions = self.sessions();
                let running = sessions.iter().filter(|x| matches!(s(x, "status"), "running" | "paused")).count();
                let st = &self.status;
                let agents = st["agents_rss_bytes"].as_u64().unwrap_or(0);
                let free = RichText::new(format!("{} · {running} running", mb(agents))).size(11.5);
                let free = match s(st, "memory_pressure") {
                    "critical" => free.color(theme::bad()),
                    "low" => free.color(theme::warn()),
                    _ => free.color(theme::text()),
                };
                let r = ui
                    .add(egui::Button::new(free).frame(false))
                    .on_hover_text("memory and CPU per project");
                egui::Popup::menu(&r).align(egui::RectAlign::TOP_END).width(380.0).show(|ui| {
                    let cpu: f64 = sessions.iter().map(|x| x["usage"]["cpu_percent"].as_f64().unwrap_or(0.0)).sum();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Resources").strong().color(theme::ink()));
                        ui.label(RichText::new(format!(
                            "{cpu:.1}% CPU · agents {} · free {} of {} · kimo {}",
                            mb(agents),
                            mb(st["mem_available_bytes"].as_u64().unwrap_or(0)),
                            mb(st["mem_total_bytes"].as_u64().unwrap_or(0)),
                            mb(st["kimo_rss_bytes"].as_u64().unwrap_or(0))
                        )).size(11.0).color(theme::muted()));
                    });
                    let mut groups: Vec<(String, Vec<&Value>)> = Vec::new();
                    for x in &sessions {
                        let name = self.project_name_of(x);
                        match groups.iter_mut().find(|(n, _)| *n == name) {
                            Some((_, list)) => list.push(x),
                            None => groups.push((name, vec![x])),
                        }
                    }
                    egui::ScrollArea::vertical().max_height(440.0).show(ui, |ui| {
                        egui::Grid::new("resources").num_columns(4).spacing([10.0, 4.0]).show(ui, |ui| {
                            for (name, list) in &groups {
                                let gcpu: f64 = list.iter().map(|x| x["usage"]["cpu_percent"].as_f64().unwrap_or(0.0)).sum();
                                let grss: u64 = list.iter().map(|x| x["usage"]["rss_bytes"].as_u64().unwrap_or(0)).sum();
                                ui.label(RichText::new(name.to_uppercase()).size(11.0).strong().color(theme::ink()));
                                sparkline(ui, self.cpu_history.get(name));
                                ui.label(RichText::new(format!("{gcpu:.1}%")).size(11.5));
                                ui.label(RichText::new(mb(grss)).size(11.5));
                                ui.end_row();
                                for x in list {
                                    let color = match s(x, "status") {
                                        "running" => theme::good(),
                                        "paused" => theme::warn(),
                                        _ => theme::muted(),
                                    };
                                    ui.horizontal(|ui| {
                                        ui.add_space(10.0);
                                        dot(ui, color);
                                        if ui.add(egui::Button::new(RichText::new(s(x, "name")).size(12.0).color(theme::ink())).frame(false)).clicked() {
                                            open = Some(format!("session:{}", s(x, "id")));
                                        }
                                    });
                                    ui.label("");
                                    ui.label(RichText::new(format!("{:.1}%", x["usage"]["cpu_percent"].as_f64().unwrap_or(0.0))).size(11.5).color(theme::text()));
                                    ui.label(RichText::new(mb(x["usage"]["rss_bytes"].as_u64().unwrap_or(0))).size(11.5).color(theme::text()));
                                    ui.end_row();
                                }
                            }
                        });
                        if groups.is_empty() {
                            ui.label(RichText::new("No terminals yet.").size(11.5).color(theme::muted()));
                        }
                    });
                    ui.label(RichText::new("Pause saves CPU and keeps memory; stop frees memory and the agent continues its conversation when started again.").size(10.5).color(theme::muted()));
                });

                // Keeping the computer awake.
                let awake = &st["awake"];
                if awake["supported"] == true {
                    let mode = s(awake, "mode").to_string();
                    let label = match mode.as_str() {
                        "on" => "awake: on",
                        "agents" => "awake: agents",
                        _ => "awake: off",
                    };
                    let active = awake["active"] == true;
                    let r = bar_button(ui, &format!("{} {label}", if active { "●" } else { "○" }))
                        .on_hover_text(if active { "kept awake now" } else { "the computer may sleep" });
                    egui::Popup::menu(&r).align(egui::RectAlign::TOP_END).width(300.0).show(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Keep computer awake").strong().color(theme::ink()));
                            ui.label(RichText::new(if active { "active" } else { "inactive" }).size(11.0).color(theme::muted()));
                        });
                        for (value, title, about) in [
                            ("on", "On", "Keep this computer awake all the time"),
                            ("agents", "While agents work", "Stay awake while an agent is working, and two minutes after"),
                            ("off", "Off", "The computer sleeps as usual"),
                        ] {
                            let mark = if mode == value { "●" } else { "  " };
                            let text = format!("{mark} {title}\n     {about}");
                            if ui.button(RichText::new(text).color(theme::ink())).clicked() && mode != value {
                                awake_to = Some(value);
                            }
                        }
                    });
                }
            });
        });
        if let Some(v) = go_view {
            if let Some(i) = host {
                self.switch_host(i);
            }
            self.view = v;
            self.refresh_view();
        } else if let Some(i) = host {
            self.switch_host(i);
        }
        if let Some(target) = open {
            match target.strip_prefix("session:") {
                Some(id) => self.open_session(id),
                None => open_url(&target),
            }
        }
        if let Some(mode) = awake_to {
            self.job(move |c| {
                c.post("/awake", json!({ "mode": mode }))?;
                Ok(Msg::Note(format!("Keep awake: {mode}")))
            });
            self.last_status = Instant::now() - Duration::from_secs(60);
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        // The kimos this window shows: this machine and the paired ones (their projects,
        // agents and terminals, as if they were here).
        let mut chosen = None;
        let mut connect = false;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, h) in self.hosts.iter().enumerate() {
                let selected = i == self.current;
                let online = h.loaded();
                let running = h.sessions().iter().filter(|x| s(x, "status") == "running").count();
                let text = RichText::new(format!("{} {}", if online { "●" } else { "○" }, h.name))
                    .size(12.0)
                    .color(if selected { theme::background() } else { theme::ink() });
                let button = egui::Button::new(text)
                    .fill(if selected { theme::ink() } else { theme::background() })
                    .stroke(egui::Stroke::new(1.0, if selected { theme::ink() } else { theme::line() }))
                    .corner_radius(6);
                let hover = if online { format!("{running} running") } else { "offline".into() };
                if ui.add(button).on_hover_text(hover).clicked() && !selected {
                    chosen = Some(i);
                }
            }
            connect = ui
                .add(egui::Button::new(RichText::new("+ kimo").size(12.0).color(theme::muted())).frame(false))
                .on_hover_text("connect the kimo of another machine (a Linux box, another Mac): its projects and agents show here")
                .clicked();
        });
        if let Some(i) = chosen {
            self.switch_host(i);
        }
        if connect {
            self.switch_host(0);
            self.view = View::Devices;
            self.refresh_view();
        }
        ui.add_space(4.0);
        let sessions = self.sessions();
        section(ui, "Projects");
        ui.horizontal_wrapped(|ui| {
            let add = egui::Button::new(RichText::new("+ project").color(theme::background()))
                .fill(theme::accent())
                .stroke(egui::Stroke::new(1.0, theme::accent()));
            if ui.add(add).clicked() {
                self.project_form = Some(ProjectForm {
                    mode: "new".into(),
                    git: true,
                    ..Default::default()
                });
            }
            ui.label(
                RichText::new("or drop a folder here")
                    .size(11.0)
                    .color(theme::muted()),
            );
        });
        ui.add_space(6.0);
        // Every session goes to the worktree (folder) it works in: the longest matching path.
        let folders: Vec<String> = self
            .projects
            .iter()
            .flat_map(|p| worktrees_of(p).into_iter().map(|w| w.path))
            .collect();
        let home_of = |x: &Value| -> Option<String> {
            let cwd = s(x, "cwd");
            folders
                .iter()
                .filter(|f| cwd == f.as_str() || cwd.starts_with(&format!("{f}/")))
                .max_by_key(|f| f.len())
                .cloned()
        };
        let placed: Vec<(Value, Option<String>)> =
            sessions.iter().map(|x| (x.clone(), home_of(x))).collect();
        let now = now_ms();
        let mut open: Option<String> = None;
        let mut session_act: Option<SessionAct> = None;
        let mut add_folder: Option<String> = None;
        let mut start: Option<(String, String, String)> = None;
        let mut history: Option<(String, String)> = None;
        let mut open_project: Option<String> = None;
        let mut toggle: Option<String> = None;
        let mut settings: Option<Value> = None;
        let mut worktree: Option<Value> = None;
        let mut remove_project: Option<(String, String)> = None;
        let mut remove_worktree: Option<(String, String)> = None;
        let mut reveal: Option<String> = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for p in &self.projects {
                    let (id, path, name) = (s(p, "id"), s(p, "path"), s(p, "name"));
                    let trees = worktrees_of(p);
                    let collapsed = self.collapsed.contains(id);
                    let mine: Vec<&Value> = placed
                        .iter()
                        .filter(|(_, home)| home.as_ref().is_some_and(|h| trees.iter().any(|w| &w.path == h)))
                        .map(|(x, _)| x)
                        .collect();
                    let running = mine.iter().filter(|x| s(x, "status") == "running").count();
                    let waiting = mine.iter().any(|x| s(x, "activity") == "waiting");
                    // Header: mark, name (opens its latest agent), then collapse, menu and +.
                    // The project whose terminals are shown is highlighted.
                    let shown = self.project.as_deref() == Some(id);
                    egui::Frame::new()
                        .fill(if shown { theme::background() } else { Color32::TRANSPARENT })
                        .stroke(egui::Stroke::new(1.0, if shown { theme::line() } else { Color32::TRANSPARENT }))
                        .corner_radius(6)
                        .inner_margin(egui::Margin::symmetric(4, 3))
                        .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        project_mark(ui, p);
                        let title = RichText::new(name).size(14.0).strong().color(theme::ink());
                        if ui
                            .add(egui::Label::new(title).sense(egui::Sense::click()).truncate())
                            .on_hover_text(format!("{path}\nclick: show its terminals"))
                            .clicked()
                        {
                            open_project = Some(id.to_string());
                        }
                        if waiting {
                            dot(ui, theme::accent());
                        } else if running > 0 {
                            ui.label(RichText::new(format!("{running}")).size(11.0).color(theme::good()))
                                .on_hover_text(format!("{running} running"));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let git = trees.first().is_some_and(|w| w.branch.is_some());
                            let plus = ui
                                .add_enabled(git, egui::Button::new(RichText::new("+").size(15.0).color(theme::ink())).frame(false))
                                .on_hover_text("new worktree: a branch in its own folder, with an agent")
                                .on_disabled_hover_text("worktrees need a git repository");
                            if plus.clicked() {
                                worktree = Some(p.clone());
                            }
                            flat_menu(ui, RichText::new("···").size(14.0).color(theme::ink()), |ui| {
                                if ui.button("Project settings…").clicked() {
                                    settings = Some(p.clone());
                                    ui.close();
                                }
                                if ui.button("Earlier conversations").clicked() {
                                    history = Some((name.to_string(), path.to_string()));
                                    ui.close();
                                }
                                if ui.button(reveal_label()).clicked() {
                                    reveal = Some(path.to_string());
                                    ui.close();
                                }
                                ui.separator();
                                if ui.button(RichText::new("Remove project").color(theme::bad())).clicked() {
                                    remove_project = Some((id.to_string(), name.to_string()));
                                    ui.close();
                                }
                            });
                            if chevron(ui, !collapsed)
                                .on_hover_text(if collapsed { "show its worktrees and agents" } else { "collapse" })
                                .clicked()
                            {
                                toggle = Some(id.to_string());
                            }
                        });
                    });
                        });
                    if collapsed {
                        ui.add_space(6.0);
                        continue;
                    }
                    for w in &trees {
                        let agents: Vec<&Value> = mine
                            .iter()
                            .copied()
                            .filter(|x| placed.iter().any(|(y, h)| s(y, "id") == s(x, "id") && h.as_deref() == Some(w.path.as_str())))
                            .collect();
                        egui::Frame::new()
                            .stroke(egui::Stroke::new(1.0, theme::line()))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(10, 8))
                            .outer_margin(egui::Margin { left: 0, right: 2, top: 2, bottom: 4 })
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let branch = w.branch.clone().unwrap_or_else(|| "folder".into());
                                    ui.add(egui::Label::new(RichText::new(&branch).size(13.0).color(theme::ink())).truncate());
                                    if w.primary && w.branch.is_some() {
                                        badge(ui, "primary");
                                    }
                                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                        flat_menu(ui, RichText::new("···").size(12.0).color(theme::muted()), |ui| {
                                            if ui.button("Earlier conversations").clicked() {
                                                history = Some((name.to_string(), w.path.clone()));
                                                ui.close();
                                            }
                                            if ui.button(reveal_label()).clicked() {
                                                reveal = Some(w.path.clone());
                                                ui.close();
                                            }
                                            if !w.primary
                                                && ui
                                                    .button(RichText::new("Remove worktree").color(theme::bad()))
                                                    .on_hover_text("git worktree remove: refused while it has uncommitted changes; the branch stays")
                                                    .clicked()
                                            {
                                                remove_worktree = Some((id.to_string(), w.path.clone()));
                                                ui.close();
                                            }
                                        });
                                    });
                                });
                                ui.label(RichText::new(short_path(&w.path, 34)).size(10.5).color(theme::muted()))
                                    .on_hover_text(&w.path);
                                if !agents.is_empty() {
                                    ui.label(
                                        RichText::new(format!("{} agent{}", agents.len(), if agents.len() == 1 { "" } else { "s" }))
                                            .size(10.5)
                                            .color(theme::muted()),
                                    );
                                }
                                for x in &agents {
                                    // Right side first (last output, state), so the name gets the rest.
                                    ui.horizontal(|ui| {
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            if let Some(at) = x["last_output_at"].as_u64() {
                                                ui.label(RichText::new(compact_ago(now, at)).size(10.0).color(theme::muted()))
                                                    .on_hover_text("last output");
                                            }
                                            activity_tag(ui, x);
                                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                                dot(ui, theme::status_color(s(x, "status")));
                                                ui.label(RichText::new(kind_label(s(x, "kind"))).size(10.0).color(theme::muted()));
                                                let r = ui
                                                    .add(egui::Button::new(RichText::new(s(x, "name")).color(theme::ink())).frame(false).truncate())
                                                    .on_hover_text(format!(
                                                        "{}\nid {}\nresume {}",
                                                        s(x, "name"),
                                                        s(x, "id"),
                                                        x["conversation"].as_str().unwrap_or("-")
                                                    ));
                                                if r.clicked() {
                                                    open = Some(s(x, "id").to_string());
                                                }
                                                session_menu(&r, x, &mut session_act);
                                            });
                                        });
                                    });
                                }
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    for kind in ["claude", "codex", "shell"] {
                                        let label = if kind == "shell" { "terminal" } else { kind };
                                        if link_button(ui, &format!("+ {label}")).on_hover_text(format!("start {label} here")).clicked() {
                                            start = Some((
                                                if w.primary { name.to_string() } else { format!("{name} {}", w.branch.clone().unwrap_or_default()) },
                                                w.path.clone(),
                                                kind.to_string(),
                                            ));
                                        }
                                    }
                                });
                            });
                    }
                    ui.add_space(8.0);
                }
                // Sessions outside every project (sign-in terminals are not listed: they end).
                let others: Vec<&Value> = placed
                    .iter()
                    .filter(|(x, home)| home.is_none() && !is_login_session(x))
                    .map(|(x, _)| x)
                    .collect();
                if !others.is_empty() {
                    section(ui, "Not in a project");
                    // By folder, each one a project away from its agents' own place.
                    let mut folders: Vec<(&str, Vec<&Value>)> = Vec::new();
                    for x in others {
                        match folders.iter_mut().find(|(f, _)| *f == s(x, "cwd")) {
                            Some((_, list)) => list.push(x),
                            None => folders.push((s(x, "cwd"), vec![x])),
                        }
                    }
                    for (folder, list) in folders {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(home(folder)).size(11.0).color(theme::muted()))
                                .on_hover_text(folder);
                            if link_button(ui, "+ add as project")
                                .on_hover_text("make this folder a project: its agents show under it")
                                .clicked()
                            {
                                add_folder = Some(folder.to_string());
                            }
                        });
                        for x in list {
                            ui.horizontal(|ui| {
                                ui.add_space(8.0);
                                dot(ui, theme::status_color(s(x, "status")));
                                let r = ui
                                    .selectable_label(false, s(x, "name"))
                                    .on_hover_text("click to open; right-click to stop or remove");
                                if r.clicked() {
                                    open = Some(s(x, "id").to_string());
                                }
                                session_menu(&r, x, &mut session_act);
                                activity_tag(ui, x);
                            });
                        }
                    }
                }
                if self.projects.is_empty() {
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(
                            "No projects yet. Drop a folder on this window or create one.",
                        )
                        .color(theme::muted()),
                    );
                }
            });
        if let Some(id) = toggle {
            if !self.collapsed.remove(&id) {
                self.collapsed.insert(id);
            }
            self.save_prefs();
        }
        if let Some(p) = settings {
            self.project_settings = Some(ProjectSettings {
                id: s(&p, "id").to_string(),
                name: s(&p, "name").to_string(),
                path: s(&p, "path").to_string(),
                color: s(&p, "color").to_string(),
                confirm_remove: false,
            });
        }
        if let Some(p) = worktree {
            self.worktree_form = Some(WorktreeForm {
                project: s(&p, "id").to_string(),
                project_name: s(&p, "name").to_string(),
                name: String::new(),
                agent: "claude".into(),
            });
        }
        if let Some((id, name)) = remove_project {
            self.project_settings = None;
            self.confirm = Some(Confirm::RemoveProject { id, name });
        }
        if let Some((project, path)) = remove_worktree {
            self.confirm = Some(Confirm::RemoveWorktree { project, path });
        }
        if let Some(path) = reveal {
            reveal_folder(&path);
        }
        if let Some(id) = open_project {
            self.open_project(&id, &placed);
        }
        if let Some(id) = open {
            self.open_session(&id);
        }
        if let Some(act) = session_act {
            self.session_act(act);
        }
        if let Some(path) = add_folder {
            self.last_projects = Instant::now() - Duration::from_secs(60);
            self.action("Project added: its agents show under it", move |c| {
                c.post(
                    "/projects",
                    json!({ "mode": "existing", "path": path, "resume": false }),
                )
            });
        }
        if let Some((name, folder)) = history {
            self.job(move |c| {
                let list = c.get(&format!("/conversations?path={}", query_escape(&folder)))?;
                Ok(Msg::History(name, folder, list))
            });
        }
        if let Some((label, cwd, kind)) = start {
            self.start_agent(&label, &cwd, &kind);
        }
    }

    /// Open a project's most recent agent; with none yet, continue its latest conversations
    /// (adding a known folder again does that).
    fn open_project(&mut self, id: &str, placed: &[(Value, Option<String>)]) {
        let Some(p) = self.projects.iter().find(|p| s(p, "id") == id).cloned() else {
            return;
        };
        self.view = View::Terminals;
        self.project = Some(id.to_string());
        // Its terminals already open: show them as they were.
        let host = self.host().id.clone();
        let open_here = self.terminals.iter().any(|t| {
            t.host == host
                && placed.iter().any(|(x, home)| {
                    s(x, "id") == t.session_id
                        && home
                            .as_ref()
                            .is_some_and(|h| worktrees_of(&p).iter().any(|w| &w.path == h))
                })
        });
        if open_here {
            return;
        }
        let trees = worktrees_of(&p);
        let latest = placed
            .iter()
            .filter(|(_, home)| {
                home.as_ref()
                    .is_some_and(|h| trees.iter().any(|w| &w.path == h))
            })
            .map(|(x, _)| x)
            .max_by_key(|x| {
                (
                    s(x, "status") == "running",
                    x["last_output_at"].as_u64().unwrap_or(0),
                )
            })
            .cloned();
        match latest {
            Some(x) => {
                let sid = s(&x, "id").to_string();
                self.open_session(&sid);
            }
            None => {
                let path = s(&p, "path").to_string();
                self.job(move |c| {
                    Ok(Msg::Added(c.post(
                        "/projects",
                        json!({ "mode": "existing", "path": path }),
                    )?))
                });
            }
        }
    }

    fn terminals_view(&mut self, ui: &mut Ui) {
        // Tabs of deleted sessions (or of forgotten hosts) close by themselves.
        let hosts = &self.hosts;
        // A tab opened for a brand-new session waits until the session shows up in the list.
        self.terminals
            .retain_mut(|t| match hosts.iter().find(|h| h.id == t.host) {
                Some(h) if h.loaded() => {
                    let listed = h.sessions().iter().any(|x| s(x, "id") == t.session_id);
                    t.seen |= listed;
                    listed || !t.seen
                }
                Some(_) => true,
                None => false,
            });
        let sessions = self.sessions();
        let host_id = self.host().id.clone();
        let info_of = |id: &str| sessions.iter().find(|x| s(x, "id") == id).cloned();
        // The tabs of the project shown (sign-in terminals show everywhere until they end).
        let visible: Vec<usize> = self
            .terminals
            .iter()
            .enumerate()
            .filter(|(_, t)| t.host == host_id)
            .filter(|(_, t)| match info_of(&t.session_id) {
                Some(x) => is_login_session(&x) || self.project_of(&x) == self.project,
                // Brand new: it was started in the project shown.
                None => true,
            })
            .map(|(i, _)| i)
            .collect();
        if !visible.contains(&self.active) {
            let remembered = self.last_tab.get(&self.project).and_then(|id| {
                visible
                    .iter()
                    .copied()
                    .find(|&i| self.terminals[i].session_id == *id)
            });
            if let Some(i) = remembered.or_else(|| visible.last().copied()) {
                self.active = i;
            }
        }
        let project = self
            .project
            .as_ref()
            .and_then(|id| self.projects.iter().find(|p| s(p, "id") == id))
            .cloned();
        let project_name = project.as_ref().map(|p| s(p, "name").to_string());
        // Where "+" starts things: the active tab's folder when it is in this project.
        let active_cwd = visible
            .contains(&self.active)
            .then(|| info_of(&self.terminals[self.active].session_id))
            .flatten()
            .and_then(|x| self.home_of(&x))
            .map(|(_, w)| w);
        let new_cwd = active_cwd
            .or_else(|| project.as_ref().map(|p| s(p, "path").to_string()))
            .unwrap_or_default();
        let label = project_name.clone().unwrap_or_default();

        // Tabs, then "+".
        let mut close = None;
        let mut open_editor: Option<usize> = None;
        let mut close_editor: Option<usize> = None;
        let mut tab_act: Option<SessionAct> = None;
        let mut start: Option<&'static str> = None;
        ui.horizontal_wrapped(|ui| {
            if let Some(name) = &project_name {
                ui.label(RichText::new(name).size(14.0).strong().color(theme::ink()));
                ui.add_space(6.0);
            }
            for &i in &visible {
                let t = &self.terminals[i];
                let info = info_of(&t.session_id);
                let name = info
                    .as_ref()
                    .map(|x| s(x, "name").to_string())
                    .unwrap_or_else(|| t.session_id.clone());
                let status = info
                    .as_ref()
                    .map(|x| s(x, "status").to_string())
                    .unwrap_or_default();
                let selected = i == self.active;
                egui::Frame::new()
                    .fill(if selected {
                        theme::surface()
                    } else {
                        theme::background()
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if selected {
                            theme::ink()
                        } else {
                            theme::line()
                        },
                    ))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            dot(ui, theme::status_color(&status));
                            if let Some(x) = &info {
                                ui.label(
                                    RichText::new(kind_label(s(x, "kind")))
                                        .size(10.0)
                                        .color(theme::muted()),
                                );
                            }
                            let r = ui
                                .add(
                                    egui::Button::new(RichText::new(&name).color(theme::ink()))
                                        .frame(false),
                                )
                                .on_hover_text("right-click to stop or remove");
                            if r.clicked() {
                                self.active = i;
                                self.editor_active = None;
                            }
                            if let Some(x) = &info {
                                session_menu(&r, x, &mut tab_act);
                            }
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("×").color(theme::muted()))
                                        .frame(false),
                                )
                                .on_hover_text("close the tab (the session keeps running)")
                                .clicked()
                            {
                                close = Some(i);
                            }
                        });
                    });
            }
            // Files open in this project, as tabs after its terminals.
            for (j, e) in self.editors.iter().enumerate() {
                if e.scope.host != host_id || Some(&e.scope.project) != self.project.as_ref() {
                    continue;
                }
                let selected = self.editor_active == Some(j);
                egui::Frame::new()
                    .fill(if selected {
                        theme::surface()
                    } else {
                        theme::background()
                    })
                    .stroke(egui::Stroke::new(
                        1.0,
                        if selected {
                            theme::ink()
                        } else {
                            theme::line()
                        },
                    ))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let name = e.path.rsplit('/').next().unwrap_or(&e.path);
                            let text = format!("≡ {name}{}", if e.dirty() { " ●" } else { "" });
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(text).italics().color(theme::ink()),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text(&e.path)
                                .clicked()
                            {
                                open_editor = Some(j);
                            }
                            if ui
                                .add(
                                    egui::Button::new(RichText::new("×").color(theme::muted()))
                                        .frame(false),
                                )
                                .on_hover_text("close the file")
                                .clicked()
                            {
                                close_editor = Some(j);
                            }
                        });
                    });
            }
            // Terminals belong to a project: "+" only inside one.
            if project.is_some() {
                let plus = egui::Button::new(RichText::new("+").size(16.0).color(theme::ink()))
                    .frame(false);
                egui::containers::menu::MenuButton::from_button(plus)
                    .ui(ui, |ui| {
                        ui.set_min_width(190.0);
                        let place = if label.is_empty() {
                            "your home folder".to_string()
                        } else {
                            short_path(&new_cwd, 30)
                        };
                        ui.label(
                            RichText::new(format!("in {place}"))
                                .size(10.5)
                                .color(theme::muted()),
                        );
                        for (kind, text) in [
                            ("shell", "New terminal"),
                            ("claude", "Claude"),
                            ("codex", "Codex"),
                        ] {
                            let shortcut =
                                (kind == "shell").then_some(if cfg!(target_os = "macos") {
                                    "⌘T"
                                } else {
                                    "Ctrl+T"
                                });
                            let button =
                                egui::Button::new(text).shortcut_text(shortcut.unwrap_or(""));
                            if ui.add(button).clicked() {
                                start = Some(kind);
                                ui.close();
                            }
                        }
                    })
                    .0
                    .on_hover_text("new terminal or agent in this project");
            }
        });
        if project.is_some()
            && ui
                .ctx()
                .input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::T))
        {
            start = Some("shell");
        }
        if let Some(kind) = start {
            self.start_agent(&label, &new_cwd, kind);
        }
        if let Some(act) = tab_act {
            self.session_act(act);
        }
        if let Some(j) = open_editor {
            self.editor_active = Some(j);
        }
        if let Some(j) = close_editor {
            if self.editors[j].dirty() {
                self.confirm = Some(Confirm::CloseFile { index: j });
            } else {
                self.close_editor(j);
            }
        }
        // An open file of this project is shown instead of a terminal.
        if let Some(j) = self.editor_active.filter(|&j| {
            self.editors.get(j).is_some_and(|e| {
                e.scope.host == host_id && Some(&e.scope.project) == self.project.as_ref()
            })
        }) {
            ui.add_space(4.0);
            self.editor_view(ui, j);
            return;
        }
        if let Some(i) = close {
            self.terminals.remove(i);
            if self.active > i {
                self.active -= 1;
            }
            return;
        }
        if !visible.contains(&self.active) || self.terminals.is_empty() {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| match &project_name {
                Some(name) => {
                    ui.label(RichText::new(format!("No terminal open in {name}")).size(22.0).color(theme::ink()));
                    ui.label(RichText::new("Start an agent or a terminal here, or open one of its agents on the left.").color(theme::muted()));
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        let width = 3.0 * 120.0;
                        ui.add_space((ui.available_width() - width).max(0.0) / 2.0);
                        for (kind, text) in [("claude", "+ Claude"), ("codex", "+ Codex"), ("shell", "+ Terminal")] {
                            if ui.add(egui::Button::new(text).min_size(egui::vec2(110.0, 30.0))).clicked() {
                                self.start_agent(&label, &new_cwd, kind);
                            }
                        }
                    });
                }
                None => {
                    ui.label(RichText::new("No terminal open").size(22.0).color(theme::ink()));
                    ui.label(RichText::new("Open a project on the left, add an agent to one, or drop a folder to start a project.").color(theme::muted()));
                    ui.add_space(16.0);
                    if primary(ui, "start a new project from zero").on_hover_text("a new folder (with git), ready for agents").clicked() {
                        self.project_form = Some(ProjectForm { mode: "new".into(), git: true, ..Default::default() });
                    }
                }
            });
            return;
        }
        self.last_tab.insert(
            self.project.clone(),
            self.terminals[self.active].session_id.clone(),
        );

        // Toolbar of the active session.
        let id = self.terminals[self.active].session_id.clone();
        let info = sessions
            .iter()
            .find(|x| s(x, "id") == id)
            .cloned()
            .unwrap_or(Value::Null);
        let status = s(&info, "status").to_string();
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            dot(ui, theme::status_color(&status));
            ui.label(RichText::new(&status).size(11.0).color(theme::status_color(&status)));
            activity_tag(ui, &info);
            // Ids: kimo's id (for kimo send/read) and the agent's own resume id.
            for (label, value) in [("id", s(&info, "id").to_string()), ("resume", info["conversation"].as_str().unwrap_or("").to_string())] {
                if !value.is_empty() {
                    // Long resume ids are shortened; a click copies the full value.
                    let shown = if label == "resume" && value.chars().count() > 14 { format!("{}…", value.chars().take(8).collect::<String>()) } else { value.clone() };
                    let r = ui.add(egui::Button::new(RichText::new(format!("{label} {shown}")).size(10.5).color(theme::muted())).frame(false));
                    if r.on_hover_text(format!("{value}\nclick to copy")).clicked() {
                        ui.ctx().copy_text(value.clone());
                        self.toasts.push((format!("Copied {label}: {value}"), false, Instant::now()));
                    }
                }
            }
            let usage = &info["usage"];
            if status == "running" || status == "paused" {
                let heavy = self.memory["heavy"].as_array().is_some_and(|h| h.iter().any(|h| h == id.as_str()));
                let text = RichText::new(format!("{} · {}% cpu", mb(usage["rss_bytes"].as_u64().unwrap_or(0)), usage["cpu_percent"])).size(11.0);
                let r = ui
                    .add(egui::Button::new(if heavy { text.color(theme::warn()) } else { text.color(theme::muted()) }).frame(false))
                    .on_hover_text(if heavy { "uses a lot of memory: click to see what" } else { "click to see what uses the memory" });
                if r.clicked() {
                    if self.parts.as_ref().is_some_and(|(p, _)| *p == id) {
                        self.parts = None;
                    } else {
                        let sid = id.clone();
                        self.job(move |c| Ok(Msg::Parts(sid.clone(), c.get(&format!("/sessions/{sid}/memory"))?)));
                    }
                }
            }
            ui.separator();
            let act = |app: &Self, action: &'static str, done: &'static str| {
                let id = id.clone();
                app.action(done, move |c| c.post(&format!("/sessions/{id}/action"), json!({ "action": action })));
            };
            match status.as_str() {
                "running" => {
                    if ui.button("pause").on_hover_text("freeze: no CPU, memory kept").clicked() { act(self, "pause", "Paused"); }
                    if ui.button("stop").on_hover_text("free the memory; continue later").clicked() { act(self, "stop", "Stopped: memory freed"); }
                }
                "paused" => {
                    if ui.button("resume").clicked() { act(self, "resume", "Resumed"); }
                    if ui.button("stop").clicked() { act(self, "stop", "Stopped: memory freed"); }
                }
                "" => {}
                _ => {
                    if primary(ui, "continue").clicked() { act(self, "start", "Started"); }
                }
            }
            // Subscription switch for agent sessions.
            let kind = s(&info, "kind").to_string();
            if kind != "shell" && info["command"].is_null() && !kind.is_empty() {
                let current = info["account"].as_str().unwrap_or("").to_string();
                let accounts = self.accounts();
                // This computer's own login is the empty key (no account).
                let options: Vec<(String, String)> = accounts
                    .iter()
                    .filter(|a| s(a, "provider") == kind)
                    .map(|a| (if a["system"] == true { String::new() } else { s(a, "id").to_string() }, s(a, "name").to_string()))
                    .collect();
                let label = options.iter().find(|(k, _)| *k == current).map(|(_, n)| n.clone()).unwrap_or_else(|| if current.is_empty() { "this computer's login".into() } else { current.clone() });
                let mut chosen = current.clone();
                egui::ComboBox::from_id_salt(("account", &id)).selected_text(label).width(170.0).show_ui(ui, |ui| {
                    for (key, name) in &options {
                        ui.selectable_value(&mut chosen, key.clone(), name);
                    }
                });
                if chosen != current {
                    let id = id.clone();
                    self.action("Switched subscription: the conversation continues", move |c| {
                        c.post(&format!("/sessions/{id}/account"), json!({ "account": if chosen.is_empty() { Value::Null } else { Value::String(chosen) } }))
                    });
                }
            }
            if ui
                .button("redraw")
                .on_hover_text("garbled after a resize? reload the terminal and ask the program to draw itself again")
                .clicked()
            {
                let t = &self.terminals[self.active];
                if let Some(host) = self.hosts.iter().find(|h| h.id == t.host) {
                    let mut fresh = TerminalView::open(&host.client, &t.host, &t.session_id, &self.ctx);
                    fresh.seen = true;
                    fresh.redraw();
                    self.terminals[self.active] = fresh;
                }
            }
            // Quick commands.
            let commands: Vec<QuickCommand> = self.settings.as_ref().map(|st| st.quick_commands.clone()).unwrap_or_default();
            if !commands.is_empty() {
                ui.menu_button("quick commands", |ui| {
                    for q in &commands {
                        if ui.button(&q.name).on_hover_text(&q.text).clicked() {
                            self.terminals[self.active].send(&q.text);
                            self.terminals[self.active].send("\r");
                            ui.close();
                        }
                    }
                });
            }
        });
        // What the agent's memory is made of (the memory figure in the toolbar toggles it).
        if let Some((_, parts)) = self.parts.as_ref().filter(|(p, _)| *p == id) {
            let parts = parts.as_array().cloned().unwrap_or_default();
            let mut close = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("memory:").size(11.0).color(theme::muted()));
                for (i, p) in parts.iter().enumerate() {
                    let label = if i == 0 { format!("{} itself", s(p, "label")) } else { s(p, "label").to_string() };
                    let n = p["processes"].as_u64().unwrap_or(1);
                    ui.label(RichText::new(format!("{label} {}", mb(p["bytes"].as_u64().unwrap_or(0)))).size(11.0).color(theme::text()))
                        .on_hover_text(format!("pid {}{}", p["pid"], if n > 1 { format!(", {n} processes") } else { String::new() }));
                }
                if parts.iter().any(|p| s(p, "label").to_ascii_lowercase().contains("mcp")) {
                    ui.label(RichText::new("· each agent starts its own MCP servers: turn off the ones a project does not need").size(10.5).color(theme::muted()));
                }
                close = ui.add(egui::Button::new(RichText::new("×").color(theme::muted())).frame(false)).clicked();
            });
            if close {
                self.parts = None;
            }
        }

        // Message box at the bottom, terminal above it.
        let font_size = self
            .settings
            .as_ref()
            .map_or(13.0, |st| st.terminal.font_size);
        egui::Panel::bottom(("message", self.active))
            .resizable(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.message)
                            .hint_text("Message the agent… (Enter sends)")
                            .desired_width(ui.available_width() - 70.0),
                    );
                    let send = ui.button("send").clicked()
                        || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if send && !self.message.is_empty() {
                        let text = std::mem::take(&mut self.message);
                        let t = &self.terminals[self.active];
                        t.send(&text);
                        t.send("\r");
                    }
                });
            });
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, theme::line()))
            .show(ui, |ui| {
                self.terminals[self.active].show(ui, font_size);
            });
        if let Some(image) = self.terminals[self.active].pasted_image.take() {
            self.paste_image(image);
        }
    }

    fn session_act(&mut self, act: SessionAct) {
        match act {
            SessionAct::Open(id) => self.open_session(&id),
            SessionAct::Stop(id) => self.action("Stopped: memory freed", move |c| {
                c.post(
                    &format!("/sessions/{id}/action"),
                    json!({ "action": "stop" }),
                )
            }),
            SessionAct::Remove { id, name } => {
                self.confirm = Some(Confirm::RemoveSession { id, name })
            }
        }
    }

    /// An image pasted into the terminal. Claude Code and Codex on this machine read it from
    /// the clipboard themselves (Ctrl+V, as in any terminal: "[Image #1]"); on another kimo,
    /// or in a shell, it is saved on the session's machine and its path typed.
    fn paste_image(&mut self, image: Arc<egui::ColorImage>) {
        let t = &self.terminals[self.active];
        let info = self
            .sessions()
            .into_iter()
            .find(|x| s(x, "id") == t.session_id)
            .unwrap_or(Value::Null);
        let agent = matches!(s(&info, "kind"), "claude" | "codex") && info["command"].is_null();
        if agent && t.host == "local" {
            t.send("\x16");
            return;
        }
        let id = t.session_id.clone();
        self.toast("Pasting the image…", false);
        self.job(move |c| {
            use base64::Engine;
            let png = encode_png(&image)?;
            let name = format!("pasted-{}.png", &uuid::Uuid::new_v4().simple().to_string()[..8]);
            let r = c.post(
                &format!("/sessions/{id}/files"),
                json!({ "name": name, "data": base64::engine::general_purpose::STANDARD.encode(&png) }),
            )?;
            Ok(Msg::Type(id, format!("{} ", s(&r, "path"))))
        });
    }

    fn subscriptions_view(&mut self, ui: &mut Ui) {
        let accounts = self.accounts();
        let claude_limits = self.settings.as_ref().is_some_and(|st| st.claude_limits);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let mut use_account: Option<Switch> = None;
        let mut sign_in: Option<String> = None;
        let mut remove: Option<(String, String)> = None;
        let mut add: Option<(String, String)> = None;
        let mut limits_on = false;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.set_max_width(760.0);
            ui.heading(RichText::new("Subscriptions").color(theme::ink()));
            ui.label(RichText::new("New agents start on the active subscription of their provider. Switching can move the running agents too: each continues its conversation.").color(theme::muted()));
            for (provider, title) in [("claude", "Claude"), ("codex", "Codex")] {
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).size(18.0).strong().color(theme::ink()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+ Add account").on_hover_text("another subscription: sign in to it in a terminal").clicked() {
                            self.adding_account = Some(provider.to_string());
                            self.account_name.clear();
                        }
                    });
                });
                if self.adding_account.as_deref() == Some(provider) {
                    ui.horizontal(|ui| {
                        let edit = ui.add(egui::TextEdit::singleline(&mut self.account_name).hint_text("a name: Work Max, Personal…").desired_width(260.0));
                        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (primary(ui, "add and sign in").clicked() || enter) && !self.account_name.trim().is_empty() {
                            add = Some((std::mem::take(&mut self.account_name), provider.to_string()));
                        }
                        if ui.button("cancel").clicked() {
                            self.adding_account = None;
                        }
                    });
                }
                let mine: Vec<&Value> = accounts.iter().filter(|a| s(a, "provider") == provider).collect();
                if mine.is_empty() {
                    ui.label(RichText::new(format!("Not signed in to {title} on this computer. Add an account to sign in.")).size(12.0).color(theme::muted()));
                }
                let others_agents = |id: &str| -> usize {
                    mine.iter().filter(|b| s(b, "id") != id).map(|b| b["sessions"].as_u64().unwrap_or(0) as usize).sum()
                };
                for a in &mine {
                    let (id, name) = (s(a, "id").to_string(), s(a, "name").to_string());
                    let system = a["system"] == true;
                    let active = a["active"] == true;
                    let agents = a["sessions"].as_u64().unwrap_or(0);
                    let key = if system { Value::Null } else { Value::String(id.clone()) };
                    let usage = self.usage.iter().find(|u| u["account"] == key && s(u, "provider") == provider);
                    egui::Frame::new()
                        .stroke(egui::Stroke::new(1.0, if active { theme::ink() } else { theme::line() }))
                        .corner_radius(8)
                        .inner_margin(egui::Margin::symmetric(14, 10))
                        .outer_margin(egui::Margin { left: 0, right: 0, top: 6, bottom: 0 })
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&name).size(14.0).color(theme::ink()));
                                if active {
                                    badge(ui, "Active");
                                }
                                if system {
                                    badge(ui, "this computer's login");
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let signed_in = a["signed_in"] == true;
                                    if !system {
                                        // Less common actions in a menu: signing in again (an
                                        // expired login) and removing.
                                        ui.menu_button(RichText::new("···").color(theme::muted()), |ui| {
                                            if ui.button("Sign in again").on_hover_text("opens a terminal with this account's login").clicked() {
                                                sign_in = Some(id.clone());
                                            }
                                            let r = ui.add_enabled(agents == 0, egui::Button::new(RichText::new("Remove").color(theme::bad())))
                                                .on_disabled_hover_text(format!("{agents} agents use it: move them first"));
                                            if r.clicked() {
                                                remove = Some((id.clone(), name.clone()));
                                            }
                                        });
                                        if !signed_in && primary(ui, "Sign in").on_hover_text("opens a terminal with this account's login").clicked() {
                                            sign_in = Some(id.clone());
                                        }
                                    }
                                    if !active && primary(ui, "Use").on_hover_text("new agents start on it; you choose whether running ones move").clicked() {
                                        use_account = Some(Switch {
                                            provider: provider.to_string(),
                                            id: id.clone(),
                                            name: name.clone(),
                                            agents: others_agents(&id),
                                            move_agents: true,
                                        });
                                    }
                                });
                            });
                            let mut meta = vec![];
                            if let Some(email) = a["email"].as_str().filter(|e| *e != name) {
                                meta.push(email.to_string());
                            }
                            meta.push(format!("{agents} agent{}", if agents == 1 { "" } else { "s" }));
                            ui.horizontal(|ui| {
                                if a["signed_in"] == true {
                                    ui.label(RichText::new("✓ signed in").size(11.5).color(theme::good()));
                                } else {
                                    ui.label(RichText::new("not signed in").size(11.5).color(theme::warn()));
                                }
                                ui.label(RichText::new(meta.join(" · ")).size(11.5).color(theme::muted()));
                            });
                            limit_rows(ui, usage, now);
                        });
                }
                if provider == "claude" && !claude_limits && !mine.is_empty() {
                    ui.add_space(6.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("See each Claude subscription's 5-hour and weekly use, and when it runs out at the current pace.").size(11.5).color(theme::text()));
                        if ui.button("show plan limits").on_hover_text(if cfg!(target_os = "macos") { "kimo reads Claude's login to ask Claude for them: macOS asks you once to allow it (Always Allow)" } else { "kimo reads Claude's login to ask Claude for them" }).clicked() {
                            limits_on = true;
                        }
                    });
                }
            }
            ui.add_space(14.0);
            ui.label(RichText::new("Plan limits come from Claude and Codex themselves. Costs per model are in Usage.").size(11.0).color(theme::muted()));
        });
        if let Some(sw) = use_account {
            self.switch = Some(sw);
        }
        if let Some(id) = sign_in {
            self.job(move |c| {
                Ok(Msg::Open(
                    c.post(&format!("/accounts/{id}/login"), json!({}))?,
                ))
            });
        }
        if let Some((id, name)) = remove {
            self.confirm = Some(Confirm::RemoveAccount { id, name });
        }
        if let Some((name, provider)) = add {
            self.adding_account = None;
            // Add it, then open its sign-in terminal.
            self.job(move |c| {
                let a = c.post("/accounts", json!({ "name": name, "provider": provider }))?;
                Ok(Msg::Open(c.post(
                    &format!("/accounts/{}/login", s(&a, "id")),
                    json!({}),
                )?))
            });
        }
        if limits_on {
            if let Some(st) = self.settings.as_mut() {
                st.claude_limits = true;
            }
            self.job(|c| {
                let mut current: Value = c.get("/settings")?;
                current["plan_limits"] = json!(true);
                c.put("/settings", current)?;
                Ok(Msg::Usage(c.get("/usage")?))
            });
        }
    }

    /// "Use this subscription?" with the choice to move the running agents too.
    fn switch_dialog(&mut self, ctx: &egui::Context) {
        let Some(sw) = self.switch.as_mut() else {
            return;
        };
        let (mut yes, mut no) = (false, false);
        let provider = if sw.provider == "codex" {
            "Codex"
        } else {
            "Claude"
        };
        egui::Window::new(format!("Use {}?", sw.name))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("New {provider} agents start on {}.", sw.name)).color(theme::text()));
                if sw.agents > 0 {
                    ui.add_space(6.0);
                    ui.checkbox(
                        &mut sw.move_agents,
                        format!(
                            "Also move the {} {provider} agent{} on other subscriptions (running ones restart and continue their conversation)",
                            sw.agents,
                            if sw.agents == 1 { "" } else { "s" }
                        ),
                    );
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    yes = primary(ui, &format!("Use {}", sw.name)).clicked();
                    no = ui.button("cancel").clicked();
                });
            });
        if yes {
            if let Some(sw) = self.switch.take() {
                let body = json!({
                    "provider": sw.provider,
                    "account": if sw.id == "system" { Value::Null } else { Value::String(sw.id) },
                    "move": sw.move_agents && sw.agents > 0,
                });
                let name = sw.name;
                self.job(move |c| {
                    let r = c.post("/accounts/active", body)?;
                    Ok(Msg::Toast(match r["moved"].as_u64().unwrap_or(0) {
                        0 => format!("{name} is the active subscription"),
                        n => format!("{name} is the active subscription · moved {n} agents"),
                    }))
                });
            }
        } else if no {
            self.switch = None;
        }
    }

    fn skills_view(&mut self, ui: &mut Ui) {
        ui.heading(RichText::new("Skills").color(theme::ink()));
        ui.label(
            RichText::new(
                "What your agents know how to do. Installed once, they work on every subscription.",
            )
            .color(theme::muted()),
        );
        section(ui, "Agents working together");
        let has_kimo_skill = self.skills.iter().any(|sk| s(sk, "name") == "kimo-agents");
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("The kimo-agents skill teaches Claude and Codex to list the other agents, message them, wait for them and read their answers (kimo agents · send · wait · read).").size(11.5));
        });
        ui.horizontal(|ui| {
            let label = if has_kimo_skill {
                "reinstall kimo-agents"
            } else {
                "install kimo-agents for Claude and Codex"
            };
            if primary(ui, label).clicked() {
                self.job(|c| {
                    c.post("/skills/kimo", json!({ "agents": ["claude", "codex"] }))?;
                    Ok(Msg::Toast(
                        "kimo-agents installed: new agent sessions can talk to each other".into(),
                    ))
                });
            }
            if has_kimo_skill {
                ui.label(RichText::new("installed").size(11.0).color(theme::good()));
            }
        });
        section(ui, "Install");
        let f = &mut self.skill_form;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut f.source)
                    .hint_text("owner/repo/folder, a git URL or a folder")
                    .desired_width(360.0),
            );
            ui.add(
                egui::TextEdit::singleline(&mut f.skill)
                    .hint_text("only this skill (optional)")
                    .desired_width(200.0),
            );
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut f.claude, "Claude");
            ui.checkbox(&mut f.codex, "Codex");
            ui.checkbox(&mut f.shared, "shared (.agents)");
            ui.separator();
            let projects = &self.projects;
            let label = projects
                .iter()
                .find(|p| s(p, "id") == f.project)
                .map_or("global".to_string(), |p| s(p, "name").to_string());
            egui::ComboBox::from_id_salt("skill-scope")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut f.project, String::new(), "global");
                    for p in projects {
                        ui.selectable_value(&mut f.project, s(p, "id").to_string(), s(p, "name"));
                    }
                });
            ui.checkbox(&mut f.replace, "replace existing");
        });
        if primary(ui, "install").clicked() && !self.skill_form.source.trim().is_empty() {
            let f = &self.skill_form;
            let agents: Vec<&str> = [
                (f.claude, "claude"),
                (f.codex, "codex"),
                (f.shared, "shared"),
            ]
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, a)| *a)
            .collect();
            let body = json!({
                "source": f.source.trim(), "agents": agents, "replace": f.replace,
                "skill": if f.skill.trim().is_empty() { Value::Null } else { Value::String(f.skill.trim().into()) },
                "project": if f.project.is_empty() { Value::Null } else { Value::String(f.project.clone()) },
            });
            self.job(move |c| {
                let r = c.post("/skills", body)?;
                Ok(Msg::Toast(format!(
                    "Installed {} skill copies",
                    r.as_array().map_or(0, Vec::len)
                )))
            });
        }
        ui.add_space(6.0);
        let mut remove = None;
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut scope = String::new();
                for sk in &self.skills {
                    if s(sk, "scope") != scope {
                        scope = s(sk, "scope").to_string();
                        let title = if scope == "global" {
                            "Global".to_string()
                        } else {
                            self.projects
                                .iter()
                                .find(|p| s(p, "id") == scope)
                                .map_or(scope.clone(), |p| s(p, "name").to_string())
                        };
                        section(ui, &title);
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(s(sk, "name")).color(theme::ink()).size(13.5));
                        ui.label(
                            RichText::new(s(sk, "agent"))
                                .size(10.5)
                                .color(theme::teal()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if sk["managed"].as_bool() == Some(true) {
                                ui.label(
                                    RichText::new("synced / built in")
                                        .size(10.5)
                                        .color(theme::muted()),
                                );
                            } else if ui
                                .small_button(RichText::new("remove").color(theme::bad()))
                                .on_hover_text("moved to ~/.kimo/skill-backups")
                                .clicked()
                            {
                                remove = Some(s(sk, "path").to_string());
                            }
                        });
                    });
                    let description = s(sk, "description");
                    if !description.is_empty() {
                        let short: String = description.chars().take(180).collect();
                        let short = if short.len() < description.len() {
                            format!("{}…", short.trim_end())
                        } else {
                            short
                        };
                        ui.label(RichText::new(short).size(11.0).color(theme::muted()))
                            .on_hover_text(description);
                    }
                    ui.add_space(4.0);
                }
                if self.skills.is_empty() {
                    ui.label(RichText::new("No skills found.").color(theme::muted()));
                }
            });
        if let Some(path) = remove {
            self.job(move |c| {
                let r = c.delete(&format!("/skills?path={}", path.replace(' ', "%20")))?;
                Ok(Msg::Toast(format!(
                    "Removed (backup in {})",
                    home(s(&r, "backup"))
                )))
            });
        }
    }

    fn settings_view(&mut self, ui: &mut Ui) {
        ui.heading(RichText::new("Settings").color(theme::ink()));
        let Some(settings) = self.settings.as_mut() else {
            ui.label("Loading…");
            return;
        };
        let mut save = false;
        let mut new_theme = None;
        let mut check_now = false;
        let mut icon_to: Option<bool> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Appearance");
            ui.horizontal(|ui| {
                for (value, label) in [(Theme::System, "like the system"), (Theme::Light, "light"), (Theme::Dark, "dark")] {
                    if ui.radio(settings.theme == value, label).clicked() && settings.theme != value {
                        settings.theme = value;
                        new_theme = Some(value);
                    }
                }
            });
            ui.label(RichText::new("For this window and the phone app. Saved at once.").size(11.0).color(theme::muted()));
            if cfg!(target_os = "macos") {
                ui.horizontal(|ui| {
                    ui.label("app icon");
                    for (dark, label) in [(false, "light"), (true, "dark")] {
                        if ui.radio(self.dark_icon == dark, label).clicked() && self.dark_icon != dark {
                            icon_to = Some(dark);
                        }
                    }
                });
                ui.label(RichText::new("The Dock icon while kimo runs and kimo in Applications.").size(11.0).color(theme::muted()));
            }
            section(ui, "Agents");
            for (name, prefs) in [("Claude Code", &mut settings.agents.claude), ("Codex", &mut settings.agents.codex)] {
                let kind = if name == "Codex" { "codex" } else { "claude" };
                let found = self.agents.iter().find(|a| s(a, "kind") == kind);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).color(theme::ink()).size(14.0));
                    match found {
                        Some(a) if a["installed"].as_bool() == Some(true) => ui.label(RichText::new(s(a, "version")).size(11.0).color(theme::good())),
                        Some(_) => ui.label(RichText::new("not installed").size(11.0).color(theme::bad())),
                        None => ui.label(""),
                    };
                });
                egui::Grid::new(("prefs", kind)).num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    ui.label("enabled");
                    ui.checkbox(&mut prefs.enabled, "");
                    ui.end_row();
                    ui.label("permissions");
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut prefs.permissions, Permissions::Manual, "ask (manual)");
                        ui.radio_value(&mut prefs.permissions, Permissions::Yolo, "skip every prompt (yolo)");
                    });
                    ui.end_row();
                    ui.label("model");
                    ui.add(egui::TextEdit::singleline(&mut prefs.model).hint_text("the CLI's default").desired_width(220.0));
                    ui.end_row();
                    ui.label("effort");
                    let provider = if kind == "codex" { crate::core::models::Provider::Codex } else { crate::core::models::Provider::Claude };
                    egui::ComboBox::from_id_salt(("effort", kind))
                        .selected_text(if prefs.effort.is_empty() { "the CLI's default".to_string() } else { prefs.effort.clone() })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut prefs.effort, String::new(), "the CLI's default");
                            for level in crate::core::launch::effort_levels(provider) {
                                ui.selectable_value(&mut prefs.effort, level.to_string(), *level);
                            }
                        });
                    ui.end_row();
                    ui.label("extra arguments");
                    let mut args = prefs.args.join(" ");
                    if ui.add(egui::TextEdit::singleline(&mut args).hint_text("--verbose").desired_width(320.0)).changed() {
                        prefs.args = args.split_whitespace().map(String::from).collect();
                    }
                    ui.end_row();
                });
                if prefs.permissions == Permissions::Yolo {
                    ui.label(RichText::new("Agents will run commands and edit files without asking. Use it on machines and folders you can afford to lose.").size(11.0).color(theme::bad()));
                }
                ui.add_space(8.0);
            }
            section(ui, "Terminal");
            ui.horizontal(|ui| {
                ui.label("font size");
                ui.add(egui::DragValue::new(&mut settings.terminal.font_size).range(9.0..=22.0).speed(0.25).suffix(" px"));
            });
            section(ui, "Memory");
            let mem = &mut settings.memory;
            ui.checkbox(&mut mem.free_when_low, "when memory is almost full, stop idle agents (biggest first)");
            ui.horizontal(|ui| {
                ui.label("stop agents idle for more than");
                ui.add(egui::DragValue::new(&mut mem.stop_idle_after_minutes).range(0..=1440).speed(1.0).suffix(" min"));
                ui.label(RichText::new(if mem.stop_idle_after_minutes == 0 { "(never)" } else { "" }).size(11.0).color(theme::muted()));
            });
            ui.horizontal(|ui| {
                ui.label("show agents over");
                ui.add(egui::DragValue::new(&mut mem.heavy_mb).range(200..=32768).speed(50.0).suffix(" MB"));
                ui.label("as heavy");
            });
            ui.label(RichText::new("Only Claude and Codex agents waiting at their prompt are stopped, never working ones, ones asking you something or shells; they continue their conversation when you open them. Most of an agent's memory is often its MCP servers: click an agent's memory figure to see.").size(11.0).color(theme::muted()));
            section(ui, "Updates");
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("kimo {}", s(&self.status, "version"))).color(theme::ink()));
                if self.current == 0 && ui.small_button("check now").clicked() {
                    check_now = true;
                }
            });
            ui.checkbox(&mut settings.updates.notify, "notify me when a new version is out");
            ui.checkbox(&mut settings.updates.auto, "update by itself (when no agent is working)");
            ui.label(RichText::new("kimo checks for new versions every hour. Updating restarts running agents; they continue their conversations.").size(11.0).color(theme::muted()));
            section(ui, "Quick commands");
            ui.label(RichText::new("Saved prompts and commands, sent to the open terminal with one click (also on your phone).").size(11.0).color(theme::muted()));
            let mut drop = None;
            for (i, q) in settings.quick_commands.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut q.name).hint_text("name").desired_width(160.0));
                    ui.add(egui::TextEdit::singleline(&mut q.text).hint_text("text sent to the terminal").desired_width(420.0));
                    if ui.small_button(RichText::new("remove").color(theme::bad())).clicked() {
                        drop = Some(i);
                    }
                });
            }
            if let Some(i) = drop {
                settings.quick_commands.remove(i);
            }
            if ui.button("+ quick command").clicked() {
                settings.quick_commands.push(QuickCommand { name: String::new(), text: String::new(), project: None, extra: Default::default() });
            }
            ui.add_space(14.0);
            save = primary(ui, "save settings").clicked();
        });
        let body = save.then(|| serde_json::to_value(&*settings).unwrap_or(Value::Null));
        if let Some(dark) = icon_to {
            self.dark_icon = dark;
            self.save_prefs();
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(
                    super::window_icon(dark),
                ))));
            super::set_dock_icon(dark);
            std::thread::spawn(move || {
                if let Err(e) = crate::services::macapp::set_icon(&crate::config::user_home(), dark)
                {
                    tracing::warn!("cannot change kimo.app's icon: {e:#}");
                }
            });
        }
        if check_now {
            self.dismissed = None;
            self.local_job(|c| {
                let r: Value = c.get("/update?force=true")?;
                if let Some(e) = r["error"].as_str() {
                    return Ok(Msg::Error(format!("cannot check for updates: {e}")));
                }
                if r["available"] != true && r["restart"] != true {
                    return Ok(Msg::Note(format!(
                        "kimo {} is the latest version",
                        s(&r, "current")
                    )));
                }
                Ok(Msg::Release(r))
            });
        }
        if let Some(value) = new_theme {
            // Only the theme is saved here; other edits wait for "save settings".
            if self.current == 0 {
                self.theme = value;
            }
            self.job(move |c| {
                let mut current: Value = c.get("/settings")?;
                current["theme"] = json!(value);
                c.put("/settings", current)?;
                Ok(Msg::Note("Appearance saved".into()))
            });
        }
        if let Some(body) = body {
            self.job(move |c| {
                Ok(Msg::Settings(serde_json::from_value(
                    c.put("/settings", body)?,
                )?))
            });
            self.toast("Settings saved: new sessions use them", false);
        }
    }

    fn usage_view(&mut self, ui: &mut Ui) {
        use super::chart::{self, Column};
        let r = self.daily.clone();
        let n = |v: &Value| v.as_u64().unwrap_or(0);
        let both = |v: &Value| n(&v["claude"]) + n(&v["codex"]);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.heading(RichText::new("Usage").color(theme::ink()));
            ui.label(RichText::new("Every Claude Code and Codex conversation on this machine, inside kimo or not, from the agents' own logs.").color(theme::muted()));
            ui.add_space(10.0);
            // The range: one row of choices above everything it filters.
            ui.horizontal(|ui| {
                ui.label(RichText::new("Last").size(12.0).color(theme::muted()));
                for days in [7u32, 14, 30, 90] {
                    let selected = self.daily_days == days;
                    let r = ui.add(egui::Button::new(RichText::new(format!("{days} days")).color(if selected { theme::ink() } else { theme::muted() })).frame(false));
                    if selected {
                        ui.painter().hline(r.rect.x_range(), r.rect.bottom() + 1.0, egui::Stroke::new(1.0, theme::ink()));
                    }
                    if r.clicked() && !selected {
                        self.daily_days = days;
                        self.refresh_view();
                    }
                }
                if ui.button("refresh").clicked() {
                    self.refresh_view();
                }
            });
            if r.is_null() {
                ui.add_space(20.0);
                ui.label(RichText::new("Reading the agents' logs…").color(theme::muted()));
                return;
            }
            ui.add_space(14.0);
            // Headline figures.
            ui.horizontal_wrapped(|ui| {
                let tile = |ui: &mut Ui, label: &str, value: String, note: String| -> egui::Response {
                    let r = ui
                        .vertical(|ui| {
                            ui.set_min_width(150.0);
                            ui.label(RichText::new(label).size(11.0).color(theme::muted()));
                            ui.label(RichText::new(value).size(26.0).color(theme::ink()));
                            ui.label(RichText::new(note).size(11.0).color(theme::text()));
                        })
                        .response;
                    ui.add_space(18.0);
                    r
                };
                let split = |v: &Value| format!("claude {} · codex {}", tokens(n(&v["claude"])), tokens(n(&v["codex"])));
                tile(ui, "Today", tokens(both(&r["today"])), split(&r["today"]));
                tile(ui, "Last 7 days", tokens(both(&r["last_7_days"])), split(&r["last_7_days"]));
                tile(ui, &format!("Last {} days", self.daily_days), tokens(both(&r["total"])), split(&r["total"]));
                tile(ui, "Prompts", n(&r["prompts"]).to_string(), format!("in {} conversations", n(&r["conversations"])));
                let f = |v: &Value| v.as_f64().unwrap_or(0.0);
                tile(ui, "Cost at API prices", dollars(f(&r["cost_total"])), format!("today {} · 7 days {}", dollars(f(&r["cost_today"])), dollars(f(&r["cost_7_days"]))))
                    .on_hover_text("What the same tokens cost on the Claude and OpenAI APIs (input, output, cache writes and reads per model). Subscriptions are not billed per token.");
            });

            section(ui, "Tokens per day");
            ui.horizontal(|ui| {
                chart::legend(ui, chart::claude(), "claude");
                chart::legend(ui, chart::codex(), "codex");
                ui.label(RichText::new("new input, output and cache writes; cache reads are not counted").size(11.0).color(theme::muted()));
            });
            let cols: Vec<Column> = r["days"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|d| Column { date: s(d, "date").to_string(), claude: n(&d["tokens"]["claude"]), codex: n(&d["tokens"]["codex"]), prompts: n(&d["prompts"]) })
                .collect();
            chart::daily(ui, &cols, 220.0);
            egui::CollapsingHeader::new(RichText::new("as a table").size(11.0).color(theme::muted())).id_salt("daily-table").show(ui, |ui| {
                egui::Grid::new("daily-grid").num_columns(4).spacing([24.0, 4.0]).show(ui, |ui| {
                    for h in ["day", "claude", "codex", "prompts"] {
                        ui.label(RichText::new(h).size(11.0).color(theme::muted()));
                    }
                    ui.end_row();
                    for c in cols.iter().rev() {
                        ui.label(RichText::new(&c.date).size(12.0).color(theme::text()));
                        ui.label(RichText::new(tokens(c.claude)).size(12.0).color(theme::ink()));
                        ui.label(RichText::new(tokens(c.codex)).size(12.0).color(theme::ink()));
                        ui.label(RichText::new(c.prompts.to_string()).size(12.0).color(theme::ink()));
                        ui.end_row();
                    }
                });
            });

            section(ui, "Models");
            let models = r["models"].as_array().cloned().unwrap_or_default();
            if models.is_empty() {
                ui.label(RichText::new("No answers in these days.").color(theme::muted()));
            }
            egui::Grid::new("models").num_columns(7).spacing([20.0, 6.0]).show(ui, |ui| {
                for h in ["model", "input", "output", "cache writes", "cache reads", "answers", "cost"] {
                    ui.label(RichText::new(h).size(11.0).color(theme::muted()));
                }
                ui.end_row();
                for m in &models {
                    let t = &m["tokens"];
                    ui.label(RichText::new(s(m, "model")).size(12.5).color(theme::ink()));
                    for k in ["input", "output", "cache_write", "cache_read"] {
                        ui.label(RichText::new(tokens(n(&t[k]))).size(12.0).color(theme::text()));
                    }
                    ui.label(RichText::new(n(&m["answers"]).to_string()).size(12.0).color(theme::text()));
                    let cost = m["cost"].as_f64().unwrap_or(0.0);
                    if cost > 0.0 || crate_price_known(s(m, "model")) {
                        ui.label(RichText::new(dollars(cost)).size(12.5).color(theme::ink()));
                    } else {
                        ui.label(RichText::new("price not known").size(11.0).color(theme::muted()));
                    }
                    ui.end_row();
                }
            });
            ui.label(RichText::new("At API list prices per million tokens: new input, output, cache writes (5 minutes or 1 hour) and cache reads.").size(11.0).color(theme::muted()));

            section(ui, "Where");
            let places = r["places"].as_array().cloned().unwrap_or_default();
            if places.is_empty() {
                ui.label(RichText::new("No agent work in these days.").color(theme::muted()));
            }
            let top = places.first().map_or(1, |p| both(&p["tokens"]));
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            egui::Grid::new("places").num_columns(7).spacing([16.0, 8.0]).show(ui, |ui| {
                for h in ["", "", "tokens", "prompts", "conversations", "last used", ""] {
                    ui.label(RichText::new(h).size(11.0).color(theme::muted()));
                }
                ui.end_row();
                for p in &places {
                    let t = &p["tokens"];
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.add(egui::Label::new(RichText::new(s(p, "name")).size(14.0).color(theme::ink())).truncate());
                        ui.add(egui::Label::new(RichText::new(short_path(&home(s(p, "path")), 34)).size(10.5).color(theme::muted())).truncate());
                    });
                    chart::share(ui, n(&t["claude"]), n(&t["codex"]), top, 180.0);
                    ui.label(RichText::new(tokens(both(t))).color(theme::ink()))
                        .on_hover_text(format!("claude {} · codex {}", tokens(n(&t["claude"])), tokens(n(&t["codex"]))));
                    ui.label(RichText::new(n(&p["prompts"]).to_string()).color(theme::text()));
                    ui.label(RichText::new(n(&p["conversations"]).to_string()).color(theme::text()));
                    ui.label(RichText::new(ago(now, n(&p["last_active"]))).size(12.0).color(theme::text()));
                    if p["project"].is_string() {
                        ui.label(RichText::new("project").size(11.0).color(theme::muted()));
                    } else if !s(p, "path").is_empty() && ui.button("add as project").on_hover_text("make this folder a kimo project").clicked() {
                        let path = s(p, "path").to_string();
                        self.action("Project added", move |c| c.post("/projects", json!({ "mode": "existing", "path": path, "resume": false })));
                    }
                    ui.end_row();
                }
            });

            section(ui, "Subscriptions");
            let accounts = r["accounts"].as_array().cloned().unwrap_or_default();
            let outside = both(&r["outside_kimo"]);
            let top = accounts.iter().map(|a| n(&a["tokens"])).chain([outside]).max().unwrap_or(1);
            egui::Grid::new("daily-accounts").num_columns(4).spacing([16.0, 8.0]).show(ui, |ui| {
                for a in &accounts {
                    let claude = s(a, "provider") == "claude";
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.add(egui::Label::new(RichText::new(s(a, "name")).size(14.0).color(theme::ink())).truncate());
                        ui.label(RichText::new(s(a, "provider")).size(10.5).color(theme::muted()));
                    });
                    chart::share(ui, if claude { n(&a["tokens"]) } else { 0 }, if claude { 0 } else { n(&a["tokens"]) }, top, 180.0);
                    ui.label(RichText::new(tokens(n(&a["tokens"]))).color(theme::ink()));
                    ui.label(RichText::new(format!("{} prompts · {}", n(&a["prompts"]), dollars(a["cost"].as_f64().unwrap_or(0.0)))).size(12.0).color(theme::text()));
                    ui.end_row();
                }
                if outside > 0 {
                    ui.vertical(|ui| {
                        ui.set_width(210.0);
                        ui.label(RichText::new("outside kimo").size(14.0).color(theme::text()));
                        ui.label(RichText::new("account not known").size(10.5).color(theme::muted()));
                    })
                    .response
                    .on_hover_text("Conversations not run by kimo (or by sessions since deleted): which account they used is not known.");
                    chart::share(ui, n(&r["outside_kimo"]["claude"]), n(&r["outside_kimo"]["codex"]), top, 180.0);
                    ui.label(RichText::new(tokens(outside)).color(theme::ink()));
                    ui.label("");
                    ui.end_row();
                }
            });
            section(ui, "Plan limits and pace");
            let now_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let with_limits: Vec<&Value> = self.usage.iter().filter(|u| u["limits"]["limits"].as_array().is_some_and(|l| !l.is_empty())).collect();
            if with_limits.is_empty() {
                ui.label(RichText::new("No plan limits known yet: Codex reports them as it works; Claude's show once turned on in Subscriptions.").size(11.5).color(theme::muted()));
            }
            for u in with_limits {
                ui.label(RichText::new(format!("{} · {}", s(u, "name"), s(u, "provider"))).size(13.0).color(theme::ink()));
                limit_rows(ui, Some(u), now_secs);
                ui.add_space(6.0);
            }
            ui.add_space(20.0);
        });
    }

    fn devices_view(&mut self, ui: &mut Ui) {
        ui.heading(RichText::new("Devices").color(theme::ink()));
        ui.label(RichText::new("Use kimo from your phone or any device on your network: scan the QR code, or type the code in the installed app.").color(theme::muted()));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Pair a phone");
            let addresses: Vec<Value> = self.network["addresses"].as_array().cloned().unwrap_or_default();
            let remote: Vec<&Value> = addresses.iter().filter(|a| a["kind"] != "localhost").collect();
            // The QR code shows as soon as there is an address: no extra click.
            let expired = self.pairing.as_ref().is_some_and(|p| p.until <= Instant::now());
            if expired {
                self.pairing = None;
            }
            let asked_lately = self.pairing_asked.is_some_and(|t| t.elapsed() < Duration::from_secs(15));
            if self.pairing.is_none() && !asked_lately && !self.pair_address.is_empty() {
                self.pairing_asked = Some(Instant::now());
                self.job(move |c| {
                    let p = c.post("/pairing", json!({}))?;
                    Ok(Msg::Pairing(s(&p, "code").to_string(), p["expires_in"].as_u64().unwrap_or(600)))
                });
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Address the phone uses").size(12.0).color(theme::muted()));
                // Choosing another address redraws the QR code at once (same one-time code).
                egui::ComboBox::from_id_salt("pair-address").selected_text(&self.pair_address).width(320.0).show_ui(ui, |ui| {
                    for a in &remote {
                        ui.selectable_value(&mut self.pair_address, s(a, "url").to_string(), format!("{} · {}", s(a, "label"), s(a, "url")));
                    }
                });
            });
            if remote.is_empty() && !self.network.is_null() {
                ui.label(RichText::new("kimo is only reachable from this machine: start it with --bind all (or KIMO_BIND=all) to pair phones.").size(11.5).color(theme::warn()));
            }
            if let Some(warning) = self.network["firewall"].as_str() {
                ui.add_space(6.0);
                ui.label(RichText::new(warning).size(11.5).color(theme::bad()));
            }
            let chosen = remote.iter().find(|a| s(a, "url") == self.pair_address).copied();
            if let Some(tip) = chosen.and_then(|a| reach_tip(s(a, "kind"))) {
                ui.add_space(4.0);
                ui.label(RichText::new(tip).size(11.0).color(theme::muted()));
            }
            if let Some(p) = &self.pairing {
                let left = p.until.saturating_duration_since(Instant::now()).as_secs();
                ui.add_space(8.0);
                let url = format!("{}/#pair={}", self.pair_address, p.code);
                let code = p.code.clone();
                let mut fresh = false;
                ui.horizontal(|ui| {
                    qr(ui, &url, 220.0);
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Scan it with the phone's camera.").color(theme::ink()));
                        ui.label(RichText::new(&url).size(11.0).color(theme::teal()));
                        ui.add_space(8.0);
                        ui.label(RichText::new("Or type this code in the installed kimo app:").size(12.0).color(theme::text()));
                        ui.label(RichText::new(&code).size(26.0).monospace().color(theme::ink()));
                        ui.label(RichText::new(format!("valid for {}:{:02}, once", left / 60, left % 60)).size(11.0).color(theme::muted()));
                        ui.add_space(6.0);
                        fresh = ui.button("new code").clicked();
                        ui.add_space(6.0);
                        let https = chosen.is_some_and(|a| a["https"] == true);
                        ui.label(
                            RichText::new(if https {
                                "Install it as an app: open the link on the phone, then Add to Home Screen (iPhone: Share menu; Android: ⋮ menu → Install app). Open the app and type the code."
                            } else {
                                "Install it as an app: iPhone → Share → Add to Home Screen, then open it and type the code. Android installs apps only over HTTPS: choose the Tailscale HTTPS address (enable HTTPS below)."
                            })
                            .size(11.0)
                            .color(theme::muted()),
                        );
                    });
                });
                if fresh {
                    self.pairing = None;
                }
                // Watch for the new device.
                if self.last_projects.elapsed() > Duration::from_secs(2) {
                    self.job(|c| Ok(Msg::Devices(c.get("/devices")?)));
                }
            }
            let ts = &self.network["tailscale"];
            if ts.is_object() && ts["https_url"].is_null() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Phones install kimo as an app only over HTTPS: serve it on {} with Tailscale.", s(ts, "dns_name"))).size(11.0));
                    if ui.button("enable HTTPS").clicked() {
                        self.job(|c| {
                            let r = c.post("/network/https", json!({}))?;
                            Ok(Msg::Toast(format!("HTTPS ready: {}", s(&r, "url"))))
                        });
                    }
                });
            }
            section(ui, "Addresses");
            for a in &addresses {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s(a, "label")).size(11.5).color(theme::muted()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(s(a, "url")));
                });
            }
            section(ui, "Other kimos");
            ui.label(RichText::new("Pair this kimo with the ones on your other machines (lab box, Raspberry Pi, Mac mini): their agents appear here and in `kimo agents` as host/id, and agents can message each other across machines.").size(11.0).color(theme::muted()));
            let mut forget = None;
            for p in &self.peer_info {
                ui.horizontal(|ui| {
                    dot(ui, if p["online"] == true { theme::good() } else { theme::muted() });
                    ui.label(RichText::new(s(p, "name")).color(theme::ink()));
                    ui.label(RichText::new(s(p, "url")).size(11.0).color(theme::muted()));
                    if let Some(n) = p["sessions"].as_u64() {
                        ui.label(RichText::new(format!("{n} sessions")).size(11.0).color(theme::muted()));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(RichText::new("forget").color(theme::bad())).clicked() {
                            forget = Some(s(p, "id").to_string());
                        }
                    });
                });
            }
            if let Some(id) = forget {
                self.local_job(move |c| {
                    c.delete(&format!("/peers/{id}"))?;
                    Ok(Msg::Toast("Host forgotten".into()))
                });
                self.last_hosts = Instant::now() - Duration::from_secs(60);
            }
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.peer_url).hint_text("http://100.x.y.z:7777").desired_width(240.0));
                ui.add(egui::TextEdit::singleline(&mut self.peer_code).hint_text("pairing code (its QR / kimo pair)").desired_width(260.0));
                if primary(ui, "pair").clicked() && !self.peer_url.trim().is_empty() && !self.peer_code.trim().is_empty() {
                    let (url, code) = (self.peer_url.trim().to_string(), self.peer_code.trim().to_string());
                    self.peer_code.clear();
                    self.local_job(move |c| {
                        let p = c.post("/peers", json!({ "url": url, "code": code }))?;
                        Ok(Msg::Toast(format!("Paired with {}", s(&p, "name"))))
                    });
                    self.last_hosts = Instant::now() - Duration::from_secs(60);
                }
            });
            ui.label(RichText::new("The code is the one the other kimo shows in its Devices view (or `kimo pair` there).").size(10.5).color(theme::muted()));
            section(ui, "Paired devices");
            let mut unpair = None;
            for d in &self.devices {
                ui.horizontal(|ui| {
                    dot(ui, theme::good());
                    ui.label(RichText::new(s(d, "name")).color(theme::ink()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button(RichText::new("unpair").color(theme::bad())).clicked() {
                            unpair = Some(s(d, "id").to_string());
                        }
                    });
                });
            }
            if self.devices.is_empty() {
                ui.label(RichText::new("No devices yet.").color(theme::muted()));
            }
            if let Some(id) = unpair {
                self.action("Device unpaired", move |c| c.delete(&format!("/devices/{id}")));
            }
            section(ui, "Apps (your PWAs)");
            for a in &self.apps {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(s(a, "name")).color(theme::ink()));
                    ui.label(RichText::new(s(a, "scope")).size(10.5).color(theme::teal()));
                    let origins: Vec<&str> = a["origins"].as_array().map(|o| o.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                    ui.label(RichText::new(origins.join(", ")).size(11.0).color(theme::muted()));
                });
            }
            ui.label(RichText::new("Create app keys from the web app's Apps tab or POST /api/clients.").size(11.0).color(theme::muted()));
        });
        if self.pairing.is_some() {
            self.ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn project_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = self.project_form.as_mut() else {
            return;
        };
        let mut open = true;
        let mut submit = false;
        let mut browse: Option<String> = None;
        let mut make_folder: Option<(String, String)> = None;
        egui::Window::new("New project")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut form.mode, "new".to_string(), "new folder");
                    ui.radio_value(&mut form.mode, "existing".to_string(), "existing folder");
                    ui.radio_value(&mut form.mode, "clone".to_string(), "clone");
                });
                egui::Grid::new("project-form")
                    .num_columns(2)
                    .spacing([12.0, 8.0])
                    .show(ui, |ui| {
                        if form.mode == "clone" {
                            ui.label("git URL");
                            ui.add(
                                egui::TextEdit::singleline(&mut form.url)
                                    .hint_text("git@github.com:you/app.git")
                                    .desired_width(320.0),
                            );
                            ui.end_row();
                        }
                        ui.label("name");
                        ui.add(
                            egui::TextEdit::singleline(&mut form.name)
                                .hint_text(if form.mode == "new" {
                                    "Booking app"
                                } else {
                                    "default: folder / repo name"
                                })
                                .desired_width(320.0),
                        );
                        ui.end_row();
                        ui.label(if form.mode == "existing" {
                            "folder"
                        } else {
                            "inside"
                        });
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut form.path)
                                    .hint_text("projects folder (~/dev)")
                                    .desired_width(250.0),
                            );
                            if ui.button("browse").clicked() {
                                browse = Some(form.path.clone());
                            }
                        });
                        ui.end_row();
                        if form.mode == "new" {
                            ui.label("");
                            ui.checkbox(&mut form.git, "start a git repository");
                            ui.end_row();
                        }
                    });
                if let Some(listing) = form.listing.clone() {
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.small_button("up").clicked() {
                            browse = listing["parent"].as_str().map(String::from);
                        }
                        ui.label(RichText::new(short_path(s(&listing, "path"), 48)).size(11.0))
                            .on_hover_text(s(&listing, "path"));
                        if ui.small_button("use this folder").clicked() {
                            form.path = s(&listing, "path").to_string();
                            form.listing = None;
                        }
                    });
                    ui.horizontal(|ui| {
                        let edit = ui.add(
                            egui::TextEdit::singleline(&mut form.new_folder)
                                .hint_text("new folder here")
                                .desired_width(200.0),
                        );
                        let enter =
                            edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if (ui.small_button("create folder").clicked() || enter)
                            && !form.new_folder.trim().is_empty()
                        {
                            make_folder = Some((
                                s(&listing, "path").to_string(),
                                std::mem::take(&mut form.new_folder).trim().to_string(),
                            ));
                        }
                    });
                    if let Some(l) = &form.listing {
                        egui::ScrollArea::vertical()
                            .max_height(220.0)
                            .show(ui, |ui| {
                                for d in l["dirs"].as_array().into_iter().flatten() {
                                    let label = if d["git"].as_bool() == Some(true) {
                                        format!("{}   git", s(d, "name"))
                                    } else {
                                        s(d, "name").to_string()
                                    };
                                    if ui.selectable_label(false, label).clicked() {
                                        browse = Some(s(d, "path").to_string());
                                    }
                                }
                            });
                    }
                }
                ui.add_space(6.0);
                submit = primary(ui, "create").clicked();
            });
        if let Some((parent, name)) = make_folder {
            // The new folder opens in the picker, ready to be used.
            self.job(move |c| {
                Ok(Msg::Folder(
                    c.post("/fs", json!({ "parent": parent, "name": name }))?,
                ))
            });
        }
        if let Some(path) = browse {
            self.job(move |c| {
                let q = if path.is_empty() {
                    String::new()
                } else {
                    format!("?path={}", path.replace(' ', "%20"))
                };
                Ok(Msg::Folder(c.get(&format!("/fs{q}"))?))
            });
        }
        if submit {
            let f = self.project_form.take().unwrap_or_default();
            let body = json!({ "mode": f.mode, "name": f.name, "path": if f.path.is_empty() { Value::Null } else { Value::String(f.path) }, "git": f.git, "url": f.url });
            self.job(move |c| Ok(Msg::Added(c.post("/projects", body)?)));
        } else if !open {
            self.project_form = None;
        }
    }

    /// Name, colour and removal of a project.
    fn project_settings_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = self.project_settings.as_mut() else {
            return;
        };
        let mut open = true;
        let (mut save, mut remove) = (false, false);
        egui::Window::new(format!("Project settings · {}", form.name))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .min_width(460.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(home(&form.path))
                        .size(11.0)
                        .color(theme::muted()),
                );
                ui.add_space(8.0);
                ui.label(RichText::new("Display name").color(theme::ink()));
                ui.add(egui::TextEdit::singleline(&mut form.name).desired_width(420.0));
                ui.add_space(8.0);
                ui.label(RichText::new("Colour").color(theme::ink()));
                ui.horizontal(|ui| {
                    for hex in std::iter::once("").chain(PROJECT_COLORS) {
                        let color = parse_color(hex).unwrap_or_else(theme::surface);
                        let (rect, r) =
                            ui.allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                        ui.painter().rect_filled(rect.shrink(3.0), 5.0, color);
                        if hex.is_empty() {
                            ui.painter().line_segment(
                                [rect.shrink(7.0).left_bottom(), rect.shrink(7.0).right_top()],
                                egui::Stroke::new(1.0, theme::muted()),
                            );
                        }
                        if form.color.eq_ignore_ascii_case(hex) {
                            ui.painter().rect_stroke(
                                rect,
                                6.0,
                                egui::Stroke::new(2.0, theme::ink()),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if r.on_hover_text(if hex.is_empty() { "no colour" } else { hex })
                            .clicked()
                        {
                            form.color = hex.to_string();
                        }
                    }
                });
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    save = primary(ui, "save").clicked();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if form.confirm_remove {
                            remove = ui
                                .button(RichText::new("yes, remove it").color(theme::bad()))
                                .clicked();
                            ui.label(
                                RichText::new("Its folder and files stay.")
                                    .size(11.0)
                                    .color(theme::muted()),
                            );
                        } else if ui
                            .button(RichText::new("Remove project").color(theme::bad()))
                            .clicked()
                        {
                            form.confirm_remove = true;
                        }
                    });
                });
            });
        if save {
            let f = self.project_settings.take().expect("open");
            let body = json!({ "name": f.name, "color": f.color });
            let id = f.id;
            self.action("Project saved", move |c| {
                c.patch(&format!("/projects/{id}"), body)
            });
        } else if remove {
            let f = self.project_settings.take().expect("open");
            let id = f.id;
            self.action("Project removed: its folder is untouched", move |c| {
                c.delete(&format!("/projects/{id}"))
            });
        } else if !open {
            self.project_settings = None;
        }
    }

    /// A new worktree: a branch of the project in its own folder, with an agent in it.
    fn worktree_dialog(&mut self, ctx: &egui::Context) {
        let Some(form) = self.worktree_form.as_mut() else {
            return;
        };
        let mut open = true;
        let mut create = false;
        egui::Window::new(format!("New worktree · {}", form.project_name))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new("A branch of this repository in its own folder, so an agent works on it without touching your checkout.").size(11.5).color(theme::text()));
                ui.add_space(8.0);
                ui.label(RichText::new("Branch").color(theme::ink()));
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut form.name)
                        .hint_text("fix-login (an existing branch is checked out)")
                        .desired_width(380.0),
                );
                if form.name.is_empty() {
                    edit.request_focus();
                }
                ui.add_space(8.0);
                ui.label(RichText::new("Start in it").color(theme::ink()));
                ui.horizontal(|ui| {
                    for (value, label) in [("claude", "Claude"), ("codex", "Codex"), ("shell", "shell"), ("", "nothing")] {
                        ui.radio_value(&mut form.agent, value.to_string(), label);
                    }
                });
                ui.add_space(10.0);
                create = primary(ui, "create worktree").clicked()
                    || (edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
            });
        if create && !form.name.trim().is_empty() {
            let f = self.worktree_form.take().expect("open");
            let body = json!({ "name": f.name.trim(), "agent": if f.agent.is_empty() { Value::Null } else { Value::String(f.agent) } });
            let id = f.project;
            self.last_projects = Instant::now() - Duration::from_secs(60);
            self.job(move |c| {
                let made = c.post(&format!("/projects/{id}/worktrees"), body)?;
                Ok(match made["session"].as_object() {
                    Some(_) => Msg::Open(made["session"].clone()),
                    None => Msg::Toast(format!("Worktree ready: {}", s(&made["worktree"], "path"))),
                })
            });
        } else if !open {
            self.worktree_form = None;
        }
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else {
            return;
        };
        let (title, text) = match confirm {
            Confirm::CloseFile { index } => (
                "Close without saving?".to_string(),
                format!(
                    "{} has changes that are not saved; they are lost when it closes.",
                    self.editors.get(*index).map_or("The file", |e| e.path.as_str())
                ),
            ),
            Confirm::RemoveSession { name, .. } => (
                format!("Remove {name}?"),
                "kimo ends it and takes it off the list. A Claude or Codex conversation stays in the agent's history: continue it later from the project's earlier conversations.".to_string(),
            ),
            Confirm::RemoveProject { name, .. } => (
                format!("Remove {name}?"),
                "kimo forgets the project and ends its terminals and agents. Its folder and files stay where they are, and Claude and Codex conversations stay in their history.".to_string(),
            ),
            Confirm::RemoveWorktree { path, .. } => (
                "Remove this worktree?".to_string(),
                format!("git deletes the folder {} (refused if it has uncommitted changes). The branch stays; stopped agents that worked there are removed from kimo.", home(path)),
            ),
            Confirm::RemoveAccount { name, .. } => (
                format!("Remove {name}?"),
                "kimo deletes its sign-in on this computer. Conversations stay; you can add it again and sign in.".to_string(),
            ),
        };
        let yes_label = if matches!(confirm, Confirm::CloseFile { .. }) {
            "close without saving"
        } else {
            "remove"
        };
        let (mut yes, mut no) = (false, false);
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(RichText::new(text).color(theme::text()));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    yes = ui
                        .button(RichText::new(yes_label).color(theme::bad()))
                        .clicked();
                    no = ui.button("cancel").clicked();
                });
            });
        if yes {
            match self.confirm.take() {
                Some(Confirm::CloseFile { index }) => self.close_editor(index),
                Some(Confirm::RemoveSession { id, .. }) => {
                    self.terminals.retain(|t| t.session_id != id);
                    self.active = self.active.min(self.terminals.len().saturating_sub(1));
                    self.action("Session removed", move |c| {
                        c.delete(&format!("/sessions/{id}"))
                    });
                }
                Some(Confirm::RemoveProject { id, .. }) => {
                    self.project = None;
                    // Its terminals end with it: their tabs go now.
                    let ended: Vec<String> = self
                        .sessions()
                        .iter()
                        .filter(|x| self.project_of(x).as_deref() == Some(id.as_str()))
                        .map(|x| s(x, "id").to_string())
                        .collect();
                    self.terminals.retain(|t| !ended.contains(&t.session_id));
                    self.active = self.active.min(self.terminals.len().saturating_sub(1));
                    self.action(
                        "Project removed with its terminals: its folder is untouched",
                        move |c| c.delete(&format!("/projects/{id}")),
                    );
                }
                Some(Confirm::RemoveWorktree { project, path }) => {
                    self.action("Worktree removed", move |c| {
                        c.delete(&format!(
                            "/projects/{project}/worktrees?path={}",
                            query_escape(&path)
                        ))
                    });
                }
                Some(Confirm::RemoveAccount { id, .. }) => {
                    self.action("Subscription removed", move |c| {
                        c.delete(&format!("/accounts/{id}"))
                    });
                }
                None => {}
            }
        } else if no {
            self.confirm = None;
        }
    }

    /// "Update available" card (bottom right), like Orca's.
    /// Shown while memory is low: what uses it, and stopping idle agents in one click.
    fn memory_card(&mut self, ctx: &egui::Context) {
        let m = &self.memory;
        let pressure = s(m, "pressure").to_string();
        if !(pressure == "low" || pressure == "critical")
            || self.memory_dismissed.as_deref() == Some(pressure.as_str())
        {
            return;
        }
        let sessions = self.sessions();
        let mut running: Vec<&Value> = sessions
            .iter()
            .filter(|x| s(x, "status") == "running" || s(x, "status") == "paused")
            .collect();
        running.sort_by_key(|x| std::cmp::Reverse(x["usage"]["rss_bytes"].as_u64().unwrap_or(0)));
        let idle: Vec<String> = m["idle"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        let idle_bytes: u64 = sessions
            .iter()
            .filter(|x| idle.iter().any(|i| i == s(x, "id")))
            .map(|x| x["usage"]["rss_bytes"].as_u64().unwrap_or(0))
            .sum();
        let auto = self
            .settings
            .as_ref()
            .is_none_or(|st| st.memory.free_when_low);
        let (mut close, mut free, mut open) = (false, false, None);
        egui::Area::new(egui::Id::unique("memory-card"))
            .anchor(Align2::LEFT_BOTTOM, [12.0, -40.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(1.0, if pressure == "critical" { theme::bad() } else { theme::line() }))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(16))
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(236.0);
                        ui.horizontal(|ui| {
                            let title = if pressure == "critical" { "Memory is almost full" } else { "Memory is low" };
                            ui.label(RichText::new(title).size(15.0).strong().color(if pressure == "critical" { theme::bad() } else { theme::ink() }));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                close = ui.add(egui::Button::new(RichText::new("×").size(16.0).color(theme::muted())).frame(false)).on_hover_text("hide until it changes").clicked();
                            });
                        });
                        ui.label(
                            RichText::new(format!(
                                "{} free of {}; agents use {}.",
                                mb(m["available_bytes"].as_u64().unwrap_or(0)),
                                mb(m["total_bytes"].as_u64().unwrap_or(0)),
                                mb(m["agents_bytes"].as_u64().unwrap_or(0))
                            ))
                            .size(12.5)
                            .color(theme::text()),
                        );
                        ui.add_space(6.0);
                        for x in running.iter().take(4) {
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new(RichText::new(s(x, "name")).color(theme::ink())).frame(false).truncate()).clicked() {
                                    open = Some(s(x, "id").to_string());
                                }
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    ui.label(RichText::new(mb(x["usage"]["rss_bytes"].as_u64().unwrap_or(0))).size(11.5).color(theme::muted()));
                                    if idle.iter().any(|i| i == s(x, "id")) {
                                        ui.label(RichText::new("idle").size(10.5).color(theme::muted()));
                                    }
                                });
                            });
                        }
                        ui.add_space(8.0);
                        if idle.is_empty() {
                            ui.label(RichText::new("No Claude or Codex agent is idle right now. Stop one you do not need (pause keeps its memory).").size(11.5).color(theme::muted()));
                        } else {
                            let label = format!("Stop {} idle agent{} (frees {})", idle.len(), if idle.len() == 1 { "" } else { "s" }, mb(idle_bytes));
                            free = primary(ui, &label).clicked();
                            ui.label(RichText::new("They continue their conversation when you open them.").size(11.0).color(theme::muted()));
                        }
                        if auto {
                            ui.label(RichText::new("When memory is almost full, kimo stops idle agents by itself (Settings → Memory).").size(10.5).color(theme::muted()));
                        }
                    });
            });
        if close {
            self.memory_dismissed = Some(pressure);
        }
        if free {
            self.job(|c| {
                let r = c.post("/memory/free", json!({}))?;
                let n = r["stopped"].as_array().map_or(0, |a| a.len());
                Ok(Msg::Toast(format!(
                    "Stopped {n} idle agent{}: memory freed",
                    if n == 1 { "" } else { "s" }
                )))
            });
            self.last_status = Instant::now() - Duration::from_secs(60);
        }
        if let Some(id) = open {
            self.open_session(&id);
        }
    }

    /// The update to offer, if any: this machine's server behind this window, a newer kimo
    /// installed but not running yet, or a new release.
    fn pending_update(&self) -> Option<Pending> {
        let own = env!("CARGO_PKG_VERSION");
        let r = &self.release;
        if let Some(v) = self.server_version.as_deref().filter(|v| is_newer(own, v)) {
            return Some(Pending {
                key: format!("behind:{v}"),
                title: "Restart kimo",
                text: format!("kimo {own} is installed, but the kimo server still runs {v}."),
                offer: Offer::Restart,
            });
        }
        if r["restart"] == true {
            let installed = s(r, "installed");
            return Some(Pending {
                key: format!("restart:{installed}"),
                title: "Restart to update",
                text: format!("kimo {installed} is installed."),
                offer: Offer::Restart,
            });
        }
        let latest = r["latest"].as_str().filter(|_| r["available"] == true)?;
        Some(Pending {
            key: format!("update:{latest}"),
            title: "Update available",
            text: format!("kimo {latest} is ready."),
            offer: Offer::Install,
        })
    }

    /// Bounce the Dock icon (or flash the taskbar) once per new update.
    fn ask_attention(&mut self) {
        let Some(p) = self.pending_update() else {
            return;
        };
        if self.attention_for.as_deref() != Some(p.key.as_str()) {
            self.attention_for = Some(p.key);
            self.ctx
                .send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Informational,
                ));
        }
    }

    /// This machine's server answered with its version: when it runs a newer kimo (updated
    /// from a phone, by itself or by the install script) the window restarts on it too, once
    /// its own binary is that new.
    fn follow_server(&mut self, version: String) {
        if version.is_empty() {
            return;
        }
        let own = env!("CARGO_PKG_VERSION");
        if is_newer(&version, own) && self.followed.as_deref() != Some(version.as_str()) {
            self.followed = Some(version.clone());
            let on_disk = super::EXE
                .get()
                .and_then(|exe| crate::services::update::version_of(exe));
            if on_disk.is_some_and(|d| is_newer(&d, own)) {
                restart_window();
            }
        }
        self.server_version = Some(version);
        self.ask_attention();
    }

    /// "Update available" card (bottom right), like Orca's: one click updates or restarts.
    fn update_card(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_update() else {
            return;
        };
        if self.dismissed.as_deref() == Some(pending.key.as_str()) {
            return;
        }
        let r = &self.release;
        let manual = r["manual"]
            .as_str()
            .filter(|_| pending.offer == Offer::Install)
            .map(String::from);
        let url = r["url"]
            .as_str()
            .filter(|_| pending.key.starts_with("update:"))
            .map(String::from);
        let auto = (self.current == 0)
            .then(|| self.settings.as_ref().map(|st| st.updates.auto))
            .flatten();
        let (mut close, mut go, mut set_auto) = (false, false, None);
        egui::Area::new(egui::Id::unique("update-card"))
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface())
                    .stroke(egui::Stroke::new(1.0, theme::line()))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(16))
                    .shadow(ctx.global_style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(pending.title)
                                    .size(15.0)
                                    .strong()
                                    .color(theme::ink()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    close = ui
                                        .add(
                                            egui::Button::new(
                                                RichText::new("×").size(16.0).color(theme::muted()),
                                            )
                                            .frame(false),
                                        )
                                        .on_hover_text("later (Update stays in the top bar)")
                                        .clicked();
                                },
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(RichText::new(&pending.text).size(13.5).color(theme::text()));
                        ui.label(
                            RichText::new("Running agents keep running through the update.")
                                .size(11.5)
                                .color(theme::muted()),
                        );
                        if let Some(url) = &url {
                            if ui
                                .link(RichText::new("Release notes").size(12.0).underline())
                                .clicked()
                            {
                                open_url(url);
                            }
                        }
                        ui.add_space(8.0);
                        match (&manual, &self.updating) {
                            (Some(reason), _) => {
                                ui.label(RichText::new(reason).size(11.5).color(theme::warn()));
                            }
                            (None, Updating::Busy(step)) => {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(RichText::new(*step).color(theme::text()));
                                });
                            }
                            (None, Updating::Idle) => {
                                let label = match pending.offer {
                                    Offer::Install => "Update",
                                    Offer::Restart => "Restart",
                                };
                                let (rect, r) = ui.allocate_exact_size(
                                    egui::vec2(300.0, 34.0),
                                    egui::Sense::click(),
                                );
                                let fill = if r.hovered() {
                                    theme::ink().gamma_multiply(0.85)
                                } else {
                                    theme::ink()
                                };
                                ui.painter().rect_filled(rect, 6.0, fill);
                                ui.painter().text(
                                    rect.center(),
                                    Align2::CENTER_CENTER,
                                    label,
                                    egui::FontId::proportional(13.5),
                                    theme::background(),
                                );
                                go = r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                            }
                        }
                        if let Some(mut on) = auto.filter(|_| manual.is_none()) {
                            ui.add_space(6.0);
                            if ui
                                .checkbox(
                                    &mut on,
                                    RichText::new("from now on, update by itself").size(11.5),
                                )
                                .on_hover_text("When no agent is working (Settings → Updates)")
                                .changed()
                            {
                                set_auto = Some(on);
                            }
                        }
                    });
            });
        if close {
            self.dismissed = Some(pending.key);
        }
        if let Some(on) = set_auto {
            if let Some(st) = self.settings.as_mut() {
                st.updates.auto = on;
            }
            self.local_job(move |c| {
                let mut current: Value = c.get("/settings")?;
                current["updates"]["auto"] = json!(on);
                c.put("/settings", current)?;
                Ok(Msg::Note(if on {
                    "kimo will update by itself when no agent is working".into()
                } else {
                    "kimo will ask before updating".into()
                }))
            });
        }
        if go {
            self.start_update(pending.offer);
        }
        if self.updating != Updating::Idle {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    /// Install the new release (or just restart onto the installed one), restart this
    /// machine's server and then the window.
    fn start_update(&mut self, offer: Offer) {
        self.updating = Updating::Busy(match offer {
            Offer::Install => "Downloading…",
            Offer::Restart => "Restarting…",
        });
        let (client, tx, ctx) = (
            self.hosts[0].client.clone(),
            self.tx.clone(),
            self.ctx.clone(),
        );
        let data_dir = self
            .prefs
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        std::thread::spawn(move || {
            let done = (|| {
                if offer == Offer::Install {
                    // Downloading takes a while on a slow line.
                    client.call_timeout("POST", "/update", Some(&json!({})), 900)?;
                }
                restart_server(&client, &data_dir)
            })();
            let msg = match done {
                Ok(version) => Msg::ServerRestarted(version),
                Err(e) => Msg::UpdateFailed(format!("Update failed: {e}")),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Earlier conversations of a project, each one continuable.
    fn history_dialog(&mut self, ctx: &egui::Context) {
        let Some((name, folder, list)) = self.history.clone() else {
            return;
        };
        let mut open = true;
        let mut action: Option<(Value, bool)> = None;
        egui::Window::new(format!("Earlier conversations · {name}"))
            .open(&mut open)
            .collapsible(false)
            .default_width(640.0)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if list.is_empty() {
                    ui.label(
                        RichText::new("No Claude or Codex conversations in this folder yet.")
                            .color(theme::muted()),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for c in &list {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(s(c, "agent")).size(10.5).color(theme::teal()),
                                );
                                ui.label(RichText::new(s(c, "title")).color(theme::ink()));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let attached = c["session"].as_str().is_some();
                                        if ui
                                            .button(if attached { "open" } else { "continue" })
                                            .clicked()
                                        {
                                            action = Some((c.clone(), attached));
                                        }
                                        let ago = {
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map_or(0, |d| d.as_millis() as u64);
                                            let m = now.saturating_sub(
                                                c["updated_at"].as_u64().unwrap_or(0),
                                            ) / 60_000;
                                            if m < 60 {
                                                format!("{m} min ago")
                                            } else if m < 1440 {
                                                format!("{} h ago", m / 60)
                                            } else {
                                                format!("{} d ago", m / 1440)
                                            }
                                        };
                                        ui.label(
                                            RichText::new(format!(
                                                "{ago} · {} asks",
                                                c["messages"]
                                            ))
                                            .size(10.5)
                                            .color(theme::muted()),
                                        );
                                    },
                                );
                            });
                            ui.separator();
                        }
                    });
            });
        if let Some((c, attached)) = action {
            self.history = None;
            if attached {
                let id = s(&c, "session").to_string();
                self.open_session(&id);
            } else {
                let body = json!({ "agent": c["agent"], "cwd": folder, "id": c["id"], "name": short_title(s(&c, "title")), "start": true });
                self.job(move |cl| Ok(Msg::Open(cl.post("/conversations/continue", body)?)));
            }
        } else if !open {
            self.history = None;
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        self.toasts
            .retain(|t| t.2.elapsed() < Duration::from_secs(4));
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(Align2::RIGHT_BOTTOM, [-16.0, -40.0])
            .show(ctx, |ui| {
                for (text, error, _) in &self.toasts {
                    egui::Frame::new()
                        .fill(if *error { theme::bad() } else { theme::ink() })
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.label(RichText::new(text).color(theme::background()));
                        });
                    ui.add_space(4.0);
                }
            });
        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `5m`, `3h`, `2d` (the sidebar's "last output" column).
fn compact_ago(now_ms: u64, at_ms: u64) -> String {
    let min = now_ms.saturating_sub(at_ms) / 60_000;
    match min {
        0 => "now".into(),
        1..=59 => format!("{min}m"),
        60..=2879 => format!("{}h", min / 60),
        _ => format!("{}d", min / 1440),
    }
}

/// Percent-encode a value for a query string.
fn query_escape(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The pointer in window points. While files are dragged in, winit reports no pointer moves,
/// so on macOS kimo asks the system where the pointer is.
fn drop_pointer(ctx: &egui::Context) -> Option<egui::Pos2> {
    #[cfg(target_os = "macos")]
    if let Some(p) = macos_pointer(ctx) {
        return Some(p);
    }
    ctx.input(|i| i.pointer.latest_pos())
}

#[cfg(target_os = "macos")]
fn macos_pointer(ctx: &egui::Context) -> Option<egui::Pos2> {
    use std::ffi::c_void;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *const c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }
    // SAFETY: an empty event made only to read the pointer location (global coordinates,
    // top-left origin, in points), released right after.
    let at = unsafe {
        let event = CGEventCreate(std::ptr::null());
        if event.is_null() {
            return None;
        }
        let at = CGEventGetLocation(event);
        CFRelease(event);
        at
    };
    let inner = ctx.input(|i| i.viewport().inner_rect)?;
    Some(egui::pos2(
        at.x as f32 - inner.min.x,
        at.y as f32 - inner.min.y,
    ))
}

/// Paths as a shell reads them: plain when safe, single-quoted otherwise.
pub fn shell_quote(path: &str) -> String {
    let safe = !path.is_empty()
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+,:@%=~".contains(&b));
    if safe {
        path.to_string()
    } else {
        format!("'{}'", path.replace('\'', "'\\''"))
    }
}

/// The worktrees the server lists for a project (the project's folder alone when none).
fn worktrees_of(p: &Value) -> Vec<Worktree> {
    let list: Vec<Worktree> = p["worktrees"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|w| Worktree {
            path: s(w, "path").to_string(),
            branch: w["branch"].as_str().map(String::from),
            primary: w["primary"] == true,
        })
        .collect();
    if list.is_empty() {
        vec![Worktree {
            path: s(p, "path").to_string(),
            branch: p["branch"].as_str().map(String::from),
            primary: true,
        }]
    } else {
        list
    }
}

fn parse_color(hex: &str) -> Option<Color32> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(hex, 16).ok()?;
    Some(Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The project's mark: a small square in its colour with its initial.
fn project_mark(ui: &mut Ui, p: &Value) {
    let color = parse_color(s(p, "color")).unwrap_or_else(theme::muted);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, color);
    let initial: String = s(p, "name")
        .chars()
        .take(1)
        .collect::<String>()
        .to_uppercase();
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial,
        egui::FontId::proportional(11.0),
        Color32::WHITE,
    );
}

/// A chevron button: down when open, right when folded.
fn chevron(ui: &mut Ui, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
    let color = if response.hovered() {
        theme::ink()
    } else {
        theme::muted()
    };
    let c = rect.center();
    let points = if open {
        vec![
            c + egui::vec2(-4.0, -2.0),
            c + egui::vec2(0.0, 2.0),
            c + egui::vec2(4.0, -2.0),
        ]
    } else {
        vec![
            c + egui::vec2(-2.0, -4.0),
            c + egui::vec2(2.0, 0.0),
            c + egui::vec2(-2.0, 4.0),
        ]
    };
    ui.painter()
        .add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
    response
}

/// The button that shows or hides the projects panel: a window outline with a side bar.
fn panel_button(ui: &mut Ui, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let color = if response.hovered() {
        theme::ink()
    } else {
        theme::text()
    };
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme::surface());
    }
    let r = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 14.0));
    let stroke = egui::Stroke::new(1.5, color);
    ui.painter()
        .rect_stroke(r, 2.5, stroke, egui::StrokeKind::Inside);
    let bar = egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 6.0, r.max.y));
    if open {
        ui.painter().rect_filled(bar, 2.0, color);
    } else {
        ui.painter()
            .vline(bar.max.x, r.y_range(), egui::Stroke::new(1.5, color));
    }
    response
}

/// The right panel's button: a frame with its right part filled when the panel is open.
fn details_button(ui: &mut Ui, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::click());
    let color = if response.hovered() {
        theme::ink()
    } else {
        theme::text()
    };
    if response.hovered() {
        ui.painter().rect_filled(rect, 6.0, theme::surface());
    }
    let r = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 14.0));
    ui.painter().rect_stroke(
        r,
        2.5,
        egui::Stroke::new(1.5, color),
        egui::StrokeKind::Inside,
    );
    let bar = egui::Rect::from_min_max(egui::pos2(r.max.x - 6.0, r.min.y), r.max);
    if open {
        ui.painter().rect_filled(bar, 2.0, color);
    } else {
        ui.painter()
            .vline(bar.min.x, r.y_range(), egui::Stroke::new(1.5, color));
    }
    response
}

/// A menu behind a frameless button (the "···" of projects and worktrees).
fn flat_menu(ui: &mut Ui, label: RichText, add: impl FnOnce(&mut Ui)) {
    egui::containers::menu::MenuButton::from_button(egui::Button::new(label).frame(false))
        .ui(ui, add);
}

/// A small text button without a frame (actions inside a worktree card).
fn link_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(11.5).color(theme::text())).frame(false))
}

/// A small outlined tag ("primary").
fn badge(ui: &mut Ui, text: &str) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, theme::line()))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(5, 0))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(10.0).color(theme::muted()));
        });
}

/// Plan limits of a subscription: a bar per window, when it resets and where the pace leads.
/// A session's right-click menu: open, stop (running ones), remove.
fn session_menu(r: &egui::Response, x: &Value, act: &mut Option<SessionAct>) {
    r.context_menu(|ui| {
        let id = s(x, "id").to_string();
        if ui.button("Open").clicked() {
            *act = Some(SessionAct::Open(id.clone()));
        }
        if matches!(s(x, "status"), "running" | "paused")
            && ui.button("Stop (free its memory)").clicked()
        {
            *act = Some(SessionAct::Stop(id.clone()));
        }
        if ui
            .button(RichText::new("Remove…").color(theme::bad()))
            .clicked()
        {
            *act = Some(SessionAct::Remove {
                id,
                name: s(x, "name").to_string(),
            });
        }
    });
}

/// A pasted image as PNG (the format agents read).
fn encode_png(image: &egui::ColorImage) -> Result<Vec<u8>> {
    let [w, h] = image.size;
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|p| p.to_srgba_unmultiplied())
        .collect();
    writer.write_image_data(&bytes)?;
    writer.finish()?;
    Ok(out)
}

/// A bottom-bar button: small text, no frame.
fn bar_button(ui: &mut Ui, text: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(text).size(11.5).color(theme::text())).frame(false))
}

/// The usage entry of an account (`system`: this computer's own login of the provider).
fn usage_of<'a>(usage: &'a [Value], account: &Value) -> Option<&'a Value> {
    usage.iter().find(|u| {
        s(u, "provider") == s(account, "provider")
            && if account["system"] == true {
                u["account"].is_null()
            } else {
                s(u, "account") == s(account, "id")
            }
    })
}

/// Every plan limit of a login, short, as in the bottom bar: "4% 4h 29m · 41% 3d 7h · 15% Fable"
/// (the 5-hour and weekly ones with their reset, a model's own by its name).
fn limits_short(usage: &Value, now: u64) -> String {
    usage["limits"]["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|l| {
            let used = l["used_percent"].as_f64().unwrap_or(0.0);
            let name = s(l, "name");
            match name.split_once(' ') {
                Some((_, model)) => format!("{used:.0}% {model}"),
                None => format!(
                    "{used:.0}% {}",
                    until(l["resets_at"].as_u64().unwrap_or(0), now)
                ),
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// One listening port; true when clicked (to open it in the browser).
fn port_row(ui: &mut Ui, p: &Value) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        clicked = ui
            .add(
                egui::Button::new(
                    RichText::new(p["port"].to_string())
                        .strong()
                        .color(theme::ink()),
                )
                .frame(false),
            )
            .on_hover_text("open in the browser")
            .clicked();
        let addresses: Vec<&str> = p["addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|a| a.as_str())
            .collect();
        ui.label(
            RichText::new(p["process"].as_str().unwrap_or("unknown process"))
                .size(11.5)
                .color(theme::text()),
        );
        ui.label(
            RichText::new(addresses.join(" "))
                .size(10.5)
                .color(theme::muted()),
        );
    });
    clicked
}

/// A small line of recent CPU use.
fn sparkline(ui: &mut Ui, history: Option<&std::collections::VecDeque<f32>>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(70.0, 14.0), egui::Sense::hover());
    let Some(h) = history.filter(|h| h.len() > 1) else {
        return;
    };
    let top = h.iter().cloned().fold(1.0f32, f32::max);
    let n = h.len() as f32 - 1.0;
    let points: Vec<egui::Pos2> = h
        .iter()
        .enumerate()
        .map(|(i, v)| {
            egui::pos2(
                rect.left() + rect.width() * i as f32 / n,
                rect.bottom() - rect.height() * v / top,
            )
        })
        .collect();
    ui.painter().add(egui::Shape::line(
        points,
        egui::Stroke::new(1.0, theme::muted()),
    ));
}

fn limit_rows(ui: &mut Ui, usage: Option<&Value>, now: u64) {
    let Some(u) = usage else {
        return;
    };
    let limits = u["limits"]["limits"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if limits.is_empty() {
        let note = u["limits_note"]
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| usage_line(u));
        ui.label(RichText::new(note).size(11.0).color(theme::muted()));
        return;
    }
    for l in &limits {
        let used = l["used_percent"].as_f64().unwrap_or(0.0);
        let resets = l["resets_at"].as_u64().unwrap_or(0);
        let pace = &l["pace"];
        let out_at = pace["runs_out_at"].as_u64();
        let color = if used >= 100.0 {
            theme::bad()
        } else if out_at.is_some() {
            theme::warn()
        } else {
            theme::teal()
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(s(l, "name")).size(11.5).color(theme::text()));
            let (rect, _) = ui.allocate_exact_size(egui::vec2(140.0, 8.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 4.0, theme::line());
            let fill = egui::Rect::from_min_size(
                rect.min,
                egui::vec2(
                    rect.width() * (used / 100.0).clamp(0.0, 1.0) as f32,
                    rect.height(),
                ),
            );
            ui.painter().rect_filled(fill, 4.0, color);
            ui.label(
                RichText::new(format!("{used:.0}%"))
                    .size(11.5)
                    .color(theme::ink()),
            );
            let mut text = vec![];
            if resets > 0 {
                text.push(format!("resets in {}", until(resets, now)));
            }
            match (out_at, pace["projected_percent"].as_f64()) {
                _ if used >= 100.0 => text.push("used up".into()),
                (Some(at), _) => text.push(format!(
                    "at this pace runs out in {} ({})",
                    until(at, now),
                    clock(at)
                )),
                (None, Some(p)) => text.push(format!("at this pace about {p:.0}% by then")),
                _ => {}
            }
            ui.label(
                RichText::new(text.join(" · "))
                    .size(11.0)
                    .color(if out_at.is_some() {
                        theme::warn()
                    } else {
                        theme::muted()
                    }),
            );
        });
    }
}

/// `$12.30`, `$0.42`, `$1,204`.
fn dollars(v: f64) -> String {
    if v >= 1000.0 {
        let whole = v.round() as u64;
        let s = whole.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().enumerate() {
            if i > 0 && (s.len() - i).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        format!("${out}")
    } else {
        format!("${v:.2}")
    }
}

fn crate_price_known(model: &str) -> bool {
    crate::core::pricing::price_of(model).is_some()
}

/// `14:35` (local time) of a unix-seconds time.
fn clock(unix_secs: u64) -> String {
    let local = unix_secs as i64 + local_offset_secs();
    format!(
        "{:02}:{:02}",
        local.rem_euclid(86_400) / 3600,
        local.rem_euclid(3600) / 60
    )
}

/// This machine's offset from UTC in seconds (`date +%z`), looked up once.
fn local_offset_secs() -> i64 {
    static OFFSET: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFFSET.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| {
                let t = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let sign = if t.starts_with('-') { -1 } else { 1 };
                let digits = t.trim_start_matches(['+', '-']);
                let h: i64 = digits.get(..2)?.parse().ok()?;
                let m: i64 = digits.get(2..4)?.parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

/// The terminal that signs an account in (`claude auth login`, `codex login`).
fn is_login_session(session: &Value) -> bool {
    let words: Vec<&str> = session["command"]
        .as_array()
        .map(|c| c.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    !session["account"].is_null()
        && (words == ["claude", "auth", "login"] || words == ["codex", "login"])
}

fn kind_label(kind: &str) -> &str {
    match kind {
        "claude" => "CC",
        "codex" => "CX",
        _ => "SH",
    }
}

fn reveal_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Show in Finder"
    } else {
        "Open folder"
    }
}

/// Show a folder in the system's file manager.
fn reveal_folder(path: &str) {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(opener)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

/// Open a web page in the browser.
fn open_url(url: &str) {
    reveal_folder(url);
}

/// Restart this machine's kimo server on the kimo installed now and wait until it answers
/// again; running agents continue their conversations. Returns its version.
fn restart_server(client: &Client, data_dir: &Path) -> Result<String> {
    let version = |c: &Client| -> Option<String> {
        let st: Value = c.call_timeout("GET", "/status", None, 2).ok()?;
        Some(s(&st, "version").to_string())
    };
    let before = version(client).unwrap_or_default();
    let (status, _, _) = client.raw(
        "POST",
        "/update/restart",
        b"{}".to_vec(),
        Some("application/json"),
        10,
    )?;
    if status == 404 {
        // Before 0.1.2 kimo could not restart itself.
        return replace_old_server(client, data_dir);
    }
    // The old server stops its agents and execs the new binary: wait for it to go and return.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut went_away = false;
    while Instant::now() < deadline {
        match version(client) {
            None => went_away = true,
            Some(v) if went_away || v != before => return Ok(v),
            Some(_) => {}
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    anyhow::bail!("the new kimo server did not come back: see ~/.kimo/kimo.log")
}

/// A kimo server too old to restart itself: stop it (its agents stop) and start this kimo's
/// server, then start again the agents that were running.
fn replace_old_server(client: &Client, data_dir: &Path) -> Result<String> {
    let port: u16 = client
        .base()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .ok_or_else(|| anyhow::anyhow!("cannot tell the server's port"))?;
    let sessions: Vec<Value> = client.get("/sessions")?;
    let running: Vec<String> = sessions
        .iter()
        .filter(|x| matches!(s(x, "status"), "running" | "paused"))
        .map(|x| s(x, "id").to_string())
        .collect();
    let out = std::process::Command::new("lsof")
        .args(["-nP", "-t", &format!("-iTCP:{port}"), "-sTCP:LISTEN"])
        .output()?;
    let pids: Vec<i32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    let is_kimo = |pid: i32| {
        std::process::Command::new("ps")
            .args(["-o", "comm=", "-p", &pid.to_string()])
            .output()
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim().ends_with("kimo"))
    };
    let pid = pids
        .into_iter()
        .find(|p| is_kimo(*p))
        .ok_or_else(|| anyhow::anyhow!("cannot find the kimo server on port {port}"))?;
    // SAFETY: a plain signal to a process we just identified as the kimo server.
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while client.call_timeout("GET", "/status", None, 1).is_ok() {
        if Instant::now() > deadline {
            anyhow::bail!("the old kimo server did not stop (pid {pid})");
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    super::start_server(port, data_dir)?;
    let deadline = Instant::now() + Duration::from_secs(20);
    let st: Value = loop {
        if let Ok(st) = client.call_timeout("GET", "/status", None, 2) {
            break st;
        }
        if Instant::now() > deadline {
            anyhow::bail!("the new kimo server did not start: see ~/.kimo/kimo.log");
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    for id in running {
        if let Err(e) = client.post(
            &format!("/sessions/{id}/action"),
            json!({ "action": "start" }),
        ) {
            tracing::warn!("cannot start {id} again: {e:#}");
        }
    }
    Ok(s(&st, "version").to_string())
}

/// Replace this window with the newly installed kimo (same arguments).
fn restart_window() {
    use std::os::unix::process::CommandExt;
    if let Some(exe) = super::EXE.get() {
        let err = std::process::Command::new(exe)
            .args(std::env::args_os().skip(1))
            .exec();
        tracing::error!("cannot restart the window: {err}");
    }
}

/// A session name from a conversation title (short enough for tabs and the sidebar).
fn short_title(title: &str) -> String {
    let t: String = title.chars().take(32).collect();
    if t.chars().count() < title.chars().count() {
        format!("{}…", t.trim_end())
    } else {
        t
    }
}

/// "5 min ago", "3 h ago", "2 d ago".
fn ago(now_ms: u64, at_ms: u64) -> String {
    if at_ms == 0 {
        return String::new();
    }
    let min = now_ms.saturating_sub(at_ms) / 60_000;
    match min {
        0 => "just now".into(),
        1..=59 => format!("{min} min ago"),
        60..=2879 => format!("{} h ago", min / 60),
        _ => format!("{} d ago", min / 1440),
    }
}

/// "needs you" / "working" next to a session name.
fn activity_tag(ui: &mut Ui, session: &Value) {
    match s(session, "activity") {
        "waiting" => {
            ui.label(RichText::new("needs you").size(10.5).color(theme::accent()));
        }
        "working" => {
            ui.label(RichText::new("working").size(10.5).color(theme::teal()));
        }
        _ => {
            let freed = &session["freed"];
            if freed.is_object() && s(session, "status") == "stopped" {
                let why = match s(freed, "reason") {
                    "idle" => "because it was idle for a while",
                    "asked" => "when you asked",
                    _ => "because memory was low",
                };
                ui.label(RichText::new("asleep").size(10.5).color(theme::muted())).on_hover_text(format!(
                    "kimo stopped it to free {} {why}; it continues its conversation when you open it",
                    mb(freed["bytes"].as_u64().unwrap_or(0))
                ));
            }
        }
    }
}

/// What the phone needs to reach kimo over this kind of address.
fn reach_tip(kind: &str) -> Option<&'static str> {
    match kind {
        "zerotier" => Some("ZeroTier: the phone needs the ZeroTier One app joined to the same network (network id from my.zerotier.com), the phone authorized there (Auth ✓), and the app's switch on. Test first: open the address in the phone's browser."),
        "tailscale" => Some("Tailscale: the phone needs the Tailscale app signed in to the same tailnet and connected."),
        "lan" => Some("Local network: the phone must be on the same Wi-Fi (not guest Wi-Fi or mobile data)."),
        _ => None,
    }
}

/// Paint a QR code.
fn qr(ui: &mut Ui, text: &str, size: f32) {
    let Ok(code) = qrcode::QrCode::new(text.as_bytes()) else {
        return;
    };
    let width = code.width();
    let colors = code.to_colors();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    // Always dark on light: phone cameras do not read inverted codes.
    painter.rect_filled(rect, 0.0, theme::LIGHT.background);
    let quiet = 2.0;
    let cell = size / (width as f32 + quiet * 2.0);
    for (i, c) in colors.iter().enumerate() {
        if *c == qrcode::Color::Dark {
            let (x, y) = ((i % width) as f32 + quiet, (i / width) as f32 + quiet);
            painter.rect_filled(
                egui::Rect::from_min_size(
                    rect.min + egui::vec2(x * cell, y * cell),
                    egui::vec2(cell + 0.5, cell + 0.5),
                ),
                0.0,
                theme::LIGHT.ink,
            );
        }
    }
}

impl eframe::App for KimoApp {
    /// Behind the panels: the projects panel's colour, so a panel content wider than its
    /// width never leaves a dark strip next to it.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::surface().to_normalized_gamma_f32()
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        self.poll();
        let ctx = ui.ctx().clone();
        let dark = match self.theme {
            Theme::Dark => true,
            Theme::Light => false,
            Theme::System => ctx.system_theme() == Some(egui::Theme::Dark),
        };
        theme::set_dark(&ctx, dark);
        egui::Panel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(theme::background())
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| self.top_bar(ui));
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme::surface())
                    .inner_margin(egui::Margin::symmetric(16, 5)),
            )
            .show(ui, |ui| self.status_bar(ui));
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::B)) {
            self.toggle_sidebar();
        }
        self.sidebar_rect = None;
        if self.sidebar_open {
            // Narrower, its content would not fit; wider, there is no room left to work.
            let widest = (ctx.content_rect().width() * 0.5).clamp(260.0, 560.0);
            let shown = egui::Panel::left("projects")
                .resizable(true)
                .default_size(280.0)
                .size_range(250.0..=widest)
                .frame(
                    egui::Frame::new()
                        .fill(theme::surface())
                        .inner_margin(egui::Margin::same(14)),
                )
                .show(ui, |ui| self.sidebar(ui));
            self.sidebar_rect = Some(shown.response.rect);
        }
        if ctx.input_mut(|i| {
            i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::E,
            )
        }) {
            self.toggle_details();
        }
        if self.details.open {
            let widest = (ctx.content_rect().width() * 0.45).clamp(280.0, 640.0);
            egui::Panel::right("details")
                .resizable(true)
                .default_size(320.0)
                .size_range(260.0..=widest)
                .frame(
                    egui::Frame::new()
                        .fill(theme::surface())
                        .inner_margin(egui::Margin::same(12)),
                )
                .show(ui, |ui| self.details_ui(ui));
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(theme::background())
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| match self.view {
                View::Terminals => self.terminals_view(ui),
                View::Usage => self.usage_view(ui),
                View::Subscriptions => self.subscriptions_view(ui),
                View::Skills => self.skills_view(ui),
                View::Settings => self.settings_view(ui),
                View::Devices => self.devices_view(ui),
            });
        self.project_dialog(&ctx);
        self.history_dialog(&ctx);
        self.project_settings_dialog(&ctx);
        self.worktree_dialog(&ctx);
        self.confirm_dialog(&ctx);
        self.switch_dialog(&ctx);
        self.update_card(&ctx);
        self.memory_card(&ctx);
        self.handle_drops(ui);
        self.toasts(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every text the frame painted (a screenshot in words).
    fn texts(out: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => into.push(t.galley.text().to_string()),
                egui::Shape::Vec(list) => list.iter().for_each(|s| walk(s, into)),
                _ => {}
            }
        }
        let mut list = vec![];
        for c in &out.shapes {
            walk(&c.shape, &mut list);
        }
        list
    }

    /// The window against a real kimo server: every view draws (no panic), the tabs show the
    /// selected project's terminals and the views show what changed.
    #[test]
    fn the_window_draws_every_view() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let (app_dir, other) = (home.join("app"), home.join("other"));
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(app_dir.join("notes.txt"), "first line\n").unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let port = rt.block_on(async {
            let manager =
                crate::services::sessions::Manager::new(crate::services::sessions::ManagerConfig {
                    data_dir: tmp.path().join("kimo"),
                    user_home: home.clone(),
                    shell: "/bin/sh".into(),
                    scrollback_bytes: 4096,
                    projects_dir: home.clone(),
                    port: 0,
                    host_name: "test".into(),
                })
                .unwrap();
            // Plan limits ask Claude and Codex themselves: not from tests.
            let mut settings = manager.settings();
            settings.claude_limits = false;
            manager.set_settings(settings).unwrap();
            let router = crate::api::router(
                manager,
                crate::api::Options {
                    token: None,
                    admin_origins: vec![],
                    local_owner: true,
                    bound: "127.0.0.1".parse().unwrap(),
                },
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap();
            });
            port
        });
        let client = Client::local(port);
        for dir in [&app_dir, &other] {
            client
                .post(
                    "/projects",
                    json!({ "mode": "existing", "path": dir.display().to_string(), "resume": false }),
                )
                .unwrap();
        }
        for (name, dir) in [
            ("app one", &app_dir),
            ("app two", &app_dir),
            ("other one", &other),
        ] {
            client
                .post(
                    "/sessions",
                    json!({ "name": name, "kind": "shell", "cwd": dir.display().to_string() }),
                )
                .unwrap();
        }
        client
            .post("/accounts", json!({ "name": "Work", "provider": "claude" }))
            .unwrap();

        let ctx = egui::Context::default();
        let mut app = KimoApp::new(ctx.clone(), client.clone(), tmp.path().join("window.json"));
        // Releases are set by hand below (no GitHub in tests).
        app.last_release = Instant::now();
        let mut frame = eframe::Frame::_new_kittest();
        let mut run = |app: &mut KimoApp, frames: usize| {
            let mut out = None;
            for _ in 0..frames {
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1280.0, 820.0),
                    )),
                    ..Default::default()
                };
                let mut o = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
                // No renderer here: textures are not uploaded anywhere.
                o.textures_delta.clear();
                out = Some(o);
                std::thread::sleep(Duration::from_millis(30));
            }
            texts(&out.unwrap())
        };
        // Data arrives (projects every few seconds, sessions from the event stream).
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            run(&mut app, 2);
            if app.projects.len() == 2 && app.sessions().len() == 3 {
                break;
            }
            app.last_projects = Instant::now() - Duration::from_secs(60);
        }
        assert_eq!((app.projects.len(), app.sessions().len()), (2, 3));

        // Opening a project's agent shows that project's tabs only, and "+".
        let one = app
            .sessions()
            .into_iter()
            .find(|x| s(x, "name") == "app one")
            .unwrap();
        app.open_session(s(&one, "id"));
        app.open_session(
            s(
                &app.sessions()
                    .into_iter()
                    .find(|x| s(x, "name") == "app two")
                    .unwrap(),
                "id",
            )
            .to_string()
            .as_str(),
        );
        let shown = run(&mut app, 5);
        assert!(shown.iter().any(|t| t == "app one"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "app two"));
        assert!(shown.iter().any(|t| t == "+"));
        let other_id = s(
            &app.sessions()
                .into_iter()
                .find(|x| s(x, "name") == "other one")
                .unwrap(),
            "id",
        )
        .to_string();
        app.open_session(&other_id);
        // The memory card lists agents too; it shows when the machine running the test is
        // low on memory.
        app.memory_dismissed = Some(s(&app.memory, "pressure").to_string());
        let shown = run(&mut app, 5);
        // Each name shows in the sidebar; a tab adds a second one.
        let count = |name: &str| shown.iter().filter(|t| *t == name).count();
        assert_eq!(count("other one"), 2, "{shown:?}");
        assert_eq!(
            count("app two"),
            1,
            "other project's tabs hidden: {shown:?}"
        );

        // The view buttons, the panel toggle and every view.
        assert!(shown.iter().any(|t| t == "Subscriptions"));
        // No "Terminals" view button: the terminals belong to the projects.
        assert!(!shown.iter().any(|t| t == "Terminals"), "{shown:?}");
        // The bottom bar: plan use, keeping awake, resources, ports and the kimos.
        assert!(shown.iter().any(|t| t.starts_with("Used")), "{shown:?}");
        assert!(shown.iter().any(|t| t.ends_with("awake: off")), "{shown:?}");
        assert!(shown.iter().any(|t| t.ends_with("3 running")), "{shown:?}");
        assert!(
            shown
                .iter()
                .any(|t| t.ends_with(" port") || t.ends_with(" ports")),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t == "1 kimo"), "{shown:?}");
        app.toggle_sidebar();
        let hidden = run(&mut app, 3);
        assert!(!hidden.iter().any(|t| t == "Projects" || t == "PROJECTS"));
        app.toggle_sidebar();
        for view in [
            View::Usage,
            View::Subscriptions,
            View::Skills,
            View::Settings,
            View::Devices,
            View::Terminals,
        ] {
            app.view = view;
            app.refresh_view();
            let shown = run(&mut app, 20);
            if view == View::Subscriptions {
                assert!(shown.iter().any(|t| t == "Work"), "{shown:?}");
                assert!(shown.iter().any(|t| t == "+ Add account"));
                assert!(!shown.iter().any(|t| t.contains("default claude login")));
                // A sign-in state on every account, never only a "sign in again" button.
                assert!(
                    shown
                        .iter()
                        .any(|t| t == "not signed in" || t == "✓ signed in"),
                    "{shown:?}"
                );
                assert!(!shown.iter().any(|t| t == "Sign in again"));
            }
            if view == View::Settings && cfg!(target_os = "macos") {
                assert!(shown.iter().any(|t| t == "app icon"), "{shown:?}");
            }
        }
        // The right panel: the project's files; a file opens in a tab, is edited and saved.
        app.view = View::Terminals;
        app.open_session(s(&one, "id"));
        app.details.open = true;
        let start = Instant::now();
        let mut shown = vec![];
        while start.elapsed() < Duration::from_secs(10) {
            shown = run(&mut app, 3);
            if shown.iter().any(|t| t.ends_with("notes.txt")) {
                break;
            }
        }
        assert!(
            shown.iter().any(|t| t == "Files") && shown.iter().any(|t| t == "Agents"),
            "{shown:?}"
        );
        assert!(shown.iter().any(|t| t.ends_with("notes.txt")), "{shown:?}");
        let scope = app.details.scope.clone().unwrap();
        app.open_file(scope, "notes.txt", false);
        let start = Instant::now();
        while app.editors[0].loading && start.elapsed() < Duration::from_secs(10) {
            run(&mut app, 2);
        }
        assert_eq!(app.editors[0].text, "first line\n");
        let shown = run(&mut app, 3);
        assert!(shown.iter().any(|t| t.contains("notes.txt")), "{shown:?}");
        app.editors[0].text.push_str("second line\n");
        assert!(app.editors[0].dirty());
        app.save_editor(0);
        let start = Instant::now();
        while app.editors[0].dirty() && start.elapsed() < Duration::from_secs(10) {
            run(&mut app, 2);
        }
        assert_eq!(
            std::fs::read_to_string(app_dir.join("notes.txt")).unwrap(),
            "first line\nsecond line\n"
        );
        app.details.open = false;
        app.editor_active = None;

        // Updates: a new release, a newer kimo installed, a server behind this window.
        app.last_hosts = Instant::now();
        app.release = json!({
            "current": env!("CARGO_PKG_VERSION"), "latest": "v99.0.0",
            "available": true, "restart": false
        });
        let shown = run(&mut app, 3);
        assert!(shown.iter().any(|t| t == "Update available"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "kimo v99.0.0 is ready."));
        app.dismissed = app.pending_update().map(|p| p.key);
        let shown = run(&mut app, 3);
        assert!(!shown.iter().any(|t| t == "Update available"));
        assert!(shown.iter().any(|t| t == "↑ Update"), "{shown:?}");
        app.release = json!({ "restart": true, "installed": "99.0.0" });
        assert!(run(&mut app, 3).iter().any(|t| t == "Restart to update"));
        app.server_version = Some("0.0.1".into());
        let shown = run(&mut app, 3);
        assert!(shown.iter().any(|t| t == "Restart kimo"), "{shown:?}");
        assert!(shown.iter().any(|t| t.contains("still runs 0.0.1")));

        let prefs: Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("window.json")).unwrap())
                .unwrap();
        assert_eq!(prefs["sidebar"], true);
    }

    #[test]
    fn limits_in_the_bottom_bar() {
        let u = json!({ "limits": { "limits": [
            { "name": "5-hour", "used_percent": 4.2, "resets_at": 1000 + 4 * 3600 + 29 * 60 },
            { "name": "weekly", "used_percent": 41.0, "resets_at": 1000 + 3 * 86_400 + 7 * 3600 },
            { "name": "weekly Fable", "used_percent": 15.0, "resets_at": 99 },
        ] } });
        assert_eq!(
            limits_short(&u, 1000),
            "4% 4 h 29 min · 41% 3 d 7 h · 15% Fable"
        );
        assert_eq!(limits_short(&json!({}), 1000), "");
    }

    #[test]
    fn pasted_images_become_png() {
        let image = egui::ColorImage::filled([3, 2], Color32::RED);
        let png = encode_png(&image).unwrap();
        let mut reader = png::Decoder::new(std::io::Cursor::new(png))
            .read_info()
            .unwrap();
        let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
        reader.next_frame(&mut pixels).unwrap();
        assert_eq!((reader.info().width, reader.info().height), (3, 2));
        assert_eq!(&pixels[..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn dropped_file_means_its_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("README.md");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(drop_target(tmp.path()).as_deref(), Some(tmp.path()));
        assert_eq!(drop_target(&file).as_deref(), Some(tmp.path()));
        assert_eq!(drop_target(Path::new("/no/such/place/file")), None);
    }

    #[test]
    fn usage_text() {
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(340_400), "340k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(until(1000 + 3 * 86_400 + 4 * 3600, 1000), "3 d 4 h");
        assert_eq!(until(1000 + 7800, 1000), "2 h 10 min");
        assert_eq!(until(10, 20), "0 min");
        let codex = serde_json::json!({"limits":{"limits":[{"used_percent":15.0,"window_minutes":10080,"resets_at":0}]}});
        assert!(usage_line(&codex).starts_with("15% of weekly limit"));
        assert_eq!(
            usage_line(&serde_json::json!({"tokens_5h":0,"tokens_7d":0})),
            "no usage through kimo in 7 days"
        );
    }

    #[test]
    fn sizes_and_paths() {
        assert_eq!(dollars(12.3), "$12.30");
        assert_eq!(dollars(2676.6), "$2,677");
        assert_eq!(dollars(1_204_000.0), "$1,204,000");
        assert_eq!(mb(5 << 20), "5 MB");
        assert_eq!(mb(3 << 30), "3.0 GB");
        assert_eq!(home("/elsewhere/x"), "/elsewhere/x");
        assert_eq!(
            short_path("/srv/a/very/long/path/to/the/project-folder", 24),
            "…/to/the/project-folder"
        );
        assert_eq!(short_path("/srv/short", 24), "/srv/short");
    }
}
