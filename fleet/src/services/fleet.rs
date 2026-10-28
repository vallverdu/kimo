//! The coordinator: the machines of the registry and their power, the jobs and workflows,
//! and the loop that moves them on (every two seconds): look at the machines, carry out
//! what their power states ask for, place ready jobs (waking a machine when it is the best
//! one), follow running jobs to their end, and move workflows on. It talks to machines only
//! through their kimo's API with the fleet key; the machines know nothing of each other.

use crate::core::jobs::{self, Failure, JobSpec, JobState, Retry, Runner, Workflow};
use crate::core::power::{Action, DeviceState, Look, Policy, Power, Seen};
use crate::core::registry::{self, Device, Registry, Sleep, Wake};
use crate::core::schedule::{self, Candidate, Choice};
use crate::services::db::{Db, Event};
use crate::services::keys::Keys;
use crate::services::net;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// A machine that is about to get work is not switched off.
const SOON_MS: u64 = 30 * 60_000;
/// A job's machine is woken this long before the job may start.
const WAKE_AHEAD_MS: u64 = 3 * 60_000;
/// Facts are kept every this often.
const SAMPLE_EVERY_MS: u64 = 30_000;
/// A job on a machine that stopped answering is lost after this long.
const LOST_AFTER_MS: u64 = 180_000;
/// Artifacts not used by a kept job for this long are deleted.
const ARTIFACTS_FOR_MS: u64 = 30 * 86_400_000;

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    /// This coordinator's name for the machines (`pi4 (fleet)` in their device lists).
    pub host_name: String,
    /// Machines are looked at this often (ms)…
    pub look_every_ms: u64,
    /// …and this often while something happens to them (waking, switching off, jobs).
    pub look_fast_ms: u64,
    /// Ping a machine whose kimo does not answer (tells "booting" from "off").
    pub ping: bool,
    /// The port kimo answers on, on machines found on the network.
    pub kimo_port: u16,
    /// The SSH program for adding machines (keys or Tailscale SSH; never passwords).
    pub ssh: String,
    /// Programs that list the networks' machines (default: found on the PATH).
    pub tailscale: Option<String>,
    pub zerotier_cli: Option<String>,
    /// Look for machines on ZeroTier too.
    pub zerotier: bool,
    /// Where new machines get kimo from, and which release (default: this coordinator's).
    pub install_url: String,
    pub kimo_version: Option<String>,
}

impl Config {
    pub fn new(data_dir: PathBuf, host_name: String) -> Self {
        Self {
            data_dir,
            host_name,
            look_every_ms: 30_000,
            look_fast_ms: 5_000,
            ping: true,
            kimo_port: 7777,
            ssh: "ssh".into(),
            tailscale: None,
            zerotier_cli: None,
            zerotier: true,
            install_url: crate::core::setup::INSTALL_URL.into(),
            kimo_version: Some(format!("v{}", env!("CARGO_PKG_VERSION"))),
        }
    }
}

/// A file a job made, kept by the coordinator under its SHA-256.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub sha256: String,
    pub bytes: u64,
}

/// A job and its life on the machines.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub spec: JobSpec,
    pub state: JobState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workflow: Option<String>,
    /// The machine it is placed on (reserved while waking, then running there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Runs started so far.
    #[serde(default)]
    pub attempt: u32,
    /// Its id on the machine's kimo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_job: Option<String>,
    /// Machines it failed on through no fault of its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclude: Vec<String>,
    /// Retried on this machine first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_on: Option<String>,
    /// Why it waits, or why it failed, in plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Its machine stopped answering then (it is lost after a while).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreachable_since: Option<u64>,
    /// A headless agent's answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Its outputs: path → artifact.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artifacts: BTreeMap<String, Artifact>,
    /// The current attempt reached its machine (the next try is a new attempt).
    #[serde(default)]
    pub attempt_started: bool,
    /// You cancelled it; its machine has not confirmed the run ended yet (its lease stays).
    #[serde(default)]
    pub cancel_requested: bool,
    /// Earlier runs that may still be alive on their machines (a machine that stopped
    /// answering, an answer that never came): cancelled when their machine answers, their
    /// GPU and slots held until then.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphans: Vec<Orphan>,
    /// What an approval shows (`show:`): the earlier steps' outputs it names.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<Evidence>,
}

/// An output of an earlier step, shown with an approval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub path: String,
    /// The step and job that made it (`None`: no step made it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// A run of a job that may still be alive on a machine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Orphan {
    pub device: String,
    pub worker_job: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// A workflow started from its YAML: one job per step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: String,
    pub name: String,
    pub state: RunState,
    /// Step name → job id, in the workflow's order.
    pub steps: Vec<(String, String)>,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// A machine as the API shows it.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub kimo: String,
    /// The registry's labels and those the machine reports.
    pub labels: Vec<String>,
    pub registry: Device,
    /// `manual` while you have control, else the power state.
    pub state: &'static str,
    pub power: Power,
    pub since: u64,
    pub manual: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_on_until: Option<u64>,
    pub never_auto_shutdown: bool,
    /// When it will be switched off if it stays idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    pub paired: bool,
    pub can_wake: bool,
    pub auto_shutdown: bool,
    pub boots_today: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Its kimo's `/api/node`, from the last look.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub facts: Option<Value>,
    /// Jobs placed on it.
    pub jobs: Vec<String>,
    /// Added from the Fleet tab (`devices.d/`): it can be removed from there too.
    pub added: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Hold {
    /// Keep it on for this many hours (0 ends it).
    #[serde(default)]
    pub hours: Option<f64>,
    /// Never switch it off for being idle.
    #[serde(default)]
    pub never: Option<bool>,
    /// Take control: nothing automatic.
    #[serde(default)]
    pub manual: Option<bool>,
}

#[derive(Default)]
struct Inner {
    registry: Registry,
    registry_error: Option<String>,
    states: BTreeMap<String, DeviceState>,
    facts: BTreeMap<String, Value>,
    looked: BTreeMap<String, u64>,
    /// Machines being looked at right now.
    looking: std::collections::BTreeSet<String>,
    /// Jobs being followed right now.
    following: std::collections::BTreeSet<String>,
    /// Jobs being sent to their machine right now (their outcome is not known yet).
    dispatching: std::collections::BTreeSet<String>,
    sampled: BTreeMap<String, u64>,
    errors: BTreeMap<String, String>,
    keys: BTreeMap<String, String>,
    jobs: BTreeMap<String, Job>,
    workflows: BTreeMap<String, WorkflowRun>,
    pruned_at: u64,
}

pub struct Fleet {
    pub cfg: Config,
    db: Arc<Db>,
    keys: Keys,
    inner: Mutex<Inner>,
    /// Machines being added (installing, pairing), and how that went.
    adoptions: Mutex<BTreeMap<String, adopt::Adoption>>,
}

#[path = "adopt.rs"]
pub mod adopt;
#[path = "discover.rs"]
pub mod discover;
#[path = "runs.rs"]
mod runs;

