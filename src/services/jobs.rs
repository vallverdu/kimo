//! Jobs: commands and headless agents run to the end, detached from kimo (`POST /api/jobs`).
//! kimo writes the job's folder and starts `kimo job-run <folder>`, which detaches, runs the
//! program and writes how it ended. kimo only reads the folder, so a job keeps running when
//! kimo updates, restarts or crashes, and the coordinator gets a real exit code.

use crate::core::jobs::{self, Exit, JobSpec, JobState};
use crate::core::models::{Permissions, Provider};
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Folders kept: older finished jobs go first.
const KEEP: usize = 200;
/// Most bytes of log one call returns.
const LOG_CHUNK: u64 = 256 << 10;
/// Between asking a job to end and ending it.
const TERM_GRACE: Duration = Duration::from_secs(10);

/// A job as `POST /api/jobs` takes it: a command, or a prompt for a headless agent.
#[derive(Debug, Clone, Deserialize)]
pub struct NewJob {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// `["make", "test"]`, or a line for `sh -c`.
    #[serde(default)]
    pub command: Option<CommandLine>,
    #[serde(default)]
    pub agent: Option<AgentJob>,
    /// The folder it runs in (default: the home folder).
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub timeout_s: Option<u64>,
    /// `false`: create it staged, to put its input files in place first
    /// (`PUT /api/jobs/{id}/files`), then `POST /api/jobs/{id}/start`.
    #[serde(default = "yes")]
    pub start: bool,
    /// Run in a fresh checkout of a commit (instead of `cwd`): a worktree of a repository on
    /// this machine, or a clone of a URL.
    #[serde(default)]
    pub git: Option<GitSource>,
}

/// Where a job's checkout comes from.
#[derive(Debug, Clone, Deserialize)]
pub struct GitSource {
    /// A repository folder on this machine (`~/dev/seg`) or a URL to clone.
    pub repo: String,
    /// The commit, branch or tag to check out (default: the repository's HEAD).
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    /// A new branch for the job's work (so an agent can push it).
    #[serde(default)]
    pub branch: Option<String>,
}

