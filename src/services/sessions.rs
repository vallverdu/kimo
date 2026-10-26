//! Terminal sessions: PTYs, process lifecycle, account switching and resource sampling.
//!
//! One reader thread and one writer thread per running session; everything else is shared.

use crate::core::launch::{self, Launch};
use crate::core::lifecycle::{self, Action};
use crate::core::models::{
    Account, Client, Provider, Run, Scope, SessionKind, SessionSpec, SessionStatus, State,
};
use crate::core::procfs::cpu_percent;
use crate::core::scrollback::Scrollback;
use crate::services::{accounts, stats, store::Store};
use anyhow::{anyhow, bail, Context, Result};
use axum::body::Bytes;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc, watch};

pub struct ManagerConfig {
    pub data_dir: PathBuf,
    pub user_home: PathBuf,
    pub shell: String,
    pub scrollback_bytes: usize,
    /// Where new projects are created by default (e.g. ~/dev).
    pub projects_dir: PathBuf,
    /// HTTP port kimo listens on (for pairing links and tailscale serve).
    pub port: u16,
    /// This kimo's name for its peers (prefix of its agents' ids on other hosts).
    pub host_name: String,
}

/// What the API shows for a session.
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    #[serde(flatten)]
    pub spec: SessionSpec,
    pub status: SessionStatus,
    pub pid: Option<i32>,
    pub exit_code: Option<u32>,
    pub usage: Usage,
    /// Unix time (ms) of the last terminal output: how long an agent has been quiet.
    pub last_output_at: Option<u64>,
    /// For running sessions: working, waiting (needs you) or idle.
    pub activity: Option<crate::core::activity::Activity>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Usage {
    pub rss_bytes: u64,
    pub cpu_percent: f64,
    pub processes: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountInfo {
    #[serde(flatten)]
    pub account: Account,
    pub signed_in: bool,
    /// The email the account is signed in with, when the CLI records it (Claude).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    pub sessions: usize,
    /// This computer's own login (`~/.claude`, `~/.codex`), not one kept by kimo.
    pub system: bool,
    /// New agents of its provider start on it.
    pub active: bool,
}

/// The id that names this computer's own login where an account id is expected.
pub const SYSTEM: &str = "system";

/// An account id from a client: "system" and "default" mean this computer's own login.
pub fn account_key(account: Option<String>) -> Option<String> {
    account.filter(|a| !a.is_empty() && a != SYSTEM && a != "default")
}

/// Fields the user provides to create a session.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct NewSession {
    pub name: String,
    pub kind: SessionKind,
    pub cwd: String,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
}

struct Runtime {
    pid: i32,
    master: Box<dyn MasterPty + Send>,
    input: std_mpsc::Sender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
}

struct Live {
    status: SessionStatus,
    runtime: Option<Runtime>,
    scrollback: Arc<Mutex<Scrollback>>,
    output: broadcast::Sender<Bytes>,
    /// Bumped on every start/stop so stale exit notices from old processes are ignored.
    generation: u64,
    exit_code: Option<u32>,
    usage: Usage,
    last_ticks: Option<(u64, Instant)>,
    size: PtySize,
    last_output: Arc<AtomicU64>,
}

pub(crate) struct Inner {
    pub(crate) state: State,
    live: HashMap<String, Live>,
}

pub struct Manager {
    pub(crate) cfg: ManagerConfig,
    store: Store,
    inner: Mutex<Inner>,
    exits: mpsc::UnboundedSender<(String, u64)>,
    /// Bumped on every visible change; event streams wait on it.
    changes: watch::Sender<u64>,
    /// One-time pairing codes and when they expire (memory only).
    pub(crate) pairing: Mutex<HashMap<String, Instant>>,
    /// Asked to restart (after an update): `kimo serve` stops and starts the new binary.
    restart: tokio::sync::Notify,
    /// When kimo last stopped agents to free memory (figures lag while processes exit).
    pub(crate) last_freed: Mutex<Option<Instant>>,
}

/// A folder path with symlinks resolved (macOS `/var` → `/private/var`), so paths written by
/// agents compare equal to the ones kimo started them in.
pub(crate) fn canonical(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(|_| path.to_string(), |p| p.display().to_string())
}

/// Runs kept for usage (the oldest are dropped).
const MAX_RUNS: usize = 2000;

