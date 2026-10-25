//! Usage per subscription: Claude tokens from the transcripts of kimo's sessions, Codex limits
//! from its session logs, each attributed to the account the run used.

use crate::core::launch::default_home;
use crate::core::models::{Provider, Run};
use crate::core::usage::{
    claude_message, codex_limits, codex_meta, codex_run_for, run_at, Limits, Tokens,
};
use crate::services::sessions::Manager;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const HOUR: u64 = 3_600_000;

#[derive(Debug, Clone, Default, Serialize)]
pub struct AccountUsage {
    /// Account id; `None` = the provider's default login.
    pub account: Option<String>,
    pub provider: Option<Provider>,
    pub name: String,
    /// Tokens in the last 5 hours and 7 days (input + output + cache writes).
    pub tokens_5h: u64,
    pub tokens_7d: u64,
    pub detail_7d: Tokens,
    pub messages_7d: u64,
    pub last_activity: Option<u64>,
    /// Latest plan limits reported by the agent (Codex).
    pub limits: Option<Limits>,
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    std::fs::File::open(path)
        .into_iter()
        .flat_map(|f| BufReader::new(f).lines().map_while(Result::ok))
}

/// Codex session logs modified in the last `days` days.
pub(crate) fn codex_logs(home: &Path, since: u64) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![home.join("sessions")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                let modified = entry.metadata().and_then(|m| m.modified()).ok();
                let ms = modified
                    .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as u64);
                if ms >= since {
                    out.push(path);
                }
            }
        }
    }
    out
}

impl Manager {
    pub fn usage(&self) -> Vec<AccountUsage> {
        let (runs, sessions, accounts) = {
            let inner = self.lock();
            (
                inner.state.runs.clone(),
                inner.state.sessions.clone(),
                inner.state.accounts.clone(),
            )
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let week = now.saturating_sub(7 * 24 * HOUR);
        let mut out: HashMap<(Provider, Option<String>), AccountUsage> = HashMap::new();
        for provider in [Provider::Claude, Provider::Codex] {
            out.insert(
                (provider, None),
                AccountUsage {
                    provider: Some(provider),
                    name: format!(
                        "default {} login",
                        if provider == Provider::Claude {
                            "claude"
                        } else {
                            "codex"
                        }
                    ),
                    ..Default::default()
                },
            );
        }
        for a in &accounts {
            out.insert(
                (a.provider, Some(a.id.clone())),
                AccountUsage {
                    account: Some(a.id.clone()),
                    provider: Some(a.provider),
                    name: a.name.clone(),
                    ..Default::default()
                },
            );
        }
        let mut bump = |run: &Run, at: u64, tokens: Tokens| {
            if let Some(u) = out.get_mut(&(run.provider, run.account.clone())) {
                if at >= now.saturating_sub(5 * HOUR) {
                    u.tokens_5h += tokens.billable();
                }
                u.tokens_7d += tokens.billable();
                u.detail_7d.add(tokens);
                u.messages_7d += 1;
                u.last_activity = Some(u.last_activity.unwrap_or(0).max(at));
            }
        };

        // Claude: the transcripts of kimo's sessions (all accounts share the projects folder).
        let projects = default_home(Provider::Claude, &self.cfg.user_home).join("projects");
        let wanted: HashMap<String, String> = sessions
            .iter()
            .filter_map(|s| {
                s.conversation
                    .as_ref()
                    .map(|c| (format!("{c}.jsonl"), s.id.clone()))
            })
            .collect();
        for dir in std::fs::read_dir(&projects).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let name = file.file_name().to_string_lossy().into_owned();
                let Some(session) = wanted.get(&name) else {
                    continue;
                };
                let mut seen = HashSet::new();
                for line in lines(&file.path()) {
                    let Some(m) = claude_message(&line) else {
                        continue;
                    };
                    if m.at < week || !seen.insert(m.id.clone()) {
                        continue;
                    }
                    if let Some(run) = run_at(&runs, session, m.at) {
                        bump(run, m.at, m.tokens);
                    }
                }
            }
        }

        // Codex: limits and token totals from its session logs.
        // Folders compared with symlinks resolved, as Codex records the resolved path.
        let codex_runs: Vec<Run> = runs
            .iter()
            .filter(|r| r.provider == Provider::Codex)
            .map(|r| Run {
                cwd: crate::services::sessions::canonical(&r.cwd),
                ..r.clone()
            })
            .collect();
        if !codex_runs.is_empty() {
            let home = default_home(Provider::Codex, &self.cfg.user_home);
            for log in codex_logs(&home, week) {
                let mut it = lines(&log);
                let Some((cwd, started)) = it.next().and_then(|l| codex_meta(&l)) else {
                    continue;
                };
                let cwd = crate::services::sessions::canonical(&cwd);
                let Some(run) = codex_run_for(&codex_runs, &cwd, started) else {
                    continue;
                };
                let mut latest: Option<Limits> = None;
                for line in it {
                    if let Some(l) = codex_limits(&line) {
                        latest = Some(l);
                    }
                }
                if let (Some(l), Some(u)) =
                    (latest, out.get_mut(&(Provider::Codex, run.account.clone())))
                {
                    u.last_activity = Some(u.last_activity.unwrap_or(0).max(l.at));
                    if u.limits.as_ref().is_none_or(|old| old.at < l.at) {
                        u.limits = Some(l);
                    }
                }
            }
        }

        let mut list: Vec<AccountUsage> = out.into_values().collect();
        list.sort_by(|a, b| (a.account.is_some(), &a.name).cmp(&(b.account.is_some(), &b.name)));
        list
    }
}
