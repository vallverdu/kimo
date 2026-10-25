//! Agents talking to agents: read another agent's last reply or screen, send it a message.

use crate::core::activity::strip_ansi;
use crate::core::launch::default_home;
use crate::core::models::{Provider, SessionKind, SessionStatus};
use crate::core::replies::{claude_reply, codex_reply, envelope, last_lines};
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub id: String,
    pub name: String,
    /// `transcript` (the agent's own log) or `terminal` (recent screen text).
    pub source: &'static str,
    pub text: String,
}

fn last_in(path: &Path, parse: fn(&str) -> Option<String>) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| parse(&l))
        .last()
}

fn find_file(root: &Path, depth: usize, matches: &dyn Fn(&str) -> bool) -> Option<PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() && depth > 0 {
            if let Some(found) = find_file(&path, depth - 1, matches) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(matches)
        {
            return Some(path);
        }
    }
    None
}

impl Manager {
    /// Find a session by id, name or unique id prefix.
    pub fn resolve(&self, key: &str) -> Result<crate::services::sessions::SessionInfo> {
        let sessions = self.sessions();
        if let Some(s) = sessions
            .iter()
            .find(|s| s.spec.id == key)
            .or_else(|| sessions.iter().find(|s| s.spec.name == key))
        {
            return Ok(s.clone());
        }
        let prefixed: Vec<_> = sessions
            .iter()
            .filter(|s| s.spec.id.starts_with(key))
            .collect();
        match prefixed.as_slice() {
            [one] => Ok((*one).clone()),
            [] => bail!("no agent called {key}: see `kimo agents`"),
            _ => bail!("{key} matches several agents: use the full id"),
        }
    }

    /// Recent terminal text of a session, without escape codes.
    pub fn screen(&self, key: &str, lines: usize) -> Result<Reply> {
        let s = self.resolve(key)?;
        let raw = self
            .attach(&s.spec.id)
            .ok_or_else(|| anyhow!("unknown session"))?
            .replay;
        Ok(Reply {
            id: s.spec.id,
            name: s.spec.name,
            source: "terminal",
            text: last_lines(&strip_ansi(&raw), lines),
        })
    }

    /// The agent's last reply from its own log (Claude transcript, Codex session log);
    /// the screen for shells and agents without a known conversation yet.
    pub fn reply(&self, key: &str) -> Result<Reply> {
        let s = self.resolve(key)?;
        let text = match (s.spec.kind, &s.spec.conversation) {
            (SessionKind::Claude, Some(conv)) => {
                let name = format!("{conv}.jsonl");
                find_file(
                    &default_home(Provider::Claude, &self.cfg.user_home).join("projects"),
                    1,
                    &|n| n == name,
                )
                .and_then(|f| last_in(&f, claude_reply))
            }
            (SessionKind::Codex, Some(conv)) => find_file(
                &default_home(Provider::Codex, &self.cfg.user_home).join("sessions"),
                4,
                &|n| n.contains(conv.as_str()),
            )
            .and_then(|f| last_in(&f, codex_reply)),
            _ => None,
        };
        match text {
            Some(text) => Ok(Reply {
                id: s.spec.id,
                name: s.spec.name,
                source: "transcript",
                text,
            }),
            None => self.screen(key, 40),
        }
    }

    /// Type a message into an agent (as a paste, then Enter), signed with the sender when known.
    pub async fn message(
        &self,
        to: &str,
        text: &str,
        from: Option<&str>,
        from_name: Option<&str>,
    ) -> Result<String> {
        let target = self.resolve(to)?;
        if target.status != SessionStatus::Running {
            bail!(
                "{} is {:?}: start it before sending messages",
                target.spec.name,
                target.status
            );
        }
        // A local sender by id or name; a sender on another kimo comes as `host/id`.
        let sender = from.map(|f| match self.resolve(f) {
            Ok(s) => (s.spec.id, s.spec.name),
            Err(_) => (f.to_string(), from_name.unwrap_or(f).to_string()),
        });
        let body = envelope(
            sender
                .as_ref()
                .map(|(id, name)| (id.as_str(), name.as_str())),
            text,
        );
        // Bracketed paste keeps multi-line messages together in TUIs and shells.
        self.write(
            &target.spec.id,
            format!("\x1b[200~{body}\x1b[201~").into_bytes(),
        );
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        self.write(&target.spec.id, b"\r".to_vec());
        Ok(target.spec.id)
    }
}