fn policy(d: &Device) -> Policy {
    Policy {
        can_wake: d.power.wake == Wake::Wol,
        auto_shutdown: d.power.sleep == Sleep::Shutdown,
        idle_ms: (d.power.idle_minutes.max(0.0) * 60_000.0) as u64,
        boot_ms: d.power.boot_deadline_s * 1000,
        boots_per_day: d.power.boots_per_day,
    }
}

fn short_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..10]
    )
}

/// Facts that say the machine is working on something of its own, or someone uses it: what
/// its kimo would refuse to switch off for.
fn busy_there(facts: &Value) -> bool {
    facts["in_use"] == true || facts["sessions"]["working"].as_u64().unwrap_or(0) > 0
}

/// Do the facts show what the registry promises (the GPUs are visible)?
fn facts_ready(d: &Device, facts: &Value) -> bool {
    let gpus = facts["gpus"].as_array().map_or(0, Vec::len) as u32;
    d.resources.gpu == 0 || gpus >= d.resources.gpu || facts["jetson_l4t"].is_string()
}

/// What one look at a machine found.
enum Seen2 {
    Kimo {
        facts: Option<Value>,
        error: Option<String>,
    },
    Ping,
    Nothing(String),
}

fn look_at(url: &str, key: Option<&str>, host: &str, ping: bool) -> Seen2 {
    let client = net::kimo(url, key);
    match client.raw("GET", "/status", vec![], None, net::CALL_SECS) {
        Ok((200, _, _)) => {}
        Ok((401, _, _)) => {
            return Seen2::Kimo {
                facts: None,
                error: Some(if key.is_some() {
                    "its kimo does not accept the coordinator's key any more: pair it again".into()
                } else {
                    "not paired yet: run `kimo pair` there, then pair it here with the code".into()
                }),
            }
        }
        Ok((status, _, _)) => {
            return Seen2::Kimo {
                facts: None,
                error: Some(format!("its kimo answered {status}")),
            }
        }
        Err(e) => {
            return if ping && net::ping(host) {
                Seen2::Ping
            } else {
                Seen2::Nothing(format!("{e:#}"))
            }
        }
    }
    match client.call_timeout("GET", "/node", None, net::CALL_SECS) {
        Ok(facts) => Seen2::Kimo {
            facts: Some(facts),
            error: None,
        },
        Err(e) => Seen2::Kimo {
            facts: None,
            error: Some(format!(
                "no facts from its kimo ({e:#}): update kimo there to 0.1.15 or later"
            )),
        },
    }
}