/// Close the open run of a session, if any.
fn end_run(state: &mut State, session: &str, at: u64) {
    if let Some(run) = state
        .runs
        .iter_mut()
        .rev()
        .find(|r| r.session == session && r.ended_at.is_none())
    {
        run.ended_at = Some(at);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// A client attached to a terminal: replayed scrollback plus live output.
pub struct Attachment {
    pub replay: Vec<u8>,
    pub output: broadcast::Receiver<Bytes>,
}

fn signal_group(pid: i32, sig: libc::c_int) {
    // SAFETY: plain kill(2) on the session's process group (the PTY child is a session leader).
    unsafe {
        libc::kill(-pid, sig);
    }
}

fn new_live(cap: usize) -> Live {
    Live {
        status: SessionStatus::Stopped,
        runtime: None,
        scrollback: Arc::new(Mutex::new(Scrollback::new(cap))),
        output: broadcast::channel(256).0,
        generation: 0,
        exit_code: None,
        usage: Usage::default(),
        last_ticks: None,
        last_output: Arc::new(AtomicU64::new(0)),
        size: PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        },
    }
}

pub(crate) fn slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let short = &uuid::Uuid::new_v4().simple().to_string()[..6];
    if s.is_empty() {
        short.to_string()
    } else {
        format!("{}-{short}", &s[..s.len().min(24)])
    }
}

impl Manager {
    /// Load the saved state (all sessions start as stopped) and start the exit listener.
    /// Must be called inside a tokio runtime.
    pub fn new(cfg: ManagerConfig) -> Result<Arc<Self>> {
        let store = Store::new(cfg.data_dir.join("state.json"));
        let mut state = store.load()?;
        // Sign-in terminals from before are done: none is running at start.
        state.sessions.retain(|s| !launch::is_login(s));
        let live = state
            .sessions
            .iter()
            .map(|s| (s.id.clone(), new_live(cfg.scrollback_bytes)))
            .collect();
        let (exits, mut exits_rx) = mpsc::unbounded_channel();
        let manager = Arc::new(Self {
            cfg,
            store,
            inner: Mutex::new(Inner { state, live }),
            exits,
            changes: watch::channel(0).0,
            pairing: Mutex::new(HashMap::new()),
            restart: tokio::sync::Notify::new(),
            last_freed: Mutex::new(None),
        });
        let weak = Arc::downgrade(&manager);
        tokio::spawn(async move {
            while let Some((id, generation)) = exits_rx.recv().await {
                match weak.upgrade() {
                    Some(m) => m.on_exit(&id, generation),
                    None => break,
                }
            }
        });
        Ok(manager)
    }

    pub(crate) fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn port(&self) -> u16 {
        self.cfg.port
    }

    pub fn host_name(&self) -> &str {
        &self.cfg.host_name
    }

