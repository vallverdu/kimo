//! The kimo window: projects (drop a folder to add one), terminals, subscriptions, skills,
//! settings and devices. It talks to the kimo server only through the API, like any app.

use super::terminal::TerminalView;
use super::theme::{self, dot, primary, section};
use crate::client::Client;
use crate::core::models::{Permissions, QuickCommand, Settings, Theme};
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
    /// The new kimo is installed and the server restarted on it: restart the window too.
    Restart,
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
    url: String,
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
enum Confirm {
    RemoveProject { id: String, name: String },
    RemoveWorktree { project: String, path: String },
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
    account_provider: String,
    move_targets: std::collections::HashMap<String, String>,
    skill_form: SkillForm,
    pairing: Option<Pairing>,
    pair_address: String,
    message: String,
    last_status: Instant,
    last_projects: Instant,
    /// Colour scheme from this machine's settings.
    theme: Theme,
    /// Projects shown folded (just their header), kept in `prefs`.
    collapsed: std::collections::HashSet<String>,
    prefs: PathBuf,
    project_settings: Option<ProjectSettings>,
    worktree_form: Option<WorktreeForm>,
    confirm: Option<Confirm>,
    /// Where the terminal was drawn last frame (dropping files there types their paths).
    terminal_rect: Option<egui::Rect>,
    /// The latest release as this machine's kimo reports it (`/update`).
    release: Value,
    last_release: Instant,
    /// The version whose update card was closed.
    dismissed: Option<String>,
    updating: Updating,
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
            account_provider: "claude".into(),
            move_targets: Default::default(),
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
            pair_address: String::new(),
            message: String::new(),
            last_status: Instant::now() - Duration::from_secs(60),
            last_projects: Instant::now() - Duration::from_secs(60),
            theme: Theme::System,
            collapsed: std::fs::read_to_string(&prefs)
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                .and_then(|v| serde_json::from_value(v["collapsed"].clone()).ok())
                .unwrap_or_default(),
            prefs,
            project_settings: None,
            worktree_form: None,
            confirm: None,
            terminal_rect: None,
            release: Value::Null,
            // The first check a few seconds after start, then every 6 hours.
            last_release: Instant::now() - Duration::from_secs(6 * 3600 - 5),
            dismissed: None,
            updating: Updating::Idle,
        };
        app.job(|c| Ok(Msg::Settings(Box::new(c.get("/settings")?))));
        app
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
        self.skills.clear();
        self.usage.clear();
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
                Msg::Pairing(url, secs) => {
                    self.pairing = Some(Pairing {
                        url,
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
                Msg::Release(v) => self.release = v,
                Msg::Restart => restart_window(),
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
            }
            View::Subscriptions => self.job(|c| Ok(Msg::Usage(c.get("/usage")?))),
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
        }
        if self.last_hosts.elapsed() > Duration::from_secs(15) {
            self.last_hosts = Instant::now();
            self.local_job(|c| Ok(Msg::Hosts(c.get("/peers/keys")?)));
        }
        if self.last_projects.elapsed() > Duration::from_secs(10) {
            self.last_projects = Instant::now();
            self.job(|c| Ok(Msg::Projects(c.get("/projects")?)));
        }
        if self.last_release.elapsed() > Duration::from_secs(6 * 3600) {
            self.last_release = Instant::now();
            self.local_job(|c| Ok(Msg::Release(c.get("/update")?)));
        }
        // Wake up only for the next status poll; live data (events, terminal output) repaints by itself.
        self.ctx.request_repaint_after(Duration::from_secs(3));
    }

    /// Dropped folders become projects; files or folders dropped on the terminal are typed
    /// into it as paths (Claude Code and Codex read images and files given by path).
    fn handle_drops(&mut self, ui: &Ui) {
        let ctx = ui.ctx().clone();
        let (dropped, hovering) =
            ui.input(|i| (i.raw.dropped_files.clone(), !i.raw.hovered_files.is_empty()));
        let terminal = self
            .terminal_rect
            .filter(|_| self.view == View::Terminals && !self.terminals.is_empty());
        let over_terminal =
            |at: Option<egui::Pos2>| terminal.zip(at).is_some_and(|(r, p)| r.contains(p));
        if !dropped.is_empty() {
            if over_terminal(drop_pointer(&ctx)) {
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
            let full = ctx.content_rect();
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
            match terminal {
                Some(term) => {
                    let on_terminal = over_terminal(drop_pointer(&ctx));
                    let left =
                        egui::Rect::from_min_max(full.min, egui::pos2(term.left(), full.max.y));
                    zone(left, "Drop a folder:\nadd it as a project", !on_terminal);
                    zone(
                        term,
                        "Drop files here:\ntype their paths in the terminal",
                        on_terminal,
                    );
                }
                None => zone(full, "Drop a folder to add it as a project", true),
            }
            // No pointer events arrive during a drag: look again soon to follow it.
            ctx.request_repaint_after(Duration::from_millis(60));
        }
    }

    // ----- layout -----

    fn top_bar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::Image::new(egui::load::SizedTexture::new(self.logo.id(), [30.0, 30.0]))
                    .tint(theme::ink()),
            );
            ui.label(RichText::new("kimo").size(20.0).color(theme::ink()));
            ui.label(
                RichText::new("AGENT ENVIRONMENT")
                    .size(10.0)
                    .color(theme::muted()),
            );
            ui.add_space(16.0);
            // Which kimo the views show: this machine or a paired one.
            let mut chosen = self.current;
            let mut pair = false;
            egui::ComboBox::from_id_salt("host")
                .selected_text(
                    RichText::new(format!("host: {}", self.host().name)).color(theme::ink()),
                )
                .width(170.0)
                .show_ui(ui, |ui| {
                    for (i, h) in self.hosts.iter().enumerate() {
                        let running = h
                            .sessions()
                            .iter()
                            .filter(|x| s(x, "status") == "running")
                            .count();
                        let label = if h.loaded() {
                            format!("{} · {running} running", h.name)
                        } else {
                            format!("{} · offline", h.name)
                        };
                        ui.selectable_value(&mut chosen, i, label);
                    }
                    ui.separator();
                    pair = ui.selectable_label(false, "pair another kimo…").clicked();
                });
            if chosen != self.current {
                self.switch_host(chosen);
            }
            if pair {
                self.switch_host(0);
                self.view = View::Devices;
                self.refresh_view();
            }
            ui.add_space(16.0);
            for (view, label) in [
                (View::Terminals, "Terminals"),
                (View::Usage, "Usage"),
                (View::Subscriptions, "Subscriptions"),
                (View::Skills, "Skills"),
                (View::Settings, "Settings"),
                (View::Devices, "Devices"),
            ] {
                let selected = self.view == view;
                let text = RichText::new(label).color(if selected {
                    theme::ink()
                } else {
                    theme::muted()
                });
                let r = ui.add(egui::Button::new(text).frame(false));
                if selected {
                    ui.painter().hline(
                        r.rect.x_range(),
                        r.rect.bottom() + 2.0,
                        egui::Stroke::new(1.0, theme::ink()),
                    );
                }
                if r.clicked() && !selected {
                    self.view = view;
                    self.refresh_view();
                }
            }
            let waiting: Vec<Value> = self
                .sessions()
                .into_iter()
                .filter(|x| s(x, "activity") == "waiting")
                .collect();
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

    fn status_bar(&self, ui: &mut Ui) {
        let sessions = self.sessions();
        let count = |st: &str| sessions.iter().filter(|x| s(x, "status") == st).count();
        let st = &self.status;
        ui.horizontal(|ui| {
            for (color, n, label) in [
                (theme::good(), count("running"), "running"),
                (theme::warn(), count("paused"), "paused"),
            ] {
                dot(ui, color);
                ui.label(RichText::new(format!("{n} {label}")).size(11.0));
            }
            ui.label(
                RichText::new(format!(
                    "{} stopped",
                    sessions.len() - count("running") - count("paused")
                ))
                .size(11.0)
                .color(theme::muted()),
            );
            ui.separator();
            if let Some(agents) = st["agents_rss_bytes"].as_u64() {
                ui.label(RichText::new(format!("agents {}", mb(agents))).size(11.0));
                ui.label(
                    RichText::new(format!(
                        "free {}",
                        mb(st["mem_available_bytes"].as_u64().unwrap_or(0))
                    ))
                    .size(11.0),
                );
                ui.label(
                    RichText::new(format!(
                        "kimo {}",
                        mb(st["kimo_rss_bytes"].as_u64().unwrap_or(0))
                    ))
                    .size(11.0)
                    .color(theme::muted()),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    RichText::new(format!("{} · {}", self.host().name, self.client().base()))
                        .size(11.0)
                        .color(theme::muted()),
                );
            });
        });
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let sessions = self.sessions();
        section(ui, "Projects");
        ui.horizontal(|ui| {
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
                    ui.horizontal(|ui| {
                        project_mark(ui, p);
                        let title = RichText::new(name).size(14.0).strong().color(theme::ink());
                        if ui
                            .add(egui::Label::new(title).sense(egui::Sense::click()).truncate())
                            .on_hover_text(format!("{path}\nclick: open its latest agent"))
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
                                            });
                                        });
                                    });
                                }
                                ui.horizontal_wrapped(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    for kind in ["claude", "codex", "shell"] {
                                        if link_button(ui, &format!("+ {kind}")).on_hover_text(format!("start {kind} here")).clicked() {
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
                let others: Vec<&Value> = placed
                    .iter()
                    .filter(|(_, home)| home.is_none())
                    .map(|(x, _)| x)
                    .collect();
                if !others.is_empty() {
                    section(ui, "Other sessions");
                    for x in others {
                        ui.horizontal(|ui| {
                            dot(ui, theme::status_color(s(x, "status")));
                            if ui.selectable_label(false, s(x, "name")).clicked() {
                                open = Some(s(x, "id").to_string());
                            }
                            activity_tag(ui, x);
                        });
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
            let prefs = json!({ "collapsed": self.collapsed });
            if let Err(e) = std::fs::write(&self.prefs, prefs.to_string()) {
                tracing::warn!("cannot save {}: {e}", self.prefs.display());
            }
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
        if let Some((name, folder)) = history {
            self.job(move |c| {
                let list = c.get(&format!("/conversations?path={}", query_escape(&folder)))?;
                Ok(Msg::History(name, folder, list))
            });
        }
        if let Some((project, cwd, kind)) = start {
            let taken: Vec<String> = sessions.iter().map(|x| s(x, "name").to_string()).collect();
            let mut name = format!("{project} {kind}");
            let mut n = 2;
            while taken.contains(&name) {
                name = format!("{project} {kind} {n}");
                n += 1;
            }
            self.job(move |c| {
                Ok(Msg::Open(c.post(
                    "/sessions",
                    json!({ "name": name, "kind": kind, "cwd": cwd }),
                )?))
            });
        }
    }

    /// Open a project's most recent agent; with none yet, continue its latest conversations
    /// (adding a known folder again does that).
    fn open_project(&mut self, id: &str, placed: &[(Value, Option<String>)]) {
        let Some(p) = self.projects.iter().find(|p| s(p, "id") == id).cloned() else {
            return;
        };
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
        if self.terminals.is_empty() {
            ui.add_space(80.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("No terminal open").size(22.0).color(theme::ink()));
                ui.label(RichText::new("Open a session on the left, add an agent to a project, or drop a folder to start a project.").color(theme::muted()));
            });
            return;
        }
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
        if self.terminals.is_empty() {
            return;
        }
        self.active = self.active.min(self.terminals.len() - 1);
        // The toolbar acts on the active tab's host.
        let tab_host = self.terminals[self.active].host.clone();
        if let Some(i) = self.hosts.iter().position(|h| h.id == tab_host) {
            self.switch_host(i);
        }
        let sessions = self.sessions();
        // Each tab's session, from its own host.
        let infos: Vec<(Option<Value>, String)> = self
            .terminals
            .iter()
            .map(|t| {
                let host = self.hosts.iter().find(|h| h.id == t.host);
                let info = host.and_then(|h| {
                    h.sessions()
                        .into_iter()
                        .find(|x| s(x, "id") == t.session_id)
                });
                (
                    info,
                    host.filter(|h| h.id != "local")
                        .map(|h| h.name.clone())
                        .unwrap_or_default(),
                )
            })
            .collect();
        // Tabs.
        let mut close = None;
        ui.horizontal_wrapped(|ui| {
            for (i, t) in self.terminals.iter().enumerate() {
                let (info, host_name) = (&infos[i].0, &infos[i].1);
                let info = info.as_ref();
                let name = info
                    .map(|x| s(x, "name"))
                    .unwrap_or(&t.session_id)
                    .to_string();
                let name = if host_name.is_empty() {
                    name
                } else {
                    format!("{name} · {host_name}")
                };
                let status = info.map(|x| s(x, "status")).unwrap_or("");
                egui::Frame::new()
                    .stroke(egui::Stroke::new(
                        1.0,
                        if i == self.active {
                            theme::ink()
                        } else {
                            theme::line()
                        },
                    ))
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            dot(ui, theme::status_color(status));
                            if ui
                                .add(
                                    egui::Button::new(RichText::new(&name).color(theme::ink()))
                                        .frame(false),
                                )
                                .clicked()
                            {
                                self.active = i;
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
        });
        if let Some(i) = close {
            self.terminals.remove(i);
            if self.terminals.is_empty() {
                return;
            }
            self.active = self.active.min(self.terminals.len() - 1);
        }

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
                ui.label(RichText::new(format!("{} · {}% cpu", mb(usage["rss_bytes"].as_u64().unwrap_or(0)), usage["cpu_percent"])).size(11.0).color(theme::muted()));
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
                let options: Vec<(String, String)> = std::iter::once((String::new(), format!("default {kind} login")))
                    .chain(accounts.iter().filter(|a| s(a, "provider") == kind).map(|a| (s(a, "id").to_string(), s(a, "name").to_string())))
                    .collect();
                let label = options.iter().find(|(k, _)| *k == current).map(|(_, n)| n.clone()).unwrap_or(current.clone());
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
        let shown = egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, theme::line()))
            .show(ui, |ui| {
                self.terminals[self.active].show(ui, font_size);
            });
        self.terminal_rect = Some(shown.response.rect);
    }

    fn subscriptions_view(&mut self, ui: &mut Ui) {
        ui.heading(RichText::new("Subscriptions").color(theme::ink()));
        ui.label(
            RichText::new(
                "Accounts you switch agents between without logging out. Conversations are shared.",
            )
            .color(theme::muted()),
        );
        let sessions = self.sessions();
        let accounts = self.accounts();
        section(ui, "Logins");
        let mut rows: Vec<Value> = ["claude", "codex"]
            .iter()
            .map(|p| json!({ "id": "default", "name": format!("default {p} login"), "provider": p, "signed_in": null,
                "sessions": sessions.iter().filter(|x| s(x, "kind") == *p && x["account"].is_null()).count() }))
            .collect();
        rows.extend(accounts.iter().cloned());
        egui::Grid::new("accounts").num_columns(5).spacing([18.0, 10.0]).striped(false).show(ui, |ui| {
            for a in &rows {
                let (id, provider) = (s(a, "id").to_string(), s(a, "provider").to_string());
                ui.label(RichText::new(s(a, "name")).color(theme::ink()).size(14.0));
                ui.label(RichText::new(format!("{provider} · {} sessions", a["sessions"])).size(11.0).color(theme::muted()));
                match a["signed_in"].as_bool() {
                    Some(true) => {
                        let text = match a["email"].as_str() {
                            Some(email) => format!("signed in · {email}"),
                            None => "signed in".to_string(),
                        };
                        ui.label(RichText::new(text).size(11.0).color(theme::good()));
                    }
                    Some(false) => { ui.label(RichText::new("not signed in").size(11.0).color(theme::warn())); }
                    None => { ui.label(""); }
                }
                let key = if id == "default" { Value::Null } else { Value::String(id.clone()) };
                let usage = self.usage.iter().find(|u| u["account"] == key && s(u, "provider") == provider);
                let reached = usage.and_then(|u| u["limits"]["reached"].as_str()).is_some();
                ui.label(RichText::new(usage.map(usage_line).unwrap_or_default()).size(11.0).color(if reached { theme::bad() } else { theme::teal() }));
                ui.horizontal(|ui| {
                    if id != "default" && ui.button("sign in").on_hover_text("opens a terminal with this account's login").clicked() {
                        let id = id.clone();
                        self.job(move |c| Ok(Msg::Open(c.post(&format!("/accounts/{id}/login"), json!({}))?)));
                    }
                    let targets: Vec<(String, String)> = std::iter::once(("default".to_string(), format!("default {provider} login")))
                        .chain(accounts.iter().filter(|b| s(b, "provider") == provider).map(|b| (s(b, "id").to_string(), s(b, "name").to_string())))
                        .filter(|(k, _)| *k != id)
                        .collect();
                    if a["sessions"].as_u64().unwrap_or(0) > 0 && !targets.is_empty() {
                        let target = self.move_targets.entry(id.clone()).or_insert_with(|| targets[0].0.clone());
                        let label = targets.iter().find(|(k, _)| k == target).map(|(_, n)| n.clone()).unwrap_or_default();
                        egui::ComboBox::from_id_salt(("move", &id)).selected_text(label).width(160.0).show_ui(ui, |ui| {
                            for (k, n) in &targets {
                                ui.selectable_value(target, k.clone(), n);
                            }
                        });
                        if ui.button("move sessions").on_hover_text("every session continues its conversation on the other account").clicked() {
                            let (from, to) = (id.clone(), target.clone());
                            self.job(move |c| {
                                let r = c.post(&format!("/accounts/{from}/move"), json!({ "to": if to == "default" { Value::Null } else { Value::String(to) } }))?;
                                Ok(Msg::Toast(format!("Moved {} sessions", r["moved"])))
                            });
                        }
                    }
                    if id != "default" && a["sessions"].as_u64() == Some(0) && ui.button(RichText::new("delete").color(theme::bad())).clicked() {
                        let id = id.clone();
                        self.action("Account deleted", move |c| c.delete(&format!("/accounts/{id}")));
                    }
                });
                ui.end_row();
            }
        });
        section(ui, "Add an account");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.account_name)
                    .hint_text("Work Max")
                    .desired_width(220.0),
            );
            egui::ComboBox::from_id_salt("provider")
                .selected_text(&self.account_provider)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.account_provider, "claude".into(), "claude");
                    ui.selectable_value(&mut self.account_provider, "codex".into(), "codex");
                });
            if primary(ui, "add").clicked() && !self.account_name.trim().is_empty() {
                let (name, provider) = (
                    std::mem::take(&mut self.account_name),
                    self.account_provider.clone(),
                );
                self.action("Account added: sign in next", move |c| {
                    c.post("/accounts", json!({ "name": name, "provider": provider }))
                });
            }
        });
        ui.label(
            RichText::new("Usage counts the agents run through kimo: Claude tokens from their transcripts, Codex plan limits as Codex reports them.")
                .size(11.0)
                .color(theme::muted()),
        );
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
            ui.horizontal(|ui| {
                let tile = |ui: &mut Ui, label: &str, value: String, note: String| {
                    ui.vertical(|ui| {
                        ui.set_min_width(150.0);
                        ui.label(RichText::new(label).size(11.0).color(theme::muted()));
                        ui.label(RichText::new(value).size(26.0).color(theme::ink()));
                        ui.label(RichText::new(note).size(11.0).color(theme::text()));
                    });
                    ui.add_space(18.0);
                };
                let split = |v: &Value| format!("claude {} · codex {}", tokens(n(&v["claude"])), tokens(n(&v["codex"])));
                tile(ui, "Today", tokens(both(&r["today"])), split(&r["today"]));
                tile(ui, "Last 7 days", tokens(both(&r["last_7_days"])), split(&r["last_7_days"]));
                tile(ui, &format!("Last {} days", self.daily_days), tokens(both(&r["total"])), split(&r["total"]));
                tile(ui, "Prompts", n(&r["prompts"]).to_string(), format!("in {} conversations", n(&r["conversations"])));
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
                    ui.label(RichText::new(format!("{} prompts", n(&a["prompts"]))).size(12.0).color(theme::text()));
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
            ui.add_space(20.0);
        });
    }

    fn devices_view(&mut self, ui: &mut Ui) {
        ui.heading(RichText::new("Devices").color(theme::ink()));
        ui.label(RichText::new("Use kimo from your phone or any device on your network: show the QR code and scan it.").color(theme::muted()));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section(ui, "Pair a phone");
            let addresses: Vec<Value> = self.network["addresses"].as_array().cloned().unwrap_or_default();
            let remote: Vec<&Value> = addresses.iter().filter(|a| a["kind"] != "localhost").collect();
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("pair-address").selected_text(&self.pair_address).width(320.0).show_ui(ui, |ui| {
                    for a in &remote {
                        ui.selectable_value(&mut self.pair_address, s(a, "url").to_string(), format!("{} · {}", s(a, "label"), s(a, "url")));
                    }
                });
                if primary(ui, "show QR code").clicked() && !self.pair_address.is_empty() {
                    let base = self.pair_address.clone();
                    self.job(move |c| {
                        let p = c.post("/pairing", json!({}))?;
                        Ok(Msg::Pairing(format!("{base}/#pair={}", s(&p, "code")), p["expires_in"].as_u64().unwrap_or(600)))
                    });
                }
            });
            if let Some(p) = &self.pairing {
                let left = p.until.saturating_duration_since(Instant::now()).as_secs();
                if left == 0 {
                    self.pairing = None;
                } else {
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        qr(ui, &p.url, 220.0);
                        ui.vertical(|ui| {
                            ui.label("Scan with the phone camera, or open:");
                            ui.label(RichText::new(&p.url).size(11.0).color(theme::teal()));
                            ui.label(RichText::new(format!("valid for {}:{:02}, once", left / 60, left % 60)).size(11.0).color(theme::muted()));
                        });
                    });
                    // Watch for the new device.
                    if self.last_projects.elapsed() > Duration::from_secs(2) {
                        self.job(|c| Ok(Msg::Devices(c.get("/devices")?)));
                    }
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
            ui.label(RichText::new("The code is the part after #pair= in the other kimo's pairing link (Devices → show QR code, or `kimo pair` there).").size(10.5).color(theme::muted()));
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
                        ui.label(RichText::new(home(s(&listing, "path"))).size(11.0));
                        if ui.small_button("use this folder").clicked() {
                            form.path = s(&listing, "path").to_string();
                            form.listing = None;
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
            Confirm::RemoveProject { name, .. } => (
                format!("Remove {name}?"),
                "kimo forgets the project. Its folder and files stay where they are, and its agents move to Other sessions.".to_string(),
            ),
            Confirm::RemoveWorktree { path, .. } => (
                "Remove this worktree?".to_string(),
                format!("git deletes the folder {} (refused if it has uncommitted changes). The branch stays; stopped agents that worked there are removed from kimo.", home(path)),
            ),
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
                        .button(RichText::new("remove").color(theme::bad()))
                        .clicked();
                    no = ui.button("cancel").clicked();
                });
            });
        if yes {
            match self.confirm.take() {
                Some(Confirm::RemoveProject { id, .. }) => {
                    self.action("Project removed: its folder is untouched", move |c| {
                        c.delete(&format!("/projects/{id}"))
                    });
                }
                Some(Confirm::RemoveWorktree { project, path }) => {
                    self.action("Worktree removed", move |c| {
                        c.delete(&format!(
                            "/projects/{project}/worktrees?path={}",
                            query_escape(&path)
                        ))
                    });
                }
                None => {}
            }
        } else if no {
            self.confirm = None;
        }
    }

    /// "Update available" card (bottom right), like Orca's.
    fn update_card(&mut self, ctx: &egui::Context) {
        let r = &self.release;
        let Some(latest) = r["latest"].as_str().map(String::from) else {
            return;
        };
        if r["available"] != true || self.dismissed.as_deref() == Some(latest.as_str()) {
            return;
        }
        let manual = r["manual"].as_str().map(String::from);
        let url = r["url"].as_str().map(String::from);
        let mut close = false;
        let mut update = false;
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
                                RichText::new("Update available")
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
                                        .on_hover_text("later")
                                        .clicked();
                                },
                            );
                        });
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!("kimo {latest} is ready."))
                                .size(13.5)
                                .color(theme::text()),
                        );
                        ui.label(
                            RichText::new(
                                "Running agents restart and continue their conversations.",
                            )
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
                                    "Update",
                                    egui::FontId::proportional(13.5),
                                    theme::background(),
                                );
                                update =
                                    r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                            }
                        }
                    });
            });
        if close {
            self.dismissed = Some(latest);
        }
        if update {
            self.updating = Updating::Busy("Downloading…");
            let (client, tx, ctx) = (
                self.hosts[0].client.clone(),
                self.tx.clone(),
                self.ctx.clone(),
            );
            std::thread::spawn(move || {
                let msg = match install_and_restart(&client) {
                    Ok(()) => Msg::Restart,
                    Err(e) => Msg::UpdateFailed(format!("Update failed: {e}")),
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        }
        if self.updating != Updating::Idle {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
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

/// Install the latest release, restart the server on it and wait until it answers again.
fn install_and_restart(client: &Client) -> Result<()> {
    // Downloading takes a while on a slow line.
    client.call_timeout("POST", "/update", Some(&json!({})), 900)?;
    client.post("/update/restart", json!({}))?;
    // The old server stops its agents and execs the new binary: wait for it to go and return.
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut went_away = false;
    while Instant::now() < deadline {
        let up = client.call_timeout("GET", "/status", None, 2).is_ok();
        if !up {
            went_away = true;
        } else if went_away {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    anyhow::bail!("the new kimo server did not come back: see ~/.kimo/kimo.log")
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
        _ => {}
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
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.handle_messages();
        self.poll();
        let ctx = ui.ctx().clone();
        self.terminal_rect = None;
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
        egui::Panel::left("projects")
            .resizable(true)
            .default_size(280.0)
            .frame(
                egui::Frame::new()
                    .fill(theme::surface())
                    .inner_margin(egui::Margin::same(14)),
            )
            .show(ui, |ui| self.sidebar(ui));
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
        self.update_card(&ctx);
        self.handle_drops(ui);
        self.toasts(&ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
