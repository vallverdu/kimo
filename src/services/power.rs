//! Switching this machine off for a fleet coordinator (`POST /api/power/shutdown`), only when
//! nothing runs and nobody is logged in; the owner may force it. The command is the system's
//! (`systemctl poweroff`, System Events on macOS) or `KIMO_SHUTDOWN_COMMAND`.

use crate::core::models::SessionStatus;
use crate::core::power::{blockers, default_shutdown_command, Blocker};
use crate::services::network::run_for;
use crate::services::sessions::Manager;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Power {
    /// Nothing keeps the machine on.
    pub can_shutdown: bool,
    pub blockers: Vec<Blocker>,
    /// What kimo runs to switch the machine off.
    pub command: String,
    /// A shutdown was started (the machine should stop answering soon).
    pub shutting_down: bool,
    /// Why the last shutdown did not happen, if it failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// A shutdown in progress, or why the last one failed.
#[derive(Default)]
pub struct Shutdown {
    started: bool,
    /// Past the last check: the command runs, and no new work may start.
    committed: bool,
    last_error: Option<String>,
    /// The command, when not the system's (tests, setups that need another).
    command: Option<String>,
    /// After a shutdown command that worked, how long new work is still refused (the machine
    /// is going down); past it, kimo is evidently still up and takes work again.
    grace: Option<Duration>,
    /// People logged in do not keep the machine on (`KIMO_SHUTDOWN_IGNORE_LOGINS=1`: a
    /// desktop that logs in by itself).
    ignore_logins: bool,
}

/// Time for the answer to reach the caller before the machine goes down.
const DELAY: Duration = Duration::from_millis(1500);

impl Manager {
    fn shutdown_state(&self) -> std::sync::MutexGuard<'_, Shutdown> {
        self.shutdown.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The command that switches the machine off: `KIMO_SHUTDOWN_COMMAND`, else the system's.
    pub fn shutdown_command(&self) -> String {
        self.shutdown_state()
            .command
            .clone()
            .or_else(|| std::env::var("KIMO_SHUTDOWN_COMMAND").ok())
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_else(|| default_shutdown_command(std::env::consts::OS).to_string())
    }

    /// Starting new work (a terminal, a job): refused once a shutdown is past its last check;
    /// before that, the new work wins and the shutdown is stopped. Hold the guard while the
    /// work starts.
    pub(crate) fn new_work(&self) -> Result<std::sync::MutexGuard<'_, Shutdown>, anyhow::Error> {
        let s = self.shutdown_state();
        if s.committed {
            anyhow::bail!("this machine is switching off");
        }
        Ok(s)
    }

    /// How long new work is refused after a shutdown command that worked (default 30 s).
    pub fn set_shutdown_grace(&self, grace: Duration) {
        self.shutdown_state().grace = Some(grace);
    }

    /// Use another command to switch the machine off (`sh -c`).
    pub fn set_shutdown_command(&self, command: &str) {
        self.shutdown_state().command = Some(command.to_string());
    }

    /// Whether people logged in keep the machine on (they do, unless
    /// `KIMO_SHUTDOWN_IGNORE_LOGINS=1` or this says otherwise).
    pub fn set_shutdown_ignores_logins(&self, ignore: bool) {
        self.shutdown_state().ignore_logins = ignore;
    }

    fn logins_count(&self) -> bool {
        !self.shutdown_state().ignore_logins
            && std::env::var("KIMO_SHUTDOWN_IGNORE_LOGINS").map_or(true, |v| v != "1")
    }

    /// What keeps this machine on right now (blocking: asks `who`).
    pub fn power_blockers(&self) -> Vec<Blocker> {
        self.blockers_with(&crate::services::node::logins())
    }

    /// What keeps this machine on, with the people logged in already looked up.
    pub(crate) fn blockers_with(&self, logins: &[crate::core::node::Login]) -> Vec<Blocker> {
        let logins = if self.logins_count() { logins } else { &[] };
        self.blockers_of(logins)
    }

    /// What keeps this machine on, counting exactly these logins (takes no shutdown lock).
    fn blockers_of(&self, logins: &[crate::core::node::Login]) -> Vec<Blocker> {
        let sessions: Vec<(String, bool)> = self
            .sessions()
            .into_iter()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Paused))
            .map(|s| (s.spec.name.clone(), s.status == SessionStatus::Paused))
            .collect();
        let jobs: Vec<String> = self.running_jobs();
        blockers(&sessions, &jobs, logins)
    }

    pub fn power(&self) -> Power {
        let blockers = self.power_blockers();
        let command = self.shutdown_command();
        let s = self.shutdown_state();
        Power {
            can_shutdown: blockers.is_empty(),
            blockers,
            command,
            shutting_down: s.started,
            last_error: s.last_error.clone(),
        }
    }

    /// Switch the machine off in a moment, unless something keeps it on (then those things
    /// are the error). `force` (the owner only) switches it off anyway.
    pub fn power_off(self: &std::sync::Arc<Self>, force: bool) -> Result<Power, Vec<Blocker>> {
        let blockers = self.power_blockers();
        if !blockers.is_empty() && !force {
            return Err(blockers);
        }
        {
            let mut s = self.shutdown_state();
            s.started = true;
            s.last_error = None;
        }
        let cmd = self.shutdown_command();
        tracing::warn!("switching this machine off: {cmd}");
        let me = self.clone();
        std::thread::spawn(move || {
            std::thread::sleep(DELAY);
            // The last check, holding off new work: something that started meanwhile wins.
            {
                let logins = if me.logins_count() {
                    crate::services::node::logins()
                } else {
                    vec![]
                };
                let mut s = me.shutdown_state();
                if !s.started {
                    return;
                }
                if !force {
                    // `blockers_with` reads the sessions: the shutdown lock is taken first,
                    // as when work starts.
                    let blockers = me.blockers_of(&logins);
                    if !blockers.is_empty() {
                        let what: Vec<&str> = blockers.iter().map(|b| b.what.as_str()).collect();
                        tracing::info!("not switching off: {} started", what.join(", "));
                        s.started = false;
                        s.last_error = Some(format!(
                            "switching off was stopped: {} started meanwhile",
                            what.join(", ")
                        ));
                        return;
                    }
                }
                s.committed = true;
            }
            let result = run_for(
                Command::new("sh").args(["-c", &cmd]),
                Duration::from_secs(60),
            );
            let error = match result {
                Ok(out) if out.status.success() => None,
                Ok(out) => Some(
                    String::from_utf8_lossy(&out.stderr)
                        .lines()
                        .last()
                        .unwrap_or("the shutdown command failed")
                        .trim()
                        .to_string(),
                ),
                Err(e) => Some(format!("{e:#}")),
            };
            if error.is_none() {
                // Going down: still here after the grace, it did not switch off.
                let grace = me.shutdown_state().grace.unwrap_or(Duration::from_secs(30));
                std::thread::sleep(grace);
                let mut s = me.shutdown_state();
                if s.committed {
                    s.started = false;
                    s.committed = false;
                    s.last_error = Some(format!(
                        "still running {} s after the shutdown command",
                        grace.as_secs()
                    ));
                }
                return;
            }
            if let Some(e) = error {
                tracing::warn!("cannot switch this machine off: {e}");
                let mut s = me.shutdown_state();
                s.started = false;
                s.committed = false;
                s.last_error = Some(e);
            }
        });
        Ok(self.power())
    }
}