    /// `~` and `~/x` become paths in the user's home.
    pub(crate) fn expand_home(&self, path: &str) -> String {
        match path.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => {
                format!("{}{rest}", self.cfg.user_home.display())
            }
            _ => path.to_string(),
        }
    }

    pub(crate) fn save(&self, inner: &Inner) -> Result<()> {
        self.changed();
        self.store.save(&inner.state)
    }

    fn changed(&self) {
        self.changes.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// Notified on every visible change (status, usage, accounts, apps).
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    // ----- queries -----

    pub fn sessions(&self) -> Vec<SessionInfo> {
        let inner = self.lock();
        inner
            .state
            .sessions
            .iter()
            .filter_map(|s| Self::info(&inner, s))
            .collect()
    }

    pub fn session(&self, id: &str) -> Option<SessionInfo> {
        let inner = self.lock();
        inner
            .state
            .sessions
            .iter()
            .find(|s| s.id == id)
            .and_then(|s| Self::info(&inner, s))
    }

    fn info(inner: &Inner, spec: &SessionSpec) -> Option<SessionInfo> {
        let live = inner.live.get(&spec.id)?;
        Some(SessionInfo {
            spec: spec.clone(),
            status: live.status,
            pid: live.runtime.as_ref().map(|r| r.pid),
            exit_code: live.exit_code,
            usage: live.usage,
            last_output_at: Some(live.last_output.load(Ordering::Relaxed)).filter(|&t| t > 0),
            activity: (live.status == SessionStatus::Running).then(|| {
                let quiet = now_ms().saturating_sub(live.last_output.load(Ordering::Relaxed));
                let recent = live.scrollback.lock().unwrap().tail(3000);
                crate::core::activity::classify(quiet, &recent)
            }),
        })
    }

    /// The accounts: this computer's own login per provider (when signed in), then kimo's.
    pub fn accounts(&self) -> Vec<AccountInfo> {
        let inner = self.lock();
        let active = &inner.state.settings.active_accounts;
        let count = |provider: Provider, account: Option<&str>| {
            inner
                .state
                .sessions
                .iter()
                .filter(|s| {
                    s.account.as_deref() == account
                        && s.kind.provider() == Some(provider)
                        && !launch::is_login(s)
                })
                .count()
        };
        let mut list: Vec<AccountInfo> = [Provider::Claude, Provider::Codex]
            .into_iter()
            .filter_map(|provider| {
                let (signed_in, email) = accounts::system_login(&self.cfg.user_home, provider);
                // Shown when signed in, or while agents still use it.
                if !signed_in && count(provider, None) == 0 {
                    return None;
                }
                Some(AccountInfo {
                    account: Account {
                        id: SYSTEM.into(),
                        name: email
                            .clone()
                            .unwrap_or_else(|| "this computer's login".into()),
                        provider,
                        extra: Default::default(),
                    },
                    signed_in,
                    email,
                    sessions: count(provider, None),
                    system: true,
                    active: active.get(provider).is_none(),
                })
            })
            .collect();
        list.extend(inner.state.accounts.iter().map(|a| {
            let (signed_in, email) = accounts::login(&self.cfg.data_dir, a);
            AccountInfo {
                account: a.clone(),
                signed_in,
                email,
                sessions: count(a.provider, Some(&a.id)),
                system: false,
                active: active.get(a.provider) == Some(&a.id),
            }
        }));
        list
    }

    // ----- sessions -----

    pub fn create(&self, new: NewSession, start: bool) -> Result<SessionInfo> {
        let name = new.name.trim();
        if name.is_empty() {
            bail!("the session needs a name");
        }
        let cwd = if new.cwd.trim().is_empty() {
            self.cfg.user_home.display().to_string()
        } else {
            self.expand_home(new.cwd.trim())
        };
        if !std::path::Path::new(&cwd).is_dir() {
            bail!("folder not found: {cwd}");
        }
        // No account named: the active one of the provider ("system" picks this computer's login).
        let account = match new.account {
            None if new.command.is_none() => new.kind.provider().and_then(|p| {
                let inner = self.lock();
                inner
                    .state
                    .settings
                    .active_accounts
                    .get(p)
                    .filter(|id| inner.state.accounts.iter().any(|a| &a.id == *id))
                    .cloned()
            }),
            other => account_key(other),
        };
        let mut spec = SessionSpec {
            id: slug(name),
            name: name.to_string(),
            kind: new.kind,
            cwd,
            account,
            args: new.args,
            command: new.command.filter(|c| !c.is_empty()),
            started_once: false,
            conversation: None,
            resume_on_start: false,
            freed: None,
            extra: Default::default(),
        };
        if spec.kind == SessionKind::Claude && spec.command.is_none() {
            spec.conversation = Some(uuid::Uuid::new_v4().to_string());
        }
        {
            let mut inner = self.lock();
            Self::check_account(&inner.state, spec.kind, spec.account.as_deref())?;
            inner
                .live
                .insert(spec.id.clone(), new_live(self.cfg.scrollback_bytes));
            inner.state.sessions.push(spec.clone());
            self.save(&inner)?;
        }
        if start {
            self.start(&spec.id)?;
        }
        self.session(&spec.id)
            .ok_or_else(|| anyhow!("session vanished"))
    }

    fn check_account(state: &State, kind: SessionKind, account: Option<&str>) -> Result<()> {
        let Some(id) = account else { return Ok(()) };
        let acc = state
            .accounts
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| anyhow!("unknown account: {id}"))?;
        if kind.provider() != Some(acc.provider) {
            bail!(
                "account '{}' is a {:?} account and cannot run a {:?} session",
                acc.name,
                acc.provider,
                kind
            );
        }
        Ok(())
    }

    pub async fn delete(&self, id: &str) -> Result<()> {
        self.stop_and_wait(id).await?;
        let mut inner = self.lock();
        inner.state.sessions.retain(|s| s.id != id);
        inner.live.remove(id);
        self.save(&inner)
    }

    pub async fn act(&self, id: &str, action: Action) -> Result<SessionInfo> {
        let status = self
            .session(id)
            .ok_or_else(|| anyhow!("unknown session: {id}"))?
            .status;
        if lifecycle::next(status, action).is_none() {
            bail!("cannot {action:?} a {status:?} session");
        }
        match action {
            Action::Start => self.start(id)?,
            Action::Pause => self.signal(id, libc::SIGSTOP, SessionStatus::Paused)?,
            Action::Resume => self.signal(id, libc::SIGCONT, SessionStatus::Running)?,
            Action::Stop => self.stop_and_wait(id).await?,
            Action::Restart => {
                self.stop_and_wait(id).await?;
                self.start(id)?;
            }
        }
        self.session(id)
            .ok_or_else(|| anyhow!("unknown session: {id}"))
    }

    fn signal(&self, id: &str, sig: libc::c_int, status: SessionStatus) -> Result<()> {
        let mut inner = self.lock();
        let live = inner
            .live
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown session: {id}"))?;
        let rt = live
            .runtime
            .as_ref()
            .ok_or_else(|| anyhow!("session is not running"))?;
        signal_group(rt.pid, sig);
        live.status = status;
        if status == SessionStatus::Paused {
            live.usage.cpu_percent = 0.0;
        }
        self.changed();
        Ok(())
    }

    pub(crate) fn start(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        let spec = inner
            .state
            .sessions
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown session: {id}"))?;
        let account = match &spec.account {
            Some(aid) => Some(
                inner
                    .state
                    .accounts
                    .iter()
                    .find(|a| &a.id == aid)
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown account: {aid}"))?,
            ),
            None => None,
        };
        if let Some(acc) = &account {
            accounts::ensure_home(&self.cfg.data_dir, &self.cfg.user_home, acc)?;
        }
        let prefs = spec
            .kind
            .provider()
            .map(|p| inner.state.settings.agents.get(p).clone());
        let mut launch = launch::build(
            &spec,
            account.as_ref(),
            &self.cfg.data_dir,
            self.can_resume(&spec),
            &self.cfg.shell,
            prefs.as_ref(),
        )
        .map_err(|e| anyhow!(e))?;
        // Agents can reach kimo (`kimo agents`, `kimo send`…): its address and its binary on PATH.
        launch.env.push((
            "KIMO_URL".into(),
            format!("http://127.0.0.1:{}", self.cfg.port),
        ));
        if let Some(dir) = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.to_path_buf()))
        {
            let path = std::env::var("PATH").unwrap_or_default();
            launch
                .env
                .push(("PATH".into(), format!("{}:{path}", dir.display())));
        }
        let live = inner
            .live
            .get_mut(id)
            .ok_or_else(|| anyhow!("unknown session: {id}"))?;
        if live.runtime.is_some() {
            bail!("session is already running");
        }
        live.generation += 1;
        let note = format!(
            "\r\n\x1b[2m[kimo] {} {}{}\x1b[0m\r\n",
            if spec.started_once {
                "resuming"
            } else {
                "starting"
            },
            launch.program,
            if launch.args.is_empty() {
                String::new()
            } else {
                format!(" {}", launch.args.join(" "))
            }
        );
        live.scrollback.lock().unwrap().push(note.as_bytes());
        let _ = live.output.send(Bytes::from(note));
        let runtime = self
            .spawn(id, live, &launch)
            .with_context(|| format!("starting {}", launch.program))?;
        live.runtime = Some(runtime);
        live.status = SessionStatus::Running;
        live.exit_code = None;
        live.last_ticks = None;
        if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
            s.started_once = true;
            s.freed = None;
        }
        // Remember which account this run uses (usage per subscription).
        if let (Some(provider), None) = (spec.kind.provider(), &spec.command) {
            let now = now_ms();
            end_run(&mut inner.state, id, now);
            inner.state.runs.push(Run {
                session: id.to_string(),
                provider,
                account: spec.account.clone(),
                cwd: spec.cwd.clone(),
                started_at: now,
                ended_at: None,
                conversation: spec.conversation.clone(),
                extra: Default::default(),
            });
            let excess = inner.state.runs.len().saturating_sub(MAX_RUNS);
            inner.state.runs.drain(..excess);
        }
        self.save(&inner)
    }

    /// Resume only a conversation that exists: a Claude session that never got a message has
    /// no transcript, and `--resume` would fail, so it starts fresh with the same id.
    fn can_resume(&self, spec: &SessionSpec) -> bool {
        if !spec.started_once {
            return false;
        }
        let Some(id) = spec
            .conversation
            .as_deref()
            .filter(|_| spec.kind == SessionKind::Claude)
        else {
            return true;
        };
        let projects = launch::default_home(Provider::Claude, &self.cfg.user_home).join("projects");
        let file = format!("{id}.jsonl");
        std::fs::read_dir(projects)
            .into_iter()
            .flatten()
            .flatten()
            .any(|dir| dir.path().join(&file).exists())
    }

    fn spawn(&self, id: &str, live: &Live, launch: &Launch) -> Result<Runtime> {
        let pair = native_pty_system().openpty(live.size)?;
        let mut cmd = CommandBuilder::new(&launch.program);
        cmd.args(&launch.args);
        cmd.cwd(&launch.cwd);
        for (key, _) in std::env::vars_os() {
            if key.to_str().is_some_and(launch::is_private_env) {
                cmd.env_remove(key);
            }
        }
        for (k, v) in &launch.env {
            cmd.env(k, v);
        }
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);
        let pid = child
            .process_id()
            .map(|p| p as i32)
            .ok_or_else(|| anyhow!("no process id"))?;
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;

        let last_output = live.last_output.clone();
        let (scrollback, output, exits, generation, sid) = (
            live.scrollback.clone(),
            live.output.clone(),
            self.exits.clone(),
            live.generation,
            id.to_string(),
        );
        std::thread::Builder::new()
            .name(format!("pty-out-{id}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            scrollback.lock().unwrap().push(&buf[..n]);
                            last_output.store(now_ms(), Ordering::Relaxed);
                            let _ = output.send(Bytes::copy_from_slice(&buf[..n]));
                        }
                    }
                }
                let _ = exits.send((sid, generation));
            })?;

        let (input, input_rx) = std_mpsc::channel::<Vec<u8>>();
        std::thread::Builder::new()
            .name(format!("pty-in-{id}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                for data in input_rx {
                    if writer
                        .write_all(&data)
                        .and_then(|_| writer.flush())
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Runtime {
            pid,
            master: pair.master,
            input,
            child,
        })
    }

    fn on_exit(&self, id: &str, generation: u64) {
        let mut inner = self.lock();
        let Some(live) = inner.live.get_mut(id) else {
            return;
        };
        if live.generation != generation {
            return;
        }
        if let Some(mut rt) = live.runtime.take() {
            let code = match rt.child.try_wait() {
                Ok(Some(status)) => Some(status.exit_code()),
                _ => {
                    signal_group(rt.pid, libc::SIGKILL);
                    rt.child.wait().ok().map(|s| s.exit_code())
                }
            };
            live.exit_code = code;
            let note = format!(
                "\r\n\x1b[2m[kimo] exited ({})\x1b[0m\r\n",
                code.map_or("?".into(), |c| c.to_string())
            );
            live.scrollback.lock().unwrap().push(note.as_bytes());
            let _ = live.output.send(Bytes::from(note));
        }
        live.status = SessionStatus::Exited;
        live.usage = Usage::default();
        let signed_in = live.exit_code == Some(0);
        end_run(&mut inner.state, id, now_ms());
        // A sign-in terminal that finished well is not kept (failed ones stay to be read).
        if signed_in
            && inner
                .state
                .sessions
                .iter()
                .any(|s| s.id == id && launch::is_login(s))
        {
            inner.state.sessions.retain(|s| s.id != id);
            inner.live.remove(id);
        }
        let _ = self.save(&inner);
    }

    /// End the session's processes (SIGHUP/SIGTERM, then SIGKILL after 3 s) and wait for them.
    async fn stop_and_wait(&self, id: &str) -> Result<()> {
        let runtime = {
            let mut inner = self.lock();
            let live = inner
                .live
                .get_mut(id)
                .ok_or_else(|| anyhow!("unknown session: {id}"))?;
            live.generation += 1;
            if live.runtime.is_some() {
                live.status = SessionStatus::Stopped;
            }
            live.usage = Usage::default();
            live.runtime.take()
        };
        let Some(mut rt) = runtime else { return Ok(()) };
        tokio::task::spawn_blocking(move || {
            for sig in [libc::SIGHUP, libc::SIGTERM, libc::SIGCONT] {
                signal_group(rt.pid, sig);
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = rt.child.try_wait() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            signal_group(rt.pid, libc::SIGKILL);
            let _ = rt.child.kill();
            let _ = rt.child.wait();
            drop(rt.master);
        })
        .await?;
        let mut inner = self.lock();
        if let Some(live) = inner.live.get_mut(id) {
            let note = b"\r\n\x1b[2m[kimo] stopped: memory freed, start to continue\x1b[0m\r\n";
            live.scrollback.lock().unwrap().push(note);
            let _ = live.output.send(Bytes::from_static(note));
        }
        end_run(&mut inner.state, id, now_ms());
        self.save(&inner)
    }

    // ----- terminal I/O -----

    pub fn attach(&self, id: &str) -> Option<Attachment> {
        let inner = self.lock();
        let live = inner.live.get(id)?;
        // Subscribe before snapshotting so no output falls between the two.
        let output = live.output.subscribe();
        let replay = live.scrollback.lock().unwrap().snapshot();
        Some(Attachment { replay, output })
    }

    pub fn write(&self, id: &str, data: Vec<u8>) {
        let inner = self.lock();
        if let Some(rt) = inner.live.get(id).and_then(|l| l.runtime.as_ref()) {
            let _ = rt.input.send(data);
        }
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        if cols == 0 || rows == 0 {
            return;
        }
        let mut inner = self.lock();
        if let Some(live) = inner.live.get_mut(id) {
            live.size = PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Some(rt) = &live.runtime {
                let _ = rt.master.resize(live.size);
            }
        }
    }

    // ----- accounts -----

    pub fn add_account(&self, name: &str, provider: Provider) -> Result<Account> {
        let name = name.trim();
        if name.is_empty() {
            bail!("the account needs a name");
        }
        let account = Account {
            id: slug(name),
            name: name.to_string(),
            provider,
            extra: Default::default(),
        };
        accounts::ensure_home(&self.cfg.data_dir, &self.cfg.user_home, &account)?;
        let mut inner = self.lock();
        inner.state.accounts.push(account.clone());
        self.save(&inner)?;
        Ok(account)
    }

    pub fn delete_account(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        let users: Vec<_> = inner
            .state
            .sessions
            .iter()
            .filter(|s| s.account.as_deref() == Some(id))
            .map(|s| s.name.clone())
            .collect();
        if !users.is_empty() {
            bail!("account is used by: {}", users.join(", "));
        }
        inner.state.accounts.retain(|a| a.id != id);
        self.save(&inner)?;
        accounts::remove_home(&self.cfg.data_dir, id)
    }

    /// Session that signs `account_id` in, started in the user's home folder.
    pub fn login_session(&self, account_id: &str) -> Result<SessionInfo> {
        let acc = self
            .lock()
            .state
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown account: {account_id}"))?;
        let kind = match acc.provider {
            Provider::Claude => SessionKind::Claude,
            Provider::Codex => SessionKind::Codex,
        };
        self.create(
            NewSession {
                name: format!("login {}", acc.name),
                kind,
                cwd: String::new(),
                account: Some(acc.id),
                args: vec![],
                command: Some(launch::login_command(acc.provider)),
            },
            true,
        )
    }

    /// Move a session to another account (or the default login). A live session is restarted
    /// and continues its conversation under the new subscription.
    pub async fn set_account(&self, id: &str, account: Option<String>) -> Result<SessionInfo> {
        let account = account_key(account);
        let was_live = {
            let mut inner = self.lock();
            let kind = inner
                .state
                .sessions
                .iter()
                .find(|s| s.id == id)
                .ok_or_else(|| anyhow!("unknown session: {id}"))?
                .kind;
            Self::check_account(&inner.state, kind, account.as_deref())?;
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.account = account;
            }
            self.save(&inner)?;
            inner.live.get(id).is_some_and(|l| l.runtime.is_some())
        };
        if was_live {
            self.stop_and_wait(id).await?;
            self.start(id)?;
        }
        self.session(id)
            .ok_or_else(|| anyhow!("unknown session: {id}"))
    }

    /// Move every session of `from` (None = default login) of the target's provider to `to`.
    /// This is the "subscription hit its limit" button.
    pub async fn move_sessions(&self, from: Option<&str>, to: Option<String>) -> Result<usize> {
        let ids: Vec<String> = {
            let inner = self.lock();
            let provider = to
                .as_deref()
                .or(from)
                .and_then(|t| inner.state.accounts.iter().find(|a| a.id == t))
                .map(|a| a.provider)
                .ok_or_else(|| anyhow!("pick an account to move to"))?;
            inner
                .state
                .sessions
                .iter()
                .filter(|s| {
                    s.account.as_deref() == from
                        && s.kind.provider() == Some(provider)
                        && s.command.is_none()
                })
                .map(|s| s.id.clone())
                .collect()
        };
        for id in &ids {
            self.set_account(id, to.clone()).await?;
        }
        Ok(ids.len())
    }

    /// Make `account` (None = this computer's login) the one new agents of `provider` start
    /// on; with `move_agents`, also move that provider's agents to it (live ones restart and
    /// continue their conversation). Returns how many agents moved.
    pub async fn set_active_account(
        &self,
        provider: Provider,
        account: Option<String>,
        move_agents: bool,
    ) -> Result<usize> {
        let account = account_key(account);
        let ids: Vec<String> = {
            let mut inner = self.lock();
            if let Some(id) = &account {
                let acc = inner
                    .state
                    .accounts
                    .iter()
                    .find(|a| &a.id == id)
                    .ok_or_else(|| anyhow!("unknown account: {id}"))?;
                if acc.provider != provider {
                    bail!("{} is not a {provider:?} account", acc.name);
                }
            }
            inner
                .state
                .settings
                .active_accounts
                .set(provider, account.clone());
            self.save(&inner)?;
            if !move_agents {
                return Ok(0);
            }
            inner
                .state
                .sessions
                .iter()
                .filter(|s| {
                    s.kind.provider() == Some(provider)
                        && s.command.is_none()
                        && s.account != account
                })
                .map(|s| s.id.clone())
                .collect()
        };
        for id in &ids {
            self.set_account(id, account.clone()).await?;
        }
        Ok(ids.len())
    }

    // ----- resources -----

    /// Refresh memory/CPU of every running session from /proc.
    pub fn sample(&self) {
        let pids: Vec<(String, i32)> = {
            let inner = self.lock();
            inner
                .live
                .iter()
                .filter_map(|(id, l)| l.runtime.as_ref().map(|r| (id.clone(), r.pid)))
                .collect()
        };
        if pids.is_empty() {
            return;
        }
        let table = stats::read_table();
        let (page, hz, now) = (stats::page_size(), stats::ticks_per_sec(), Instant::now());
        let mut inner = self.lock();
        for (id, pid) in pids {
            let Some(live) = inner.live.get_mut(&id) else {
                continue;
            };
            if live.runtime.as_ref().map(|r| r.pid) != Some(pid) {
                continue;
            }
            let tree = table.tree_usage(pid);
            let cpu = match live.last_ticks {
                Some((ticks, at)) => cpu_percent(
                    ticks,
                    tree.cpu_ticks,
                    now.duration_since(at).as_secs_f64(),
                    hz,
                ),
                None => 0.0,
            };
            live.last_ticks = Some((tree.cpu_ticks, now));
            live.usage = Usage {
                rss_bytes: tree.rss_pages * page,
                cpu_percent: (cpu * 10.0).round() / 10.0,
                processes: tree.processes,
            };
        }
        drop(inner);
        self.changed();
        self.capture_codex_ids();
    }

    /// Learn the session id Codex chose for each new Codex session (from its log's first line),
    /// so the session resumes exactly its own conversation.
    fn capture_codex_ids(&self) {
        let (pending, mut claimed) = {
            let inner = self.lock();
            let pending: Vec<(String, String, u64)> = inner
                .state
                .sessions
                .iter()
                .filter(|s| {
                    s.kind == SessionKind::Codex && s.command.is_none() && s.conversation.is_none()
                })
                .filter(|s| inner.live.get(&s.id).is_some_and(|l| l.runtime.is_some()))
                .filter_map(|s| {
                    let run = inner.state.runs.iter().rev().find(|r| r.session == s.id)?;
                    Some((s.id.clone(), s.cwd.clone(), run.started_at))
                })
                .collect();
            let claimed: std::collections::HashSet<String> = inner
                .state
                .sessions
                .iter()
                .filter_map(|s| s.conversation.clone())
                .collect();
            (pending, claimed)
        };
        let Some(oldest) = pending.iter().map(|p| p.2).min() else {
            return;
        };
        let home = launch::default_home(Provider::Codex, &self.cfg.user_home);
        let mut found: Vec<(u64, String, String)> = vec![];
        for log in crate::services::usage::codex_logs(&home, oldest.saturating_sub(5_000)) {
            let first = std::fs::File::open(&log)
                .ok()
                .and_then(|f| std::io::BufRead::lines(std::io::BufReader::new(f)).next())
                .and_then(|l| l.ok());
            let Some(line) = first else { continue };
            if let (Some((cwd, started)), Some(id)) = (
                crate::core::usage::codex_meta(&line),
                crate::core::usage::codex_session_id(&line),
            ) {
                found.push((started, cwd, id));
            }
        }
        found.sort();
        let mut assigned = vec![];
        for (session, cwd, run_start) in pending {
            let cwd = canonical(&cwd);
            if let Some((_, _, id)) = found.iter().find(|(started, c, id)| {
                canonical(c) == cwd && *started + 5_000 >= run_start && !claimed.contains(id)
            }) {
                claimed.insert(id.clone());
                assigned.push((session, id.clone()));
            }
        }
        if assigned.is_empty() {
            return;
        }
        let mut inner = self.lock();
        for (session, id) in assigned {
            if let Some(run) = inner
                .state
                .runs
                .iter_mut()
                .rev()
                .find(|r| r.session == session)
            {
                run.conversation = Some(id.clone());
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == session) {
                s.conversation = Some(id);
            }
        }
        let _ = self.save(&inner);
    }

    // ----- settings -----

    pub fn settings(&self) -> crate::core::models::Settings {
        self.lock().state.settings.clone()
    }

    pub fn set_settings(
        &self,
        settings: crate::core::models::Settings,
    ) -> Result<crate::core::models::Settings> {
        if !(6.0..=40.0).contains(&settings.terminal.font_size) {
            bail!("terminal font size must be between 6 and 40");
        }
        if let Some(q) = settings
            .quick_commands
            .iter()
            .find(|q| q.name.trim().is_empty() || q.text.is_empty())
        {
            bail!("quick command needs a name and a text (got '{}')", q.name);
        }
        let mut inner = self.lock();
        inner.state.settings = settings.clone();
        self.save(&inner)?;
        Ok(settings)
    }

    // ----- apps (other PWAs and tools) -----

    pub fn clients(&self) -> Vec<Client> {
        self.lock().state.clients.clone()
    }

    pub fn client_by_token(&self, token: &str) -> Option<Client> {
        self.lock()
            .state
            .clients
            .iter()
            .find(|c| c.token == token)
            .cloned()
    }

    /// Register an app; its token is returned here and kept in state.json (mode 600).
    pub fn add_client(&self, name: &str, origins: Vec<String>, scope: Scope) -> Result<Client> {
        let name = name.trim();
        if name.is_empty() {
            bail!("the app needs a name");
        }
        let origins: Vec<String> = origins
            .iter()
            .map(|o| o.trim().trim_end_matches('/').to_string())
            .filter(|o| !o.is_empty())
            .collect();
        if let Some(bad) = origins
            .iter()
            .find(|o| *o != "*" && !(o.starts_with("http://") || o.starts_with("https://")))
        {
            bail!("origin must look like https://host[:port], got: {bad}");
        }
        let token = format!(
            "kimo_{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let client = Client {
            id: slug(name),
            name: name.to_string(),
            token,
            origins,
            scope,
            extra: Default::default(),
        };
        let mut inner = self.lock();
        inner.state.clients.push(client.clone());
        self.save(&inner)?;
        Ok(client)
    }

    pub fn delete_client(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        let before = inner.state.clients.len();
        inner.state.clients.retain(|c| c.id != id);
        if inner.state.clients.len() == before {
            bail!("unknown app: {id}");
        }
        self.save(&inner)
    }

    /// Restart kimo (to run a newly installed version): the running sessions are marked to
    /// start again, continuing their conversations, once the new kimo is up.
    pub fn request_restart(&self) -> Result<()> {
        let mut inner = self.lock();
        let running: Vec<String> = inner
            .live
            .iter()
            .filter(|(_, l)| l.runtime.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for spec in inner.state.sessions.iter_mut() {
            spec.resume_on_start = running.contains(&spec.id);
        }
        self.save(&inner)?;
        drop(inner);
        self.restart.notify_one();
        Ok(())
    }

    /// Resolves when a restart was requested.
    pub async fn restart_requested(&self) {
        self.restart.notified().await
    }

    /// Start the sessions that were running before a restart (they continue their
    /// conversations). Returns how many.
    pub fn resume_after_restart(&self) -> usize {
        let ids: Vec<String> = {
            let mut inner = self.lock();
            let ids = inner
                .state
                .sessions
                .iter()
                .filter(|s| s.resume_on_start)
                .map(|s| s.id.clone())
                .collect();
            for spec in inner.state.sessions.iter_mut() {
                spec.resume_on_start = false;
            }
            let _ = self.save(&inner);
            ids
        };
        ids.iter()
            .filter(|id| {
                self.start(id)
                    .map_err(|e| tracing::warn!("cannot start {id} again: {e:#}"))
                    .is_ok()
            })
            .count()
    }

    /// Stop every session (on shutdown) so no agent is left orphaned.
    pub async fn shutdown(&self) {
        let ids: Vec<String> = self
            .lock()
            .live
            .iter()
            .filter(|(_, l)| l.runtime.is_some())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.stop_and_wait(&id).await;
        }
    }
}
