//! Keeping the machine's memory in check: the memory report, stopping idle agents when memory
//! runs out (or after a while idle), and what each agent's memory is made of.

use crate::core::activity::Activity;
use crate::core::lifecycle::Action;
use crate::core::memory::{self, Candidate, Pressure};
use crate::core::models::{Freed, SessionStatus};
use crate::services::sessions::{Manager, SessionInfo};
use crate::services::stats;
use serde::Serialize;
use std::time::{Duration, Instant};

/// Wait this long after stopping agents before stopping more (memory figures lag behind).
const COOLDOWN: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Serialize)]
pub struct MemoryReport {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub agents_bytes: u64,
    pub pressure: Pressure,
    /// Available memory under which the machine counts as low / critical.
    pub low_bytes: u64,
    pub critical_bytes: u64,
    /// Running agents over the "heavy" size from the settings.
    pub heavy: Vec<String>,
    /// Idle agents that can be stopped now (their conversation continues later).
    pub idle: Vec<String>,
    /// Agents kimo stopped to free memory, newest first.
    pub freed: Vec<FreedAgent>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FreedAgent {
    pub id: String,
    pub name: String,
    #[serde(flatten)]
    pub freed: Freed,
}

/// One part of an agent's memory: the agent itself, an MCP server, a tool it runs.
#[derive(Debug, Clone, Serialize)]
pub struct Part {
    pub pid: i32,
    pub label: String,
    pub bytes: u64,
    pub processes: u32,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn candidates(sessions: &[SessionInfo]) -> Vec<Candidate> {
    let now = now_ms();
    sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Running)
        .map(|s| Candidate {
            id: s.spec.id.clone(),
            rss: s.usage.rss_bytes,
            quiet_ms: now.saturating_sub(s.last_output_at.unwrap_or(now)),
            idle: s.activity == Some(Activity::Idle),
            resumable: s.spec.kind.provider().is_some()
                && s.spec.command.is_none()
                && s.spec.conversation.is_some(),
        })
        .collect()
}

impl Manager {
    pub fn memory(&self) -> MemoryReport {
        let (total, available) = stats::system_memory();
        let sessions = self.sessions();
        let heavy_bytes = u64::from(self.settings().memory.heavy_mb) << 20;
        let mut freed: Vec<FreedAgent> = sessions
            .iter()
            .filter(|s| s.status != SessionStatus::Running)
            .filter_map(|s| {
                Some(FreedAgent {
                    id: s.spec.id.clone(),
                    name: s.spec.name.clone(),
                    freed: s.spec.freed.clone()?,
                })
            })
            .collect();
        freed.sort_by_key(|f| std::cmp::Reverse(f.freed.at));
        MemoryReport {
            total_bytes: total,
            available_bytes: available,
            agents_bytes: sessions.iter().map(|s| s.usage.rss_bytes).sum(),
            pressure: memory::pressure(total, available),
            low_bytes: memory::low_mark(total),
            critical_bytes: memory::critical_mark(total),
            heavy: sessions
                .iter()
                .filter(|s| s.status == SessionStatus::Running && s.usage.rss_bytes > heavy_bytes)
                .map(|s| s.spec.id.clone())
                .collect(),
            idle: memory::to_free(&candidates(&sessions), u64::MAX),
            freed,
        }
    }

    /// Stop these agents to free memory, remembering why (their conversation continues when
    /// they start again). Returns the ids actually stopped.
    pub async fn free_agents(&self, ids: &[String], reason: &str) -> Vec<String> {
        let mut stopped = vec![];
        for id in ids {
            let bytes = self.session(id).map_or(0, |s| s.usage.rss_bytes);
            match self.act(id, Action::Stop).await {
                Ok(_) => {
                    let mut inner = self.lock();
                    if let Some(spec) = inner.state.sessions.iter_mut().find(|s| &s.id == id) {
                        spec.freed = Some(Freed {
                            at: now_ms(),
                            bytes,
                            reason: reason.to_string(),
                        });
                    }
                    let _ = self.save(&inner);
                    tracing::info!("stopped {id} to free memory ({reason}, {} MB)", bytes >> 20);
                    stopped.push(id.clone());
                }
                Err(e) => tracing::warn!("could not stop {id} to free memory: {e:#}"),
            }
        }
        if !stopped.is_empty() {
            *self.last_freed.lock().unwrap() = Some(Instant::now());
        }
        stopped
    }

    /// Stop every idle agent now (the "free memory" button).
    pub async fn free_idle(&self) -> Vec<String> {
        let ids = memory::to_free(&candidates(&self.sessions()), u64::MAX);
        self.free_agents(&ids, "asked").await
    }

    /// Called every few seconds: when memory is critical, stop idle agents (biggest first)
    /// until it is back above "low"; also stop agents idle longer than the setting allows.
    /// `memory` is (total, available) bytes.
    pub async fn guard_memory(&self, memory: (u64, u64)) -> Vec<String> {
        if self
            .last_freed
            .lock()
            .unwrap()
            .is_some_and(|t| t.elapsed() < COOLDOWN)
        {
            return vec![];
        }
        let settings = self.settings().memory;
        let sessions = self.sessions();
        let all = candidates(&sessions);
        let mut stopped = self
            .free_agents(
                &memory::idle_too_long(&all, settings.stop_idle_after_minutes),
                "idle",
            )
            .await;
        if settings.free_when_low {
            let (total, available) = memory;
            if memory::pressure(total, available) == Pressure::Critical {
                let rest: Vec<Candidate> = all
                    .into_iter()
                    .filter(|c| !stopped.contains(&c.id))
                    .collect();
                let need = memory::low_mark(total).saturating_sub(available);
                let ids = memory::to_free(&rest, need);
                if ids.is_empty() {
                    tracing::warn!(
                        "memory is critical ({} MB free) and no idle agent can be stopped",
                        available >> 20
                    );
                }
                stopped.extend(self.free_agents(&ids, "low memory").await);
            }
        }
        stopped
    }

    /// What an agent's memory is made of: the agent itself, then each process it started
    /// with its own children (MCP servers, builds, dev servers), biggest first.
    pub fn memory_parts(&self, id: &str) -> Vec<Part> {
        let Some(pid) = self.session(id).and_then(|s| s.pid) else {
            return vec![];
        };
        let page = stats::page_size();
        stats::read_table()
            .tree_parts(pid)
            .into_iter()
            .map(|(pid, usage)| Part {
                pid,
                label: memory::part_label(&stats::cmdline(pid)),
                bytes: usage.rss_pages * page,
                processes: usage.processes,
            })
            .collect()
    }
}