/// `git` with a time limit and no prompts; its error output becomes the error.
fn git(dir: &Path, args: &[&str], secs: u64) -> Result<String> {
    let out = crate::services::network::run_for(
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_TERMINAL_PROMPT", "0"),
        Duration::from_secs(secs),
    )
    .context("running git")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .map(|l| {
                l.trim_start_matches("fatal: ")
                    .trim_start_matches("error: ")
            })
            .find(|l| !l.trim().is_empty())
            .unwrap_or("git failed")
            .to_string();
        bail!("git {}: {line}", args.first().copied().unwrap_or(""));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CommandLine {
    Shell(String),
    Argv(Vec<String>),
}

/// A headless agent run: `claude -p` or `codex exec`, ending with its answer.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentJob {
    pub kind: Provider,
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// A kimo account (default: the active one of the provider).
    #[serde(default)]
    pub account: Option<String>,
    /// `yolo` skips the agent's approvals (unattended work needs it); default: the agent
    /// settings'.
    #[serde(default)]
    pub permissions: Option<Permissions>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: String,
    pub name: String,
    pub state: JobState,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    pub log_bytes: u64,
    /// The checkout it runs in (repository, commit, branch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<jobs::Checkout>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogChunk {
    pub offset: u64,
    /// Where the next call continues.
    pub next: u64,
    pub size: u64,
    pub text: String,
    /// The job ended: nothing more will come.
    pub done: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn read_number(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Write a file whole or not at all (readers never see half of it).
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn alive(pid: i32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    pid > 0
        && (unsafe { libc::kill(pid, 0) } == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

/// When the machine started (ms), to tell a runner from before a reboot (looked up once).
fn boot_at() -> Option<u64> {
    static BOOT: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *BOOT.get_or_init(|| {
        let up = crate::services::node::uptime_s()?;
        now_ms().checked_sub(up * 1000 + 2000)
    })
}

fn read_job(dir: &Path, boot: Option<u64>) -> Option<Job> {
    let spec: JobSpec = serde_json::from_slice(&std::fs::read(dir.join("spec.json")).ok()?).ok()?;
    let exit: Option<Exit> = std::fs::read(dir.join("exit.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let started_at = read_number(&dir.join("started_at"));
    let runner = read_number(&dir.join("runner.pid")).unwrap_or(0) as i32;
    let state = jobs::state(
        exit.as_ref(),
        spec.created_at,
        started_at,
        alive(runner),
        boot,
        dir.join("staged").exists(),
        now_ms(),
    );
    let log_bytes = std::fs::metadata(dir.join("log")).map_or(0, |m| m.len());
    Some(Job {
        id: spec.id,
        name: spec.name,
        state,
        program: spec.program,
        args: spec.args,
        cwd: spec.cwd,
        agent: spec.agent,
        timeout_s: spec.timeout_s,
        created_at: spec.created_at,
        started_at,
        ended_at: exit.as_ref().map(|e| e.ended_at),
        exit_code: exit.as_ref().and_then(|e| e.code),
        signal: exit.as_ref().and_then(|e| e.signal),
        log_bytes,
        git: spec.git,
    })
}

impl Manager {
    pub(crate) fn jobs_dir(&self) -> PathBuf {
        self.cfg.data_dir.join("jobs")
    }

    fn job_dir(&self, id: &str) -> Result<PathBuf> {
        if !jobs::valid_id(id) {
            bail!("unknown job: {id}");
        }
        let dir = self.jobs_dir().join(id);
        if !dir.join("spec.json").is_file() {
            bail!("unknown job: {id}");
        }
        Ok(dir)
    }

    /// The program that runs jobs (`kimo job-run`): this kimo, unless set otherwise (tests).
    fn job_runner(&self) -> Result<PathBuf> {
        if let Some(exe) = self
            .job_runner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            return Ok(exe);
        }
        crate::services::update::exe()
    }

    /// Run jobs with another kimo binary.
    pub fn set_job_runner(&self, exe: &Path) {
        *self.job_runner.lock().unwrap_or_else(|e| e.into_inner()) = Some(exe.to_path_buf());
    }

    /// Every job, newest first.
    pub fn jobs(&self) -> Vec<Job> {
        let boot = boot_at();
        let mut list: Vec<Job> = std::fs::read_dir(self.jobs_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| read_job(&e.path(), boot))
            .collect();
        list.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        list
    }

    pub fn job(&self, id: &str) -> Result<Job> {
        read_job(&self.job_dir(id)?, boot_at()).ok_or_else(|| anyhow!("unknown job: {id}"))
    }

    /// Ids of the jobs running now (they keep the machine awake and on). Cheap enough to
    /// ask every few seconds: ended jobs are passed over by their `exit.json`.
    pub fn running_jobs(&self) -> Vec<String> {
        let boot = boot_at();
        std::fs::read_dir(self.jobs_dir())
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|dir| !dir.join("exit.json").exists())
            .filter_map(|dir| read_job(&dir, boot))
            .filter(|j| !j.state.finished())
            .map(|j| j.id)
            .collect()
    }

    /// Start a job: its folder, then its runner. Returns it as it starts.
    pub fn create_job(self: &Arc<Self>, new: NewJob) -> Result<Job> {
        let id = match new.id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => {
                if !jobs::valid_id(id) {
                    bail!("a job id has only letters, digits, - and _");
                }
                id.to_string()
            }
            _ => format!("job-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
        };
        let root = self.jobs_dir();
        std::fs::create_dir_all(&root)?;
        let dir = root.join(&id);
        if dir.exists() {
            bail!("there is already a job {id}");
        }
        if new.git.is_some() && new.cwd.as_deref().is_some_and(|c| !c.trim().is_empty()) {
            bail!("a job runs in its cwd or in a git checkout, not both");
        }
        if let Some(b) = new.git.as_ref().and_then(|g| g.branch.as_deref()) {
            if !jobs::valid_branch(b) {
                bail!("{b:?} is not a branch name");
            }
        }
        // Its folder, or a fresh one of its own (`work` in the job's folder).
        let cwd = match new.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
            Some(c) => {
                let cwd = self.cfg.user_home.join(self.expand_home(c));
                if !cwd.is_dir() {
                    bail!("no folder {}", cwd.display());
                }
                cwd
            }
            None => dir.join("work"),
        };
        let mut env: Vec<(String, String)> = new.env.into_iter().collect();
        env.push(("KIMO_JOB".into(), id.clone()));
        env.push((
            "KIMO_URL".into(),
            format!("http://127.0.0.1:{}", self.cfg.port),
        ));
        let mut stdin_file = None;
        let (program, args, agent, stdout_is_result) = match (new.command, new.agent) {
            (Some(_), Some(_)) => bail!("a job runs a command or an agent, not both"),
            (None, None) => bail!("a job needs a command or an agent"),
            (Some(CommandLine::Shell(line)), None) => {
                if line.trim().is_empty() {
                    bail!("the command is empty");
                }
                ("sh".to_string(), vec!["-c".to_string(), line], None, false)
            }
            (Some(CommandLine::Argv(argv)), None) => {
                let Some((program, args)) = argv.split_first() else {
                    bail!("the command is empty");
                };
                (program.clone(), args.to_vec(), None, false)
            }
            (None, Some(agent)) => {
                if agent.prompt.trim().is_empty() {
                    bail!("the agent needs a prompt");
                }
                let settings = self.settings();
                let mut prefs = settings.agents.get(agent.kind).clone();
                if let Some(m) = agent.model.filter(|m| crate::core::launch::valid_model(m)) {
                    prefs.model = m;
                }
                if let Some(e) = agent.effort {
                    prefs.effort = e;
                }
                if let Some(p) = agent.permissions {
                    prefs.permissions = p;
                }
                let account = match crate::services::sessions::account_key(agent.account) {
                    Some(a) => Some(a),
                    None => settings.active_accounts.get(agent.kind).cloned(),
                };
                if let Some(aid) = account {
                    let acc = self
                        .lock()
                        .state
                        .accounts
                        .iter()
                        .find(|a| a.id == aid && a.provider == agent.kind)
                        .cloned()
                        .ok_or_else(|| anyhow!("unknown account: {aid}"))?;
                    crate::services::accounts::ensure_home(
                        &self.cfg.data_dir,
                        &self.cfg.user_home,
                        &acc,
                    )?;
                    env.push((
                        crate::core::launch::config_env_var(agent.kind).to_string(),
                        crate::core::launch::account_home(&self.cfg.data_dir, &acc.id)
                            .display()
                            .to_string(),
                    ));
                }
                let result = dir.join("result");
                let (program, args, stdout_is_result) =
                    jobs::headless_agent(agent.kind, &prefs, &result.display().to_string());
                stdin_file = Some("prompt".to_string());
                (
                    program,
                    args,
                    Some((agent.kind, agent.prompt)),
                    stdout_is_result,
                )
            }
        };
        let name = new
            .name
            .map(|n| n.trim().chars().take(80).collect::<String>())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.clone());
        let spec = JobSpec {
            id: id.clone(),
            name,
            program,
            args,
            cwd: cwd.display().to_string(),
            env,
            timeout_s: new.timeout_s.filter(|t| *t > 0),
            agent: agent.as_ref().map(|a| a.0),
            stdout_is_result,
            stdin_file,
            created_at: now_ms(),
            git: None,
        };
        std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let checkout = match &new.git {
            Some(g) => match self.check_out(g, &cwd) {
                Ok(c) => Some(c),
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(e);
                }
            },
            None => {
                std::fs::create_dir_all(&cwd)?;
                None
            }
        };
        if let Some((_, prompt)) = &agent {
            std::fs::write(dir.join("prompt"), prompt)?;
        }
        if !new.start {
            std::fs::write(dir.join("staged"), b"")?;
        }
        let spec = JobSpec {
            git: checkout,
            ..spec
        };
        write_atomic(&dir.join("spec.json"), &serde_json::to_vec_pretty(&spec)?)?;
        if new.start {
            self.start_runner(&dir)?;
        }
        self.prune_jobs();
        self.changed();
        self.job(&id)
    }

    fn start_runner(&self, dir: &Path) -> Result<()> {
        // As for terminals: not while the machine is switching off.
        let _power = self.new_work()?;
        // The runner detaches at once: this waits only for that.
        let status = Command::new(self.job_runner()?)
            .arg("job-run")
            .arg(dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("starting the job runner")?;
        if !status.success() {
            bail!("the job runner did not start");
        }
        Ok(())
    }

    /// Make the job's checkout in `work`: a worktree of a repository here, or a clone.
    fn check_out(&self, g: &GitSource, work: &Path) -> Result<jobs::Checkout> {
        let reference = g.reference.as_deref().unwrap_or("HEAD");
        if reference.starts_with('-') {
            bail!("{reference:?} is not a commit");
        }
        let parent = work.parent().unwrap_or(work);
        let target = work.display().to_string();
        let worktree = !jobs::is_remote_repo(&g.repo);
        if worktree {
            let repo = self.cfg.user_home.join(self.expand_home(g.repo.trim()));
            if !repo.is_dir() {
                bail!("no repository {}", repo.display());
            }
            let mut args = vec!["worktree", "add"];
            match g.branch.as_deref() {
                Some(b) => args.extend(["-b", b]),
                None => args.push("--detach"),
            }
            args.extend([target.as_str(), reference]);
            git(&repo, &args, 120)?;
        } else {
            git(parent, &["clone", "--quiet", "--", &g.repo, &target], 600)?;
            if g.reference.is_some() {
                git(work, &["checkout", "--quiet", "--detach", reference], 60)?;
            }
            if let Some(b) = &g.branch {
                git(work, &["checkout", "--quiet", "-b", b], 60)?;
            }
        }
        Ok(jobs::Checkout {
            repo: g.repo.clone(),
            commit: git(work, &["rev-parse", "HEAD"], 30)?,
            branch: g.branch.clone(),
            worktree,
        })
    }

    /// Remove a job's folder, and its worktree from the repository it came from.
    fn remove_job_dir(&self, dir: &Path) {
        let spec: Option<JobSpec> = std::fs::read(dir.join("spec.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        if let Some(c) = spec.and_then(|s| s.git).filter(|c| c.worktree) {
            let repo = self.cfg.user_home.join(self.expand_home(&c.repo));
            let work = dir.join("work").display().to_string();
            if let Err(e) = git(&repo, &["worktree", "remove", "--force", &work], 60) {
                tracing::info!("worktree of a job: {e:#}");
            }
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Start a staged job (its input files are in place).
    pub fn start_job(&self, id: &str) -> Result<Job> {
        let dir = self.job_dir(id)?;
        if self.job(id)?.state != JobState::Staged {
            bail!("the job {id} is not waiting to start");
        }
        std::fs::remove_file(dir.join("staged"))?;
        self.start_runner(&dir)?;
        self.changed();
        self.job(id)
    }

    /// A path in the folder a job runs in; `None` when it leads outside.
    pub fn job_path(&self, id: &str, rel: &str) -> Result<PathBuf> {
        let job = self.job(id)?;
        let root = PathBuf::from(&job.cwd);
        let rel = crate::core::files::relative(rel)
            .filter(|r| !r.as_os_str().is_empty())
            .ok_or_else(|| anyhow!("{rel:?} is not a path inside the job's folder"))?;
        let path = root.join(&rel);
        // Through symlinks too: what exists of it must stay inside.
        let root = std::fs::canonicalize(&root)?;
        let mut existing = path.as_path();
        while !existing.exists() {
            existing = existing.parent().unwrap_or(&root);
        }
        if !std::fs::canonicalize(existing)?.starts_with(&root) {
            bail!("{} leads outside the job's folder", rel.display());
        }
        Ok(path)
    }

    /// Where an input file of a staged job goes (its folders are made).
    pub fn job_input_path(&self, id: &str, rel: &str) -> Result<PathBuf> {
        if self.job(id)?.state != JobState::Staged {
            bail!("files go in before the job starts");
        }
        let path = self.job_path(id, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(path)
    }

    /// The log from `offset` (at most 256 KB per call).
    pub fn job_log(&self, id: &str, offset: u64) -> Result<LogChunk> {
        let dir = self.job_dir(id)?;
        let done = self.job(id)?.state.finished();
        let path = dir.join("log");
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        let offset = offset.min(size);
        let mut text = Vec::new();
        if let Ok(mut f) = std::fs::File::open(&path) {
            f.seek(SeekFrom::Start(offset))?;
            f.take(LOG_CHUNK).read_to_end(&mut text)?;
        }
        let next = offset + text.len() as u64;
        Ok(LogChunk {
            offset,
            next,
            size,
            text: String::from_utf8_lossy(&text).into_owned(),
            done: done && next >= size,
        })
    }

    /// A headless agent's answer (Claude's JSON, Codex's last message), once it ended.
    pub fn job_result(&self, id: &str) -> Result<Option<serde_json::Value>> {
        let dir = self.job_dir(id)?;
        let Ok(bytes) = std::fs::read(dir.join("result")) else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).trim().to_string())
        })))
    }

    /// Ask a running job to end (its runner ends it after a grace period).
    pub fn cancel_job(&self, id: &str) -> Result<Job> {
        let dir = self.job_dir(id)?;
        let job = self.job(id)?;
        if job.state.finished() {
            return Ok(job);
        }
        if job.state == JobState::Staged {
            // Never started: it ends here, and can never be started any more.
            write_atomic(
                &dir.join("exit.json"),
                &serde_json::to_vec(&Exit {
                    cancelled: true,
                    ended_at: now_ms(),
                    ..Default::default()
                })?,
            )?;
            let _ = std::fs::remove_file(dir.join("staged"));
            self.changed();
            return self.job(id);
        }
        std::fs::write(dir.join("cancel"), b"")?;
        if let Some(pid) = read_number(&dir.join("pid")) {
            // SAFETY: a signal to the job's own process group.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGTERM);
            }
        }
        self.changed();
        self.job(id)
    }

    /// Forget a job that has ended (its folder goes).
    pub fn delete_job(&self, id: &str) -> Result<()> {
        let dir = self.job_dir(id)?;
        if !self.job(id)?.state.finished() {
            bail!("the job is still running: cancel it first");
        }
        self.remove_job_dir(&dir);
        self.changed();
        Ok(())
    }

    /// Keep the newest [`KEEP`] jobs (running ones always).
    fn prune_jobs(&self) {
        let jobs = self.jobs();
        for job in jobs.iter().skip(KEEP).filter(|j| j.state.finished()) {
            self.remove_job_dir(&self.jobs_dir().join(&job.id));
        }
    }
}

