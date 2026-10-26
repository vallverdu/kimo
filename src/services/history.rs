//! Earlier Claude Code and Codex conversations in a folder, and continuing them as kimo
//! sessions (what happens when a folder becomes a project).

use crate::core::history::{claude_dir_name, claude_line, codex_line, Summary};
use crate::core::launch::default_home;
use crate::core::models::{Provider, SessionKind};
use crate::services::sessions::{canonical, Manager, NewSession, SessionInfo};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub agent: SessionKind,
    /// The agent's conversation id (what `--resume` / `codex resume` take).
    pub id: String,
    pub title: String,
    /// Unix ms of the last change.
    pub updated_at: u64,
    pub messages: u32,
    /// The kimo session already attached to it, if any.
    pub session: Option<String>,
}

fn summarize(
    path: &Path,
    fold: fn(&mut Summary, &str),
    first_only_if: Option<&dyn Fn(&Summary) -> bool>,
) -> Option<Summary> {
    let file = std::fs::File::open(path).ok()?;
    let mut s = Summary::default();
    for (i, line) in BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .enumerate()
    {
        fold(&mut s, &line);
        // Codex: stop after the first line when the folder does not match.
        if i == 0 {
            if let Some(keep) = first_only_if {
                if !keep(&s) {
                    return None;
                }
            }
        }
    }
    Some(s)
}

fn modified_ms(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

impl Manager {
    /// Claude and Codex conversations that happened in `folder`, newest first.
    pub fn conversations(&self, folder: &str) -> Vec<Conversation> {
        let folder = self.expand_home(folder.trim());
        let want = canonical(&folder);
        let in_use: HashMap<String, String> = self
            .sessions()
            .into_iter()
            .filter_map(|s| s.spec.conversation.clone().map(|c| (c, s.spec.id.clone())))
            .collect();
        let mut out = vec![];

        // Claude: ~/.claude/projects/<folder with non-alphanumerics as '-'>/<id>.jsonl
        let projects = default_home(Provider::Claude, &self.cfg.user_home).join("projects");
        let mut dirs = vec![claude_dir_name(&want)];
        if folder != want {
            dirs.push(claude_dir_name(&folder));
        }
        dirs.dedup();
        for dir in dirs {
            for entry in std::fs::read_dir(projects.join(dir))
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                let Some(s) = summarize(&path, claude_line, None) else {
                    continue;
                };
                // Different folders can share a directory name: trust the recorded folder.
                if s.messages == 0 || s.cwd.as_deref().is_some_and(|c| canonical(c) != want) {
                    continue;
                }
                let id = s.id.clone().unwrap_or_else(|| {
                    path.file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                });
                out.push(Conversation {
                    agent: SessionKind::Claude,
                    session: in_use.get(&id).cloned(),
                    title: s.best_title(),
                    updated_at: modified_ms(&path),
                    messages: s.messages,
                    id,
                });
            }
        }

        // Codex: session logs whose first line names this folder.
        let home = default_home(Provider::Codex, &self.cfg.user_home);
        let same_folder = |s: &Summary| s.cwd.as_deref().is_some_and(|c| canonical(c) == want);
        for log in crate::services::usage::codex_logs(&home, 0) {
            let Some(s) = summarize(&log, codex_line, Some(&same_folder)) else {
                continue;
            };
            let Some(id) = s.id.clone() else { continue };
            if s.messages == 0 {
                continue;
            }
            out.push(Conversation {
                agent: SessionKind::Codex,
                session: in_use.get(&id).cloned(),
                title: s.best_title(),
                updated_at: modified_ms(&log),
                messages: s.messages,
                id,
            });
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        out.truncate(100);
        out
    }

    /// A kimo session for an existing conversation (reused when one is already attached).
    pub fn continue_conversation(
        &self,
        kind: SessionKind,
        cwd: &str,
        id: &str,
        name: Option<String>,
        start: bool,
    ) -> Result<SessionInfo> {
        if kind == SessionKind::Shell {
            bail!("only Claude and Codex conversations can be continued");
        }
        let existing = self
            .sessions()
            .into_iter()
            .find(|s| s.spec.conversation.as_deref() == Some(id));
        let session = match existing {
            Some(s) => s,
            None => {
                let created = self.create(
                    NewSession {
                        model: None,
                        effort: None,
                        name: name.unwrap_or_else(|| {
                            format!(
                                "{} {}",
                                if kind == SessionKind::Claude {
                                    "claude"
                                } else {
                                    "codex"
                                },
                                &id[..id.len().min(8)]
                            )
                        }),
                        kind,
                        cwd: cwd.to_string(),
                        account: None,
                        args: vec![],
                        command: None,
                    },
                    false,
                )?;
                let mut inner = self.lock();
                if let Some(spec) = inner
                    .state
                    .sessions
                    .iter_mut()
                    .find(|s| s.id == created.spec.id)
                {
                    spec.conversation = Some(id.to_string());
                    // Starting resumes the conversation instead of opening a new one.
                    spec.started_once = true;
                }
                self.save(&inner)?;
                drop(inner);
                self.session(&created.spec.id).unwrap_or(created)
            }
        };
        if start
            && !matches!(
                session.status,
                crate::core::models::SessionStatus::Running
                    | crate::core::models::SessionStatus::Paused
            )
        {
            self.start(&session.spec.id)?;
        }
        Ok(self.session(&session.spec.id).unwrap_or(session))
    }

    /// When a folder becomes a project (or is added again): continue its latest Claude and
    /// latest Codex conversation (the newest of them starts, the other waits for "continue").
    pub fn continue_latest(&self, project: &str, folder: &str) -> Vec<SessionInfo> {
        let conversations = self.conversations(folder);
        let mut picked: Vec<&Conversation> = vec![];
        // The latest of each agent, also when a kimo session is already attached to it
        // (that session is reused and opened, not duplicated).
        for kind in [SessionKind::Claude, SessionKind::Codex] {
            if let Some(c) = conversations.iter().find(|c| c.agent == kind) {
                picked.push(c);
            }
        }
        picked.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        picked
            .iter()
            .enumerate()
            .filter_map(|(i, c)| {
                // Named after the conversation (its title), like the agent shows it.
                let title: String = c.title.chars().take(32).collect();
                let name = if title.is_empty() || title == "untitled" {
                    format!(
                        "{project} {}",
                        if c.agent == SessionKind::Claude {
                            "claude"
                        } else {
                            "codex"
                        }
                    )
                } else if title.chars().count() < c.title.chars().count() {
                    format!("{}…", title.trim_end())
                } else {
                    title
                };
                self.continue_conversation(c.agent, folder, &c.id, Some(name), i == 0)
                    .ok()
            })
            .collect()
    }
}
