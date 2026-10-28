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
    last_error: Option<String>,
    /// The command, when not the system's (tests, setups that need another).
    command: Option<String>,
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
        let sessions: Vec<(String, bool)> = self
            .sessions()
            .into_iter()
            .filter(|s| matches!(s.status, SessionStatus::Running | SessionStatus::Paused))
            .map(|s| (s.spec.name.clone(), s.status == SessionStatus::Paused))
            .collect();
        let jobs: Vec<String> = self.running_jobs();
        let logins = if self.logins_count() { logins } else { &[] };
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
            if let Some(e) = error {
                tracing::warn!("cannot switch this machine off: {e}");
                let mut s = me.shutdown_state();
                s.started = false;
                s.last_error = Some(e);
            }
        });
        Ok(self.power())
    }
}
