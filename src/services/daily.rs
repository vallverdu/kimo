//! Daily usage of every Claude Code and Codex conversation on this machine, read from the
//! agents' own logs (kimo's sessions and the ones run elsewhere), grouped by project.
//! Logs are append-only, so each file is read once and then only for its new lines.

use crate::core::daily::{aggregate, claude_event, local_day, CodexReader, Event, Report};
use crate::core::launch::default_home;
use crate::core::models::{Provider, Run};
use crate::core::usage::{codex_run_for, run_at};
use crate::services::sessions::{canonical, Manager};
use crate::services::usage::codex_logs;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const DAY: u64 = 86_400_000;

/// Where a folder's work is counted: (name, path, kimo project id).
type Place = (String, String, Option<String>);

/// What was read of one log: up to `offset` (the end of its last complete line).
#[derive(Default)]
struct Cached {
    offset: u64,
    codex: Option<CodexReader>,
    events: Vec<Event>,
}

/// Shared by every kimo in the process (logs are per machine, keyed by path).
fn cache() -> &'static Mutex<HashMap<PathBuf, Cached>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Cached>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Read the lines added to a log since the last time.
fn refresh(path: &Path, agent: Provider, c: &mut Cached) {
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let len = file.metadata().map_or(0, |m| m.len());
    if len < c.offset {
        *c = Cached::default(); // rewritten
    }
    if len == c.offset || file.seek(SeekFrom::Start(c.offset)).is_err() {
        return;
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();
    let codex = c.codex.get_or_insert_with(CodexReader::default);
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            // A line still being written is read next time.
            Ok(_) if !line.ends_with('\n') => break,
            Ok(n) => c.offset += n as u64,
        }
        let event = match agent {
            Provider::Claude => claude_event(&line, &stem),
            Provider::Codex => codex.event(&line),
        };
        if let Some(mut e) = event {
            // Most lines repeat the previous folder and conversation: share them.
            if let Some(last) = c.events.last() {
                if last.cwd == e.cwd {
                    e.cwd = last.cwd.clone();
                }
                if last.conversation == e.conversation {
                    e.conversation = last.conversation.clone();
                }
            }
            c.events.push(e);
        }
    }
}

/// Claude transcripts (subagents included) modified since `since`.
fn claude_logs(projects: &Path, since: u64) -> Vec<PathBuf> {
    let mut out = vec![];
    let mut stack = vec![(projects.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if depth < 3 {
                    stack.push((path, depth + 1));
                }
            } else if path.extension().is_some_and(|e| e == "jsonl") && modified_ms(&entry) >= since
            {
                out.push(path);
            }
        }
    }
    out
}

fn modified_ms(entry: &std::fs::DirEntry) -> u64 {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64)
}

/// Seconds east of UTC on this machine now.
fn local_offset() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as libc::time_t);
    // SAFETY: localtime_r only writes the `tm` it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            0
        } else {
            tm.tm_gmtoff as i64
        }
    }
}

/// The repository root above a folder (not above `home`), if any.
fn git_root(dir: &Path, home: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .take_while(|a| a.starts_with(home) && *a != home)
        .find(|a| a.join(".git").exists())
        .map(Path::to_path_buf)
}