/// `kimo job-run <folder>`: run the job in the folder to its end. The first call detaches
/// (a new session, not kimo's child any more) and returns at once; the detached runner
/// starts the program in its own process group and writes how it ended.
pub fn run(dir: &Path) -> Result<()> {
    if std::env::var_os("KIMO_JOB_DETACHED").is_none() {
        let mut cmd = Command::new(std::env::current_exe()?);
        cmd.arg("job-run")
            .arg(dir)
            .env("KIMO_JOB_DETACHED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid only puts the runner in a session of its own.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn().context("detaching the job runner")?;
        return Ok(());
    }
    let spec: JobSpec = serde_json::from_slice(&std::fs::read(dir.join("spec.json"))?)?;
    write_atomic(
        &dir.join("runner.pid"),
        std::process::id().to_string().as_bytes(),
    )?;
    write_atomic(&dir.join("started_at"), now_ms().to_string().as_bytes())?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("log"))?;
    let finish = |exit: Exit| -> Result<()> {
        write_atomic(&dir.join("exit.json"), &serde_json::to_vec(&exit)?)
    };
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(spec.env.iter().map(|(k, v)| (k, v)))
        .env_remove("KIMO_JOB_DETACHED")
        .stderr(log.try_clone()?);
    cmd.stdin(match &spec.stdin_file {
        Some(f) => Stdio::from(std::fs::File::open(dir.join(f))?),
        None => Stdio::null(),
    });
    cmd.stdout(if spec.stdout_is_result {
        Stdio::from(std::fs::File::create(dir.join("result"))?)
    } else {
        Stdio::from(log.try_clone()?)
    });
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            use std::io::Write;
            let _ = writeln!(&log, "kimo: cannot start {}: {e}", spec.program);
            return finish(Exit {
                code: Some(127),
                not_started: true,
                ended_at: now_ms(),
                ..Default::default()
            });
        }
    };
    let pid = child.id() as i32;
    write_atomic(&dir.join("pid"), pid.to_string().as_bytes())?;
    let started = Instant::now();
    let limit = spec.timeout_s.map(Duration::from_secs);
    let mut ending: Option<(Instant, bool)> = None; // (asked at, because of the time limit)
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let cancelled = dir.join("cancel").exists();
        let late = limit.is_some_and(|l| started.elapsed() > l);
        match ending {
            None if cancelled || late => {
                // SAFETY: signals to the job's own process group.
                unsafe { libc::kill(-pid, libc::SIGTERM) };
                ending = Some((Instant::now(), late && !cancelled));
            }
            Some((asked, _)) if asked.elapsed() > TERM_GRACE => unsafe {
                libc::kill(-pid, libc::SIGKILL);
            },
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    use std::os::unix::process::ExitStatusExt;
    // kimo signals a cancelled job itself too: it may end before this loop saw the marker.
    let cancelled = matches!(ending, Some((_, false))) || dir.join("cancel").exists();
    let timed_out = !cancelled && matches!(ending, Some((_, true)));
    finish(Exit {
        code: status.code().or(if timed_out { Some(124) } else { None }),
        signal: status.signal(),
        timed_out,
        cancelled,
        not_started: false,
        ended_at: now_ms(),
    })
}
