//! Earlier Claude Code and Codex conversations in a folder, and continuing them as kimo
//! sessions (what happens when a folder becomes a project).

use crate::core::history::{self, claude_dir_name, claude_line, codex_line, Summary};
use crate::core::launch::default_home;
use crate::core::models::{Provider, SessionKind};
use crate::services::sessions::{canonical, Manager, NewSession, SessionInfo};
use anyhow::{bail, Result};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
pub struct Conversation {
    pub agent: SessionKind,
    /// The agent's conversation id (what `--resume` / `codex resume` take).
    pub id: String,
    /// Its first 8 characters, to say and type (`kimo continue c9c7d155`).
    pub short: String,
    /// The folder it went on in last.
    pub cwd: String,
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
                // Different folders can share a directory name: trust the recorded folder. One
                // that started elsewhere and went on here (moved, or continued here) counts.
                let here = |c: &Option<String>| c.as_deref().is_some_and(|c| canonical(c) == want);
                if s.messages == 0 || !(s.cwd.is_none() || here(&s.cwd) || here(&s.last_cwd)) {
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
                    short: history::short_id(&id).to_string(),
                    cwd: folder.clone(),
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
                short: history::short_id(&id).to_string(),
                cwd: folder.clone(),
                id,
            });
        }
        out.sort_by_key(|c| std::cmp::Reverse(c.updated_at));
        // The same conversation can sit in two folders' directories once it was moved.
        let mut seen = std::collections::HashSet::new();
        out.retain(|c| seen.insert(c.id.clone()));
        out.truncate(100);
        out
    }

    fn claude_projects(&self) -> PathBuf {
        default_home(Provider::Claude, &self.cfg.user_home).join("projects")
    }

    /// Every Claude transcript and Codex log whose id starts with `prefix`, in any folder.
    fn transcripts(&self, prefix: &str) -> Vec<(SessionKind, String, PathBuf)> {
        let mut out = vec![];
        for dir in std::fs::read_dir(self.claude_projects())
            .into_iter()
            .flatten()
            .flatten()
        {
            for entry in std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                let path = entry.path();
                let stem = path.file_stem().unwrap_or_default().to_string_lossy();
                if path.extension().is_some_and(|e| e == "jsonl") && stem.starts_with(prefix) {
                    out.push((SessionKind::Claude, stem.into_owned(), path));
                }
            }
        }
        // Codex: rollout-<date>-<uuid>.jsonl
        let home = default_home(Provider::Codex, &self.cfg.user_home);
        for log in crate::services::usage::codex_logs(&home, 0) {
            let stem = log.file_stem().unwrap_or_default().to_string_lossy();
            let id = stem.get(stem.len().saturating_sub(36)..).unwrap_or("");
            if id.starts_with(prefix) {
                out.push((SessionKind::Codex, id.to_string(), log.clone()));
            }
        }
        out
    }

    /// The conversation whose id starts with `prefix`, wherever it happened.
    pub fn find_conversation(&self, prefix: &str) -> Result<Conversation> {
        let prefix = prefix.trim();
        if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-') {
            bail!("a conversation id is letters a-f, digits and dashes");
        }
        let found = self.transcripts(prefix);
        let ids: Vec<&str> = found.iter().map(|(_, id, _)| id.as_str()).collect();
        let id = history::resolve(&ids, prefix).map_err(anyhow::Error::msg)?;
        // The newest copy, when it was moved.
        let Some((kind, id, path)) = found
            .iter()
            .filter(|(_, i, _)| i == id)
            .max_by_key(|(_, _, p)| modified_ms(p))
        else {
            bail!("no conversation {prefix}");
        };
        let fold = if *kind == SessionKind::Claude {
            claude_line
        } else {
            codex_line
        };
        let s = summarize(path, fold, None).unwrap_or_default();
        let session = self
            .sessions()
            .into_iter()
            .find(|x| x.spec.conversation.as_deref() == Some(id.as_str()))
            .map(|x| x.spec.id);
        Ok(Conversation {
            agent: *kind,
            short: history::short_id(id).to_string(),
            cwd: s.last_cwd.clone().or(s.cwd.clone()).unwrap_or_default(),
            title: s.best_title(),
            updated_at: modified_ms(path),
            messages: s.messages,
            session,
            id: id.clone(),
        })
    }

    /// Claude Code resumes only conversations kept under the folder it runs in: one continued
    /// in another folder is copied there first (never over an existing copy).
    fn bring_transcript(&self, id: &str, cwd: &str) -> Result<()> {
        if id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            bail!("that is not a conversation id");
        }
        let target = self
            .claude_projects()
            .join(claude_dir_name(&canonical(&self.expand_home(cwd))));
        let file = format!("{id}.jsonl");
        if target.join(&file).exists() {
            return Ok(());
        }
        let Some((_, _, from)) = self
            .transcripts(id)
            .into_iter()
            .filter(|(k, i, _)| *k == SessionKind::Claude && i == id)
            .max_by_key(|(_, _, p)| modified_ms(p))
        else {
            // A new conversation, or one Claude keeps elsewhere: `--resume` says so.
            return Ok(());
        };
        std::fs::create_dir_all(&target)?;
        std::fs::copy(&from, target.join(&file))?;
        tracing::info!("conversation {id} copied to {}", target.display());
        Ok(())
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
        if kind == SessionKind::Claude {
            self.bring_transcript(id, cwd)?;
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