impl Manager {
    /// Usage of the last `days` days (this machine's local days), today included.
    pub fn daily_usage(&self, days: u32) -> Report {
        let days = days.clamp(1, 366) as i64;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        let since = now.saturating_sub(days as u64 * DAY + DAY);
        let home = &self.cfg.user_home;
        let mut logs: Vec<(PathBuf, Provider)> = claude_logs(
            &default_home(Provider::Claude, home).join("projects"),
            since,
        )
        .into_iter()
        .map(|p| (p, Provider::Claude))
        .collect();
        logs.extend(
            codex_logs(&default_home(Provider::Codex, home), since)
                .into_iter()
                .map(|p| (p, Provider::Codex)),
        );

        let events: Vec<Event> = {
            let mut cache = cache().lock().unwrap_or_else(|e| e.into_inner());
            // Logs out of the range are forgotten, so memory follows what is shown.
            cache.retain(|p, _| logs.iter().any(|(l, _)| l == p));
            let mut all = vec![];
            for (path, agent) in &logs {
                let c = cache.entry(path.clone()).or_default();
                refresh(path, *agent, c);
                all.extend(c.events.iter().filter(|e| e.at >= since).cloned());
            }
            all
        };

        let (projects, sessions, runs, accounts) = {
            let inner = self.lock();
            (
                inner.state.projects.clone(),
                inner.state.sessions.clone(),
                inner.state.runs.clone(),
                inner.state.accounts.clone(),
            )
        };
        let projects: Vec<(PathBuf, String, String)> = projects
            .iter()
            .map(|p| {
                (
                    PathBuf::from(canonical(&p.path)),
                    p.name.clone(),
                    p.id.clone(),
                )
            })
            .collect();
        let home_c = PathBuf::from(canonical(&home.display().to_string()));
        let resolved: Mutex<HashMap<String, String>> = Mutex::default();
        let resolve = |cwd: &str| -> String {
            let mut m = resolved.lock().unwrap_or_else(|e| e.into_inner());
            m.entry(cwd.to_string())
                .or_insert_with(|| canonical(cwd))
                .clone()
        };
        let places: Mutex<HashMap<String, Place>> = Mutex::default();
        let place_of = |cwd: &str| -> Place {
            if let Some(p) = places.lock().unwrap_or_else(|e| e.into_inner()).get(cwd) {
                return p.clone();
            }
            let dir = PathBuf::from(resolve(cwd));
            let place = if cwd.is_empty() {
                ("unknown folder".to_string(), String::new(), None)
            } else if let Some((path, name, id)) = projects
                .iter()
                .filter(|(path, ..)| dir.starts_with(path))
                .max_by_key(|(path, ..)| path.as_os_str().len())
            {
                (name.clone(), path.display().to_string(), Some(id.clone()))
            } else {
                let root = git_root(&dir, &home_c).unwrap_or(dir);
                let name = if root == home_c {
                    "~".to_string()
                } else {
                    root.file_name().map_or_else(
                        || root.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    )
                };
                (name, root.display().to_string(), None)
            };
            places
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(cwd.to_string(), place.clone());
            place
        };

        // Which kimo session (and so which run and account) each conversation belongs to.
        let by_conversation: HashMap<&str, &str> = sessions
            .iter()
            .filter_map(|s| s.conversation.as_deref().map(|c| (c, s.id.as_str())))
            .collect();
        let codex_runs: Vec<Run> = runs
            .iter()
            .filter(|r| r.provider == Provider::Codex)
            .map(|r| Run {
                cwd: canonical(&r.cwd),
                ..r.clone()
            })
            .collect();
        // A Codex conversation kimo no longer lists: the Codex run in its folder when it began.
        let mut codex_owner: HashMap<&str, Option<Option<String>>> = HashMap::new();
        for e in events.iter().filter(|e| e.agent == Provider::Codex) {
            if !by_conversation.contains_key(&*e.conversation)
                && !codex_owner.contains_key(&*e.conversation)
            {
                let run = codex_run_for(&codex_runs, &resolve(&e.cwd), e.at);
                codex_owner.insert(&e.conversation, run.map(|r| r.account.clone()));
            }
        }
        // Runs remember their conversation, so usage stays attributed after a session is deleted.
        let mut runs_of: HashMap<&str, Vec<&Run>> = HashMap::new();
        for r in &runs {
            if let Some(c) = &r.conversation {
                runs_of.entry(c.as_str()).or_default().push(r);
            }
        }
        let account_of = |agent: Provider, conversation: &str, at: u64| -> Option<Option<String>> {
            if let Some(list) = runs_of.get(conversation) {
                return list
                    .iter()
                    .rev()
                    .find(|r| r.started_at <= at && r.ended_at.is_none_or(|end| at <= end))
                    .map(|r| r.account.clone());
            }
            if let Some(session) = by_conversation.get(conversation) {
                return run_at(&runs, session, at).map(|r| r.account.clone());
            }
            match agent {
                Provider::Codex => codex_owner.get(conversation).cloned().flatten(),
                Provider::Claude => None,
            }
        };

        let offset = local_offset();
        let today = local_day(now, offset);
        let mut report = aggregate(&events, today, days, offset, &place_of, &account_of);
        for a in &mut report.accounts {
            a.name = match &a.account {
                Some(id) => accounts
                    .iter()
                    .find(|x| &x.id == id)
                    .map_or_else(|| id.clone(), |x| x.name.clone()),
                None => format!(
                    "default {} login",
                    if a.provider == Provider::Claude {
                        "claude"
                    } else {
                        "codex"
                    }
                ),
            };
        }
        report
    }
}