impl Fleet {
    pub fn open(cfg: Config) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&cfg.data_dir)
            .with_context(|| format!("creating {}", cfg.data_dir.display()))?;
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&cfg.data_dir, std::fs::Permissions::from_mode(0o700));
        }
        for dir in ["logs", "artifacts"] {
            std::fs::create_dir_all(cfg.data_dir.join(dir))?;
        }
        let registry_path = cfg.data_dir.join("devices.yaml");
        if !registry_path.exists() {
            std::fs::write(&registry_path, registry::EXAMPLE)?;
        }
        let db = Arc::new(Db::open(&cfg.data_dir.join("fleet.db"))?);
        let keys = Keys::new(&cfg.data_dir);
        let mut inner = Inner {
            keys: keys.load(),
            ..Default::default()
        };
        for (id, state) in db.device_states::<DeviceState>() {
            inner.states.insert(id, state);
        }
        for mut job in db.jobs::<Job>() {
            // About to be sent when the coordinator stopped (no run written down yet): placed
            // again. One with a run written down is checked with its machine (runs.rs).
            if job.state == JobState::Dispatched && job.worker_job.is_none() {
                job.state = JobState::Ready;
                job.device = None;
            }
            inner.jobs.insert(job.id.clone(), job);
        }
        for w in db.workflows::<WorkflowRun>() {
            inner.workflows.insert(w.id.clone(), w);
        }
        let fleet = Arc::new(Self {
            cfg,
            db,
            keys,
            inner: Mutex::new(inner),
            adoptions: Mutex::new(BTreeMap::new()),
        });
        fleet.reload()?;
        Ok(fleet)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn registry_path(&self) -> PathBuf {
        self.cfg.data_dir.join("devices.yaml")
    }

    /// Machines added from the Fleet tab, one file each.
    pub fn added_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("devices.d")
    }

    /// Read `devices.yaml` (and `devices.d/`) again. On a mistake the machines stay as they were.
    pub fn reload(&self) -> Result<()> {
        let text = std::fs::read_to_string(self.registry_path()).unwrap_or_default();
        let mut added: Vec<(String, String)> = std::fs::read_dir(self.added_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .filter_map(|p| {
                Some((
                    format!("devices.d/{}", p.file_name()?.to_string_lossy()),
                    std::fs::read_to_string(&p).ok()?,
                ))
            })
            .collect();
        added.sort();
        let mut inner = self.lock();
        match registry::merge(&text, &added) {
            Ok(r) => {
                inner.registry = r;
                inner.registry_error = None;
                // Look at new or changed machines now.
                inner.looked.clear();
                Ok(())
            }
            Err(e) => {
                inner.registry_error = Some(e.clone());
                Err(anyhow!(e))
            }
        }
    }

    pub fn registry_error(&self) -> Option<String> {
        self.lock().registry_error.clone()
    }

    fn event(&self, device: Option<&str>, job: Option<&str>, kind: &str, detail: &str) {
        tracing::info!(
            "{}{}{kind}: {detail}",
            device.map(|d| format!("{d} ")).unwrap_or_default(),
            job.map(|j| format!("{j} ")).unwrap_or_default()
        );
        self.db.event(now_ms(), device, job, kind, detail);
    }

    fn notify(&self, device: Option<&str>, job: Option<&str>, text: &str) {
        self.event(device, job, "notice", text);
        let Some(url) = self.lock().registry.ntfy.clone() else {
            return;
        };
        let title = match (device, job) {
            (Some(d), _) => format!("kimo fleet: {d}"),
            (None, Some(j)) => format!("kimo fleet: {j}"),
            _ => "kimo fleet".into(),
        };
        // In the background; a notification that did not arrive is recorded as an event.
        let (db, text) = (self.db.clone(), text.to_string());
        let (device, job) = (device.map(str::to_string), job.map(str::to_string));
        std::thread::spawn(move || {
            if let Err(e) = net::notify(&url, &title, &text) {
                tracing::warn!("notification not delivered: {e:#}");
                db.event(
                    now_ms(),
                    device.as_deref(),
                    job.as_deref(),
                    "notice",
                    &format!("notification not delivered: {e:#}"),
                );
            }
        });
    }

    pub fn events(&self, device: Option<&str>, job: Option<&str>, limit: usize) -> Vec<Event> {
        self.db.events(device, job, limit.clamp(1, 1000))
    }

    pub fn samples(&self, device: &str, hours: f64) -> Vec<(u64, Value)> {
        let since = now_ms().saturating_sub((hours.max(0.0) * 3_600_000.0) as u64);
        self.db.samples(device, since)
    }

    fn save_state(&self, id: &str, state: &DeviceState) {
        self.db.put_device(id, state);
    }

    fn save_job(&self, job: &Job) {
        let state = serde_json::to_value(job.state).unwrap_or_default();
        if let Err(e) = self
            .db
            .put_job(&job.id, job.created_at, state.as_str().unwrap_or(""), job)
        {
            tracing::warn!("cannot save job {}: {e:#}", job.id);
        }
    }

    // ----- machines -----

    fn view(inner: &Inner, id: &str, d: &Device, now: u64) -> DeviceView {
        let st = inner.states.get(id).cloned().unwrap_or_default();
        let facts = inner.facts.get(id).cloned();
        let mut labels = d.all_labels();
        if let Some(f) = &facts {
            for l in f["labels"].as_array().into_iter().flatten() {
                if let Some(l) = l.as_str() {
                    if !labels.iter().any(|x| x == l) {
                        labels.push(l.to_string());
                    }
                }
            }
        }
        let p = policy(d);
        DeviceView {
            id: id.to_string(),
            kimo: d.kimo.clone(),
            labels,
            registry: d.clone(),
            state: st.shown(),
            power: st.power,
            since: st.since,
            manual: st.manual,
            keep_on_until: st.keep_on_until.filter(|t| *t > now),
            never_auto_shutdown: st.never_auto_shutdown,
            shutdown_at: (st.power == Power::Idle && !st.held(now))
                .then(|| st.idle_since.map(|t| t + p.idle_ms))
                .flatten(),
            last_seen: st.last_seen,
            paired: inner.keys.contains_key(id),
            can_wake: p.can_wake,
            auto_shutdown: p.auto_shutdown,
            boots_today: st.boots_today(now),
            error: inner.errors.get(id).cloned(),
            facts,
            jobs: inner
                .jobs
                .values()
                .filter(|j| j.state.on_device() && j.device.as_deref() == Some(id))
                .map(|j| j.id.clone())
                .collect(),
            added: false,
        }
    }

    pub fn devices(&self) -> Vec<DeviceView> {
        let inner = self.lock();
        let now = now_ms();
        inner
            .registry
            .devices
            .iter()
            .map(|(id, d)| self.mark_added(Self::view(&inner, id, d, now)))
            .collect()
    }

    fn mark_added(&self, mut v: DeviceView) -> DeviceView {
        v.added = self.added_dir().join(format!("{}.yaml", v.id)).exists();
        v
    }

    pub fn device(&self, id: &str) -> Result<DeviceView> {
        let inner = self.lock();
        let d = inner
            .registry
            .devices
            .get(id)
            .ok_or_else(|| anyhow!("unknown machine: {id}"))?;
        Ok(self.mark_added(Self::view(&inner, id, d, now_ms())))
    }

    fn device_of(&self, id: &str) -> Result<(Device, Option<String>)> {
        let inner = self.lock();
        let d = inner
            .registry
            .devices
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown machine: {id}"))?;
        Ok((d, inner.keys.get(id).cloned()))
    }

    /// Pair with a machine's kimo using a code shown there (`kimo pair`): the coordinator gets
    /// a fleet key (facts, jobs, power); that kimo gets nothing for the coordinator.
    pub fn pair(&self, id: &str, code: &str) -> Result<()> {
        let (d, _) = self.device_of(id)?;
        let token = self
            .pair_at(&d.kimo, code)
            .with_context(|| format!("pairing with {id} at {}", d.kimo))?;
        self.keep_key(id, token)
    }

    /// Trade a pairing code shown by the kimo at `url` for a fleet key. A kimo too old to know
    /// fleet keys would make an owner key of it: that key revokes itself and pairing stops.
    fn pair_at(&self, url: &str, code: &str) -> Result<String> {
        let answer = net::kimo(url, None).call_timeout(
            "POST",
            "/pair",
            Some(&json!({
                "code": code.trim(),
                "name": format!("{} (fleet)", self.cfg.host_name),
                "role": "fleet",
            })),
            10,
        )?;
        let token = answer["token"]
            .as_str()
            .ok_or_else(|| anyhow!("its kimo did not give a key"))?
            .to_string();
        if !token.starts_with("kimo_fleet_") {
            if let Some(device) = answer["id"].as_str() {
                let _ = net::kimo(url, Some(&token)).call_timeout(
                    "DELETE",
                    &format!("/devices/{device}"),
                    None,
                    10,
                );
            }
            bail!("its kimo is too old for fleet keys: update kimo there (0.1.15 or later) and pair again");
        }
        Ok(token)
    }

    fn keep_key(&self, id: &str, token: String) -> Result<()> {
        let mut inner = self.lock();
        inner.keys.insert(id.to_string(), token);
        self.keys.save(&inner.keys)?;
        inner.looked.remove(id);
        inner.errors.remove(id);
        drop(inner);
        self.event(Some(id), None, "paired", "fleet key received");
        Ok(())
    }

    /// Forget the key for a machine (revoke it on the machine too, in its Devices list).
    pub fn forget_key(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        inner.keys.remove(id);
        self.keys.save(&inner.keys)?;
        drop(inner);
        self.event(Some(id), None, "paired", "fleet key forgotten");
        Ok(())
    }

    /// Wake a machine now (you, from the PWA or the CLI).
    pub fn wake(&self, id: &str) -> Result<DeviceView> {
        let (d, _) = self.device_of(id)?;
        let now = now_ms();
        let actions = {
            let mut inner = self.lock();
            let st = inner.states.entry(id.to_string()).or_default();
            let actions = st.wake(&policy(&d), now).map_err(|e| anyhow!(e))?;
            let st = st.clone();
            inner.looked.remove(id);
            self.save_state(id, &st);
            actions
        };
        self.carry_out(id, actions);
        self.device(id)
    }

    /// Switch a machine off now, if nothing runs there (its kimo checks too).
    pub fn shutdown(&self, id: &str) -> Result<DeviceView> {
        {
            let inner = self.lock();
            let st = inner.states.get(id).cloned().unwrap_or_default();
            if !(st.power.up() || st.power == Power::Booted) {
                bail!("{id} is not on");
            }
            let jobs = inner
                .jobs
                .values()
                .filter(|j| j.state.on_device() && j.device.as_deref() == Some(id))
                .count();
            if jobs > 0 {
                bail!("{jobs} job(s) run on {id}: cancel them first");
            }
        }
        self.switch_off(id, "asked")?;
        self.device(id)
    }

    /// Ask the machine's kimo to switch off (it re-checks that nothing runs).
    fn switch_off(&self, id: &str, why: &str) -> Result<()> {
        let (d, key) = self.device_of(id)?;
        let key = key.ok_or_else(|| anyhow!("{id} is not paired"))?;
        let client = net::kimo(&d.kimo, Some(&key));
        let (status, _, body) = client.raw(
            "POST",
            "/power/shutdown",
            b"{}".to_vec(),
            Some("application/json"),
            net::CALL_SECS,
        )?;
        let now = now_ms();
        let answer: Value = serde_json::from_slice(&body).unwrap_or_default();
        if status == 202 || status == 200 {
            let mut inner = self.lock();
            let st = inner.states.entry(id.to_string()).or_default();
            st.draining(now);
            let st = st.clone();
            inner.looked.remove(id);
            drop(inner);
            self.save_state(id, &st);
            self.event(Some(id), None, "power", &format!("switching off ({why})"));
            return Ok(());
        }
        let error = answer["error"]
            .as_str()
            .unwrap_or("its kimo refused")
            .to_string();
        {
            let mut inner = self.lock();
            let st = inner.states.entry(id.to_string()).or_default();
            st.refused(now);
            let st = st.clone();
            drop(inner);
            self.save_state(id, &st);
        }
        self.event(Some(id), None, "power", &format!("kept on: {error}"));
        bail!("{id}: {error}")
    }

    /// Keep a machine on for some hours, never switch it off, or take control of it.
    pub fn hold(&self, id: &str, hold: Hold) -> Result<DeviceView> {
        self.device_of(id)?;
        let now = now_ms();
        let mut said = vec![];
        {
            let mut inner = self.lock();
            let st = inner.states.entry(id.to_string()).or_default();
            if let Some(h) = hold.hours {
                st.keep_on_until = (h > 0.0).then(|| now + (h * 3_600_000.0) as u64);
                said.push(if h > 0.0 {
                    format!("kept on for {h} h")
                } else {
                    "no longer kept on".into()
                });
            }
            if let Some(n) = hold.never {
                st.never_auto_shutdown = n;
                said.push(
                    if n {
                        "never switched off"
                    } else {
                        "switched off when idle"
                    }
                    .into(),
                );
            }
            if let Some(m) = hold.manual {
                st.manual = m;
                said.push(
                    if m {
                        "manual control"
                    } else {
                        "automatic again"
                    }
                    .into(),
                );
            }
            let st = st.clone();
            inner.looked.remove(id);
            drop(inner);
            self.save_state(id, &st);
        }
        if !said.is_empty() {
            self.event(Some(id), None, "control", &said.join(", "));
        }
        self.device(id)
    }

    /// Carry out what a machine's power state asked for.
    fn carry_out(&self, id: &str, actions: Vec<Action>) {
        for a in actions {
            match a {
                Action::Wake => {
                    let Ok((d, _)) = self.device_of(id) else {
                        continue;
                    };
                    let sent = d
                        .mac
                        .as_deref()
                        .and_then(registry::parse_mac)
                        .ok_or_else(|| anyhow!("no MAC address"))
                        .and_then(|mac| {
                            let to = net::wol_target(d.broadcast.as_deref())?;
                            net::send_wol(mac, to)?;
                            Ok(to)
                        });
                    match sent {
                        Ok(to) => self.event(
                            Some(id),
                            None,
                            "power",
                            &format!("magic packet sent to {to}"),
                        ),
                        Err(e) => self.notify(Some(id), None, &format!("cannot wake it: {e:#}")),
                    }
                }
                Action::Shutdown => {
                    if let Err(e) = self.switch_off(id, "idle") {
                        tracing::info!("{e:#}");
                    }
                }
                Action::Notify(text) => self.notify(Some(id), None, &text),
                Action::Event(text) => self.event(Some(id), None, "power", &text),
            }
        }
    }

    /// Look at the machines that are due, each on a thread of its own, so a machine that does
    /// not answer holds up nothing else; each look moves that machine's power state on.
    fn look(self: &Arc<Self>, now: u64) {
        let mut inner = self.lock();
        let ids: Vec<String> = inner.registry.devices.keys().cloned().collect();
        for id in ids {
            if inner.looking.contains(&id) {
                continue;
            }
            let st = inner.states.get(&id).cloned().unwrap_or_default();
            let busy = inner
                .jobs
                .values()
                .any(|j| j.state.on_device() && j.device.as_deref() == Some(id.as_str()));
            let every = if busy
                || matches!(
                    st.power,
                    Power::Waking | Power::Draining | Power::Booted | Power::Unknown
                ) {
                self.cfg.look_fast_ms
            } else {
                self.cfg.look_every_ms
            };
            if inner.looked.get(&id).is_some_and(|t| now < t + every) {
                continue;
            }
            inner.looked.insert(id.clone(), now);
            inner.looking.insert(id.clone());
            let d = &inner.registry.devices[&id];
            let (url, key) = (d.kimo.clone(), inner.keys.get(&id).cloned());
            let host = registry::host_of(&d.kimo).unwrap_or_default();
            let (me, ping) = (self.clone(), self.cfg.ping);
            let spawned = std::thread::Builder::new()
                .name("fleet-look".into())
                .spawn(move || {
                    let seen = look_at(&url, key.as_deref(), &host, ping);
                    me.apply(&id, seen);
                    me.lock().looking.remove(&id);
                });
            if spawned.is_err() {
                tracing::warn!("cannot look at a machine: no thread");
            }
        }
    }

    /// What a look found: facts, errors, and the power state's next step (carried out).
    fn apply(&self, id: &str, seen: Seen2) {
        let now = now_ms();
        let actions = {
            let mut inner = self.lock();
            let Some(d) = inner.registry.devices.get(id).cloned() else {
                return;
            };
            let (seen, error) = match seen {
                Seen2::Kimo { facts, error } => {
                    let ready = facts.as_ref().is_some_and(|f| facts_ready(&d, f));
                    if let Some(f) = facts {
                        if inner
                            .sampled
                            .get(id)
                            .is_none_or(|t| now >= t + SAMPLE_EVERY_MS)
                        {
                            inner.sampled.insert(id.to_string(), now);
                            self.db.sample(now, id, &f);
                        }
                        inner.facts.insert(id.to_string(), f);
                    }
                    let error = error
                        .or_else(|| (!ready).then(|| "its GPU is not visible yet".to_string()));
                    (Seen::Kimo { ready }, error)
                }
                Seen2::Ping => (
                    Seen::Ping,
                    Some("it answers pings, its kimo does not".to_string()),
                ),
                Seen2::Nothing(e) => (Seen::Nothing, Some(format!("no answer: {e}"))),
            };
            match error {
                Some(e) => inner.errors.insert(id.to_string(), e),
                None => inner.errors.remove(id),
            };
            let leases = inner
                .jobs
                .values()
                .filter(|j| j.state.on_device() && j.device.as_deref() == Some(id))
                .count();
            let busy =
                seen == (Seen::Kimo { ready: true }) && inner.facts.get(id).is_some_and(busy_there);
            let soon = Self::work_soon(&inner, id, now);
            let st = inner.states.entry(id.to_string()).or_default();
            let before = st.clone();
            let actions = st.step(
                &policy(&d),
                &Look {
                    now,
                    seen,
                    leases,
                    busy_there: busy,
                    work_soon: soon,
                },
            );
            let after = st.clone();
            drop(inner);
            if after.power != before.power {
                self.event(
                    Some(id),
                    None,
                    "state",
                    &format!("{} → {}", before.power.name(), after.power.name()),
                );
            }
            if after != before {
                self.save_state(id, &after);
            }
            actions
        };
        self.carry_out(id, actions);
    }

    /// Could queued or blocked work go to this machine within the next half hour?
    fn work_soon(inner: &Inner, id: &str, now: u64) -> bool {
        let Some(c) = Self::candidates(inner, now)
            .into_iter()
            .find(|c| c.id == id)
        else {
            return false;
        };
        inner.jobs.values().any(|j| {
            matches!(
                j.state,
                JobState::Blocked | JobState::Ready | JobState::WaitingDevice
            ) && j.spec.runner != Runner::Approval
                && j.spec.not_before.is_none_or(|t| t <= now + SOON_MS)
                && schedule::qualifies(&j.spec, &c)
        })
    }

    fn candidates(inner: &Inner, now: u64) -> Vec<Candidate> {
        inner
            .registry
            .devices
            .iter()
            .map(|(id, d)| {
                let st = inner.states.get(id).cloned().unwrap_or_default();
                let facts = inner.facts.get(id);
                let mut labels = d.all_labels();
                if let Some(f) = facts {
                    for l in f["labels"].as_array().into_iter().flatten() {
                        if let Some(l) = l.as_str() {
                            labels.push(l.to_string());
                        }
                    }
                }
                // Its jobs, and earlier runs that may still be alive there (each holds what its
                // job asked for until its machine confirms it ended).
                let mut on: Vec<&Job> = inner
                    .jobs
                    .values()
                    .filter(|j| j.state.on_device() && j.device.as_deref() == Some(id.as_str()))
                    .collect();
                for j in inner.jobs.values() {
                    let n = j.orphans.iter().filter(|o| o.device == *id).count();
                    on.extend(std::iter::repeat_n(j, n));
                }
                Candidate {
                    id: id.clone(),
                    labels,
                    resources: d.resources.clone(),
                    memory_gb: facts
                        .and_then(|f| f["mem_total_bytes"].as_f64())
                        .map(|b| b / 1e9),
                    vram_gb: facts.and_then(|f| {
                        f["gpus"]
                            .as_array()?
                            .iter()
                            .filter_map(|g| g["memory_total_mb"].as_f64())
                            .reduce(f64::max)
                            .map(|mb| mb / 1024.0)
                    }),
                    used_gpu: on.iter().map(|j| j.spec.resources.gpu).sum(),
                    used_slots: on.iter().map(|j| j.spec.resources.cpu_slots()).sum(),
                    jobs: on.len(),
                    up: st.takes_work(),
                    wakeable: st.wakeable(&policy(d), now),
                    cost: d.cost,
                }
            })
            .collect()
    }

    // ----- jobs -----

    pub fn jobs(&self) -> Vec<Job> {
        let mut list: Vec<Job> = self.lock().jobs.values().cloned().collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        list
    }

    pub fn job(&self, id: &str) -> Result<Job> {
        self.lock()
            .jobs
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown job: {id}"))
    }

    fn new_job(spec: JobSpec, workflow: Option<String>, now: u64) -> Job {
        let state = if !spec.needs.is_empty() {
            JobState::Blocked
        } else if spec.runner == Runner::Approval {
            JobState::AwaitingApproval
        } else {
            JobState::Ready
        };
        Job {
            id: short_id("j"),
            spec,
            state,
            workflow,
            device: None,
            attempt: 0,
            worker_job: None,
            exclude: vec![],
            retry_on: None,
            reason: None,
            exit_code: None,
            created_at: now,
            updated_at: now,
            started_at: None,
            ended_at: None,
            unreachable_since: None,
            result: None,
            artifacts: BTreeMap::new(),
            attempt_started: false,
            cancel_requested: false,
            orphans: vec![],
            evidence: vec![],
        }
    }

    /// Queue one job (it needs no other steps).
    pub fn submit(&self, spec: JobSpec) -> Result<Job> {
        jobs::check(&spec).map_err(|e| anyhow!(e))?;
        if !spec.needs.is_empty() || !spec.inputs.is_empty() {
            bail!("needs and inputs are for workflow steps: submit a workflow");
        }
        let job = Self::new_job(spec, None, now_ms());
        self.lock().jobs.insert(job.id.clone(), job.clone());
        self.save_job(&job);
        self.event(
            None,
            Some(&job.id),
            "job",
            &format!("queued: {}", job.spec.task),
        );
        if job.state == JobState::AwaitingApproval {
            self.notify(
                None,
                Some(&job.id),
                &format!("{} waits for your approval", job.spec.task),
            );
        }
        Ok(job)
    }

    /// Start a workflow: one job per step, those without needs ready at once.
    pub fn submit_workflow(&self, w: Workflow) -> Result<WorkflowRun> {
        jobs::check_workflow(&w).map_err(|e| anyhow!(e))?;
        let now = now_ms();
        let run_id = short_id("w");
        let jobs: Vec<Job> = w
            .steps
            .iter()
            .map(|s| Self::new_job(s.clone(), Some(run_id.clone()), now))
            .collect();
        let run = WorkflowRun {
            id: run_id.clone(),
            name: w.workflow.clone(),
            state: RunState::Running,
            steps: jobs
                .iter()
                .map(|j| (j.spec.task.clone(), j.id.clone()))
                .collect(),
            created_at: now,
            ended_at: None,
        };
        {
            let mut inner = self.lock();
            for j in &jobs {
                inner.jobs.insert(j.id.clone(), j.clone());
            }
            inner.workflows.insert(run.id.clone(), run.clone());
        }
        for j in &jobs {
            self.save_job(j);
        }
        self.db.put_workflow(&run.id, run.created_at, &run)?;
        self.event(
            None,
            None,
            "workflow",
            &format!("{} started as {run_id}", w.workflow),
        );
        Ok(run)
    }

    pub fn workflows(&self) -> Vec<WorkflowRun> {
        let mut list: Vec<WorkflowRun> = self.lock().workflows.values().cloned().collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        list
    }

    pub fn workflow(&self, id: &str) -> Result<(WorkflowRun, Vec<Job>)> {
        let inner = self.lock();
        let run = inner
            .workflows
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown workflow: {id}"))?;
        let jobs = run
            .steps
            .iter()
            .filter_map(|(_, j)| inner.jobs.get(j).cloned())
            .collect();
        Ok((run, jobs))
    }

    fn update_job(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let mut inner = self.lock();
        let job = inner.jobs.get_mut(id)?;
        f(job);
        job.updated_at = now_ms();
        let job = job.clone();
        drop(inner);
        self.save_job(&job);
        Some(job)
    }

    pub fn cancel_workflow(&self, id: &str) -> Result<WorkflowRun> {
        let (run, jobs) = self.workflow(id)?;
        for j in jobs.iter().filter(|j| !j.state.finished()) {
            self.cancel(&j.id)?;
        }
        self.advance(now_ms());
        Ok(self.workflow(&run.id)?.0)
    }

    /// Approve (or reject) a step waiting for you.
    pub fn approve(&self, id: &str, yes: bool) -> Result<Job> {
        let job = self.job(id)?;
        if job.state != JobState::AwaitingApproval {
            bail!("{} does not wait for an approval", job.spec.task);
        }
        let job = self
            .update_job(id, |j| {
                j.state = if yes {
                    JobState::Succeeded
                } else {
                    JobState::Failed
                };
                j.reason = Some(if yes { "approved" } else { "rejected" }.into());
                j.ended_at = Some(now_ms());
            })
            .ok_or_else(|| anyhow!("unknown job: {id}"))?;
        self.event(
            None,
            Some(id),
            "approval",
            if yes { "approved" } else { "rejected" },
        );
        self.advance(now_ms());
        Ok(job)
    }

    fn log_path(&self, id: &str) -> PathBuf {
        self.cfg.data_dir.join("logs").join(format!("{id}.log"))
    }

    /// The job's log from `offset`: from its machine while it runs, kept here once it ended.
    pub fn job_log(&self, id: &str, offset: u64) -> Result<Value> {
        let job = self.job(id)?;
        if job.state.on_device() {
            if let (Some(dev), Some(wid)) = (&job.device, &job.worker_job) {
                let (d, key) = self.device_of(dev)?;
                return net::kimo(&d.kimo, key.as_deref()).call_timeout(
                    "GET",
                    &format!("/jobs/{wid}/log?offset={offset}"),
                    None,
                    net::CALL_SECS,
                );
            }
        }
        let path = self.log_path(id);
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let offset = offset.min(size);
        let mut text = Vec::new();
        if let Ok(mut f) = std::fs::File::open(&path) {
            use std::io::{Seek, SeekFrom};
            f.seek(SeekFrom::Start(offset))?;
            f.take(256 << 10).read_to_end(&mut text)?;
        }
        let next = offset + text.len() as u64;
        Ok(json!({
            "offset": offset,
            "next": next,
            "size": size,
            "text": String::from_utf8_lossy(&text),
            "done": job.state.finished() && next >= size,
        }))
    }

    pub fn artifact_path(&self, sha256: &str) -> Option<PathBuf> {
        (sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| self.cfg.data_dir.join("artifacts").join(sha256))
            .filter(|p| p.is_file())
    }

    /// Move blocked steps on, approvals to waiting, and finished workflows to their end.
    fn advance(&self, now: u64) {
        let mut changed = vec![];
        let mut notices = vec![];
        let mut ended = vec![];
        {
            let mut inner = self.lock();
            let runs: Vec<WorkflowRun> = inner
                .workflows
                .values()
                .filter(|w| w.state == RunState::Running)
                .cloned()
                .collect();
            for run in runs {
                let by_task: BTreeMap<&str, &str> = run
                    .steps
                    .iter()
                    .map(|(t, j)| (t.as_str(), j.as_str()))
                    .collect();
                for (_, jid) in &run.steps {
                    let Some(job) = inner.jobs.get(jid).cloned() else {
                        continue;
                    };
                    if job.state != JobState::Blocked {
                        continue;
                    }
                    let needs: Vec<(JobState, bool)> = job
                        .spec
                        .needs
                        .iter()
                        .filter_map(|n| inner.jobs.get(*by_task.get(n.as_str())?))
                        .map(|j| (j.state, j.spec.optional))
                        .collect();
                    let next = jobs::unblocked(&needs);
                    if next == JobState::Blocked {
                        continue;
                    }
                    let missing = (next == JobState::Ready)
                        .then(|| Self::missing_input(&inner, &run, jid))
                        .flatten();
                    let evidence = Self::evidence(&inner, &run, jid);
                    let j = inner.jobs.get_mut(jid).expect("job");
                    j.updated_at = now;
                    if next == JobState::Skipped {
                        j.state = JobState::Skipped;
                        j.ended_at = Some(now);
                        j.reason = Some("a step it needs did not succeed".into());
                    } else if let Some(missing) = missing {
                        // Never run without what it needs (audit F3).
                        j.state = JobState::Failed;
                        j.ended_at = Some(now);
                        j.reason = Some(missing);
                        notices.push((
                            j.id.clone(),
                            format!(
                                "{} failed: {}",
                                j.spec.task,
                                j.reason.clone().unwrap_or_default()
                            ),
                        ));
                    } else if j.spec.runner == Runner::Approval {
                        j.state = JobState::AwaitingApproval;
                        j.evidence = evidence.clone();
                        notices.push((
                            j.id.clone(),
                            format!("{} waits for your approval", j.spec.task),
                        ));
                    } else {
                        j.state = JobState::Ready;
                    }
                    changed.push(j.clone());
                }
                let states: Vec<&Job> = run
                    .steps
                    .iter()
                    .filter_map(|(_, j)| inner.jobs.get(j))
                    .collect();
                if states.iter().all(|j| j.state.finished()) {
                    let ok = states
                        .iter()
                        .all(|j| j.state == JobState::Succeeded || j.spec.optional);
                    let cancelled = states.iter().any(|j| j.state == JobState::Cancelled);
                    let w = inner.workflows.get_mut(&run.id).expect("run");
                    w.state = if ok {
                        RunState::Succeeded
                    } else if cancelled {
                        RunState::Cancelled
                    } else {
                        RunState::Failed
                    };
                    w.ended_at = Some(now);
                    ended.push(w.clone());
                }
            }
        }
        for j in &changed {
            self.save_job(j);
            self.event(
                None,
                Some(&j.id),
                "job",
                &format!("{}: {:?}", j.spec.task, j.state),
            );
        }
        for (id, text) in notices {
            self.notify(None, Some(&id), &text);
        }
        for w in ended {
            let _ = self.db.put_workflow(&w.id, w.created_at, &w);
            self.notify(
                None,
                None,
                &format!("workflow {} {:?}", w.name, w.state).to_lowercase(),
            );
        }
    }

    /// What an approval names in `show:`, found among the outputs of the workflow's steps
    /// (the latest step that made a path wins).
    fn evidence(inner: &Inner, run: &WorkflowRun, jid: &str) -> Vec<Evidence> {
        let Some(job) = inner.jobs.get(jid) else {
            return vec![];
        };
        job.spec
            .show
            .iter()
            .map(|path| {
                let made = run.steps.iter().rev().find_map(|(task, id)| {
                    let j = inner.jobs.get(id)?;
                    j.artifacts
                        .get(path)
                        .map(|a| (task.clone(), id.clone(), a.bytes))
                });
                Evidence {
                    path: path.clone(),
                    step: made.as_ref().map(|m| m.0.clone()),
                    job: made.as_ref().map(|m| m.1.clone()),
                    bytes: made.map(|m| m.2),
                }
            })
            .collect()
    }

    /// The first input of a step that its producer did not leave, in plain words.
    fn missing_input(inner: &Inner, run: &WorkflowRun, jid: &str) -> Option<String> {
        let job = inner.jobs.get(jid)?;
        job.spec.inputs.values().find_map(|from| {
            let (step, path) = from.split_once('/')?;
            let producer = run
                .steps
                .iter()
                .find(|(t, _)| t == step)
                .and_then(|(_, id)| inner.jobs.get(id));
            match producer {
                Some(p) if p.artifacts.contains_key(path) => None,
                Some(p) if p.state != JobState::Succeeded => Some(format!(
                    "its input {from} is not available: {step} did not succeed"
                )),
                _ => Some(format!(
                    "its input {from} is not available: {step} did not make it"
                )),
            }
        })
    }

    /// Place ready jobs: send them to a machine that is up, or wake the best one for them.
    fn schedule(self: &Arc<Self>, now: u64) {
        let mut dispatch = vec![];
        let mut wake = vec![];
        let mut saved = vec![];
        let mut waiting = vec![];
        {
            let mut inner = self.lock();
            let mut machines = Self::candidates(&inner, now);
            let mut order: Vec<(u64, String)> = inner
                .jobs
                .values()
                .filter(|j| {
                    matches!(
                        j.state,
                        JobState::Ready | JobState::WaitingDevice | JobState::Waking
                    )
                })
                .map(|j| (j.created_at, j.id.clone()))
                .collect();
            order.sort();
            for (_, id) in order {
                let job = inner.jobs[&id].clone();
                let may_start = job.spec.not_before.is_none_or(|t| now >= t);
                if job.state == JobState::Waking {
                    let dev = job.device.clone().unwrap_or_default();
                    let st = inner.states.get(&dev).cloned().unwrap_or_default();
                    if st.takes_work() && may_start {
                        let j = inner.jobs.get_mut(&id).expect("job");
                        j.state = JobState::Dispatched;
                        j.worker_job = None;
                        j.updated_at = now;
                        saved.push(j.clone());
                        dispatch.push(id);
                    } else if !matches!(st.power, Power::Waking | Power::Booted) && !st.power.up() {
                        // The wake failed: place it again.
                        let j = inner.jobs.get_mut(&id).expect("job");
                        j.state = JobState::Ready;
                        j.device = None;
                        j.reason = Some(format!("{dev} did not wake up"));
                        j.updated_at = now;
                        saved.push(j.clone());
                    }
                    continue;
                }
                if job.spec.not_before.is_some_and(|t| now + WAKE_AHEAD_MS < t) {
                    continue;
                }
                let mut spec = job.spec.clone();
                if let Some(dev) = &job.retry_on {
                    spec.prefer = Some(dev.clone());
                }
                let choice = schedule::choose(&spec, &machines, &job.exclude);
                let take = |machines: &mut Vec<Candidate>, dev: &str| {
                    if let Some(c) = machines.iter_mut().find(|c| c.id == dev) {
                        c.used_gpu += spec.resources.gpu;
                        c.used_slots += spec.resources.cpu_slots();
                        c.jobs += 1;
                    }
                };
                let j = inner.jobs.get_mut(&id).expect("job");
                match choice {
                    Choice::Run(dev) if may_start => {
                        take(&mut machines, &dev);
                        j.state = JobState::Dispatched;
                        j.worker_job = None;
                        j.device = Some(dev);
                        j.reason = None;
                        j.updated_at = now;
                        saved.push(j.clone());
                        dispatch.push(id);
                    }
                    Choice::Run(_) => {}
                    Choice::Wake(dev) => {
                        take(&mut machines, &dev);
                        if let Some(c) = machines.iter_mut().find(|c| c.id == dev) {
                            c.wakeable = false;
                        }
                        j.state = JobState::Waking;
                        j.device = Some(dev.clone());
                        j.reason = Some(format!("waking {dev}"));
                        j.updated_at = now;
                        saved.push(j.clone());
                        wake.push(dev);
                    }
                    Choice::Wait(why) => {
                        if j.state != JobState::WaitingDevice || j.reason.as_deref() != Some(&why) {
                            if j.state != JobState::WaitingDevice {
                                waiting.push((id.clone(), format!("{} waits: {why}", j.spec.task)));
                            }
                            j.state = JobState::WaitingDevice;
                            j.reason = Some(why);
                            j.updated_at = now;
                            saved.push(j.clone());
                        }
                    }
                }
            }
        }
        for j in &saved {
            self.save_job(j);
        }
        for (id, text) in waiting {
            self.notify(None, Some(&id), &text);
        }
        for dev in wake {
            let actions = {
                let mut inner = self.lock();
                let Some(d) = inner.registry.devices.get(&dev).cloned() else {
                    continue;
                };
                let st = inner.states.entry(dev.clone()).or_default();
                let actions = st.wake(&policy(&d), now).unwrap_or_default();
                let st = st.clone();
                inner.looked.remove(&dev);
                drop(inner);
                self.save_state(&dev, &st);
                actions
            };
            self.event(Some(&dev), None, "power", "woken for work");
            self.carry_out(&dev, actions);
        }
        // Each on a thread of its own: a slow machine holds up nothing else.
        for id in dispatch {
            let me = self.clone();
            let _ = std::thread::Builder::new()
                .name("fleet-dispatch".into())
                .spawn(move || me.dispatch(&id));
        }
    }

    fn step_artifact(&self, workflow: Option<&str>, step: &str, path: &str) -> Option<Artifact> {
        let inner = self.lock();
        let run = inner.workflows.get(workflow?)?;
        let (_, jid) = run.steps.iter().find(|(t, _)| t == step)?;
        inner.jobs.get(jid)?.artifacts.get(path).cloned()
    }

    fn upload(&self, url: &str, key: &str, wid: &str, path: &str, art: &Artifact) -> Result<()> {
        let file = std::fs::File::open(
            self.artifact_path(&art.sha256)
                .ok_or_else(|| anyhow!("artifact {} is gone", art.sha256))?,
        )?;
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!(
            "{}/api/jobs/{wid}/files?path={}",
            url.trim_end_matches('/'),
            encode(path)
        );
        let mut res = agent
            .put(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .header("Content-Type", "application/octet-stream")
            .send(&file)?;
        if !res.status().is_success() {
            let text = res.body_mut().read_to_string().unwrap_or_default();
            bail!("{} {}", res.status(), text);
        }
        Ok(())
    }

    /// Fetch an output of a job from its machine into the artifact store (streamed, hashed).
    fn download(&self, url: &str, key: &str, wid: &str, path: &str) -> Result<Artifact> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .build()
            .into();
        let url = format!(
            "{}/api/jobs/{wid}/files?path={}",
            url.trim_end_matches('/'),
            encode(path)
        );
        let res = agent
            .get(&url)
            .header("Authorization", &format!("Bearer {key}"))
            .call()?;
        if !res.status().is_success() {
            bail!("{path} was not made ({})", res.status());
        }
        let dir = self.cfg.data_dir.join("artifacts");
        let tmp = dir.join(format!(".part-{}", uuid::Uuid::new_v4().simple()));
        let mut out = std::fs::File::create(&tmp)?;
        let mut reader = res.into_body().into_reader();
        let mut hash = kimo::core::sha256::Sha256::new();
        let mut buf = vec![0u8; 256 << 10];
        let mut bytes = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            bytes += n as u64;
        }
        out.sync_all()?;
        let sha256 = kimo::core::sha256::to_hex(&hash.finish());
        let dest = dir.join(&sha256);
        if dest.exists() {
            let _ = std::fs::remove_file(&tmp);
            // Used again: kept longer.
            let _ = std::fs::File::options().append(true).open(&dest);
        } else {
            std::fs::rename(&tmp, &dest)?;
        }
        Ok(Artifact { sha256, bytes })
    }

    /// Keep what a run left: its log, a headless agent's answer and its outputs.
    fn collect(&self, job: &Job, url: &str, key: &str, wid: &str, worker: &Value) {
        let client = net::kimo(url, Some(key));
        let mut log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.log_path(&job.id))
            .ok();
        if let Some(f) = log.as_mut() {
            let _ = writeln!(
                f,
                "── {} on {} (attempt {}) ──",
                job.spec.task,
                job.device.as_deref().unwrap_or("?"),
                job.attempt
            );
        }
        let mut offset = 0u64;
        // At most 8 MB of log is kept here.
        while offset < 8 << 20 {
            let Ok(chunk) = client.call_timeout(
                "GET",
                &format!("/jobs/{wid}/log?offset={offset}"),
                None,
                net::CALL_SECS * 2,
            ) else {
                break;
            };
            let text = chunk["text"].as_str().unwrap_or("");
            if let Some(f) = log.as_mut() {
                let _ = f.write_all(text.as_bytes());
            }
            let next = chunk["next"].as_u64().unwrap_or(offset);
            if next <= offset || chunk["done"] == true {
                break;
            }
            offset = next;
        }
        let mut artifacts = BTreeMap::new();
        if worker["state"] == "succeeded" {
            for path in &job.spec.outputs {
                match self.download(url, key, wid, path) {
                    Ok(a) => {
                        artifacts.insert(path.clone(), a);
                    }
                    Err(e) => self.event(
                        job.device.as_deref(),
                        Some(&job.id),
                        "job",
                        &format!("output {path}: {e:#}"),
                    ),
                }
            }
        }
        let result = worker.get("result").cloned();
        self.update_job(&job.id, |j| {
            j.artifacts.extend(artifacts);
            if result.is_some() {
                j.result = result;
            }
        });
    }

    /// A run ended: succeeded, or failed and retried (where it ran, or elsewhere), or failed.
    fn end(&self, job: &Job, code: Option<i32>, failure: Option<Failure>, why: &str) {
        let now = now_ms();
        // Cancelled (or moved on) meanwhile: what its machine says no longer counts.
        let Ok(current) = self.job(&job.id) else {
            return;
        };
        if !matches!(current.state, JobState::Running | JobState::Dispatched) {
            return;
        }
        let dev = job.device.clone();
        let missing: Vec<String> = job
            .spec
            .outputs
            .iter()
            .filter(|o| {
                failure.is_none()
                    && !self
                        .job(&job.id)
                        .is_ok_and(|j| j.artifacts.contains_key(*o))
            })
            .cloned()
            .collect();
        let (failure, why) = if failure.is_none() && !missing.is_empty() {
            (
                Some(Failure::Job),
                format!("did not make {}", missing.join(", ")),
            )
        } else {
            (failure, why.to_string())
        };
        let cancelled = why.starts_with("cancelled") || current.cancel_requested;
        let next = match failure {
            None => None,
            Some(_) if cancelled => Some(Retry::No),
            Some(f) => Some(jobs::retry(job.attempt.max(1), job.spec.retries, f)),
        };
        let updated = self.update_job(&job.id, |j| {
            j.exit_code = code;
            j.unreachable_since = None;
            match next {
                None => {
                    j.state = JobState::Succeeded;
                    j.ended_at = Some(now);
                    j.reason = None;
                    j.cancel_requested = false;
                }
                Some(Retry::Same) => {
                    j.state = JobState::Ready;
                    j.retry_on = dev.clone();
                    j.device = None;
                    j.reason = Some(format!("{why}: trying again"));
                }
                Some(Retry::Elsewhere) => {
                    j.state = JobState::Ready;
                    if let Some(d) = &dev {
                        if !j.exclude.contains(d) {
                            j.exclude.push(d.clone());
                        }
                    }
                    j.retry_on = None;
                    j.device = None;
                    j.reason = Some(format!("{why}: trying another machine"));
                }
                Some(Retry::No) => {
                    j.cancel_requested = false;
                    j.state = if cancelled {
                        JobState::Cancelled
                    } else {
                        JobState::Failed
                    };
                    j.ended_at = Some(now);
                    j.reason = Some(why.clone());
                }
            }
        });
        let Some(j) = updated else { return };
        match next {
            None => self.event(
                dev.as_deref(),
                Some(&j.id),
                "job",
                &format!("{} succeeded", j.spec.task),
            ),
            Some(Retry::No) if !cancelled => self.notify(
                dev.as_deref(),
                Some(&j.id),
                &format!("{} failed: {why}", j.spec.task),
            ),
            _ => self.event(
                dev.as_deref(),
                Some(&j.id),
                "job",
                j.reason.as_deref().unwrap_or(""),
            ),
        }
        self.advance(now);
    }

    /// Forget old events and samples, and artifacts no kept job refers to.
    fn prune(&self, now: u64) {
        {
            let mut inner = self.lock();
            if now < inner.pruned_at + 3_600_000 {
                return;
            }
            inner.pruned_at = now;
        }
        self.db.prune(now);
        let used: std::collections::BTreeSet<String> = self
            .lock()
            .jobs
            .values()
            .flat_map(|j| j.artifacts.values().map(|a| a.sha256.clone()))
            .collect();
        let dir = self.cfg.data_dir.join("artifacts");
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_millis() as u64 > ARTIFACTS_FOR_MS);
            if old && (!used.contains(&name) || name.starts_with(".part-")) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    /// One turn of the loop.
    pub fn tick(self: &Arc<Self>) {
        let now = now_ms();
        self.look(now);
        self.advance(now);
        self.schedule(now);
        self.follow();
        self.prune(now);
    }

    /// What the status page shows.
    pub fn status(&self) -> Value {
        let inner = self.lock();
        let count = |s: JobState| inner.jobs.values().filter(|j| j.state == s).count();
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "host": self.cfg.host_name,
            "devices": inner.registry.devices.len(),
            "registry": self.registry_path(),
            "registry_error": inner.registry_error,
            "jobs": {
                "ready": count(JobState::Ready),
                "waiting_device": count(JobState::WaitingDevice),
                "running": count(JobState::Running) + count(JobState::Dispatched),
                "awaiting_approval": count(JobState::AwaitingApproval),
            },
        })
    }

    /// Copy the database to `path` (backups).
    pub fn backup(&self, path: &Path) -> Result<()> {
        self.db.backup_to(path)
    }
}

/// Percent-encoding for a path in a query string.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
