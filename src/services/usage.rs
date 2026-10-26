//! Usage per subscription: Claude tokens from the transcripts of kimo's sessions, Codex limits
//! from its session logs, each attributed to the account the run used.

use crate::core::launch::default_home;
use crate::core::models::{Provider, Run};
use crate::core::usage::{
    claude_limits, claude_message, codex_limits, codex_meta, codex_run_for, run_at, with_pace,
    Limits, Tokens,
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
    /// Latest plan limits: Codex reports them in its logs, Claude through its API.
    pub limits: Option<Limits>,
    /// Why Claude's plan limits are not shown, when they are not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits_note: Option<String>,
}

/// Claude's plan limits per config folder, asked at most every two minutes.
type PlanCache = HashMap<Option<PathBuf>, (std::time::Instant, Result<Limits, String>)>;
static PLAN: std::sync::Mutex<Option<PlanCache>> = std::sync::Mutex::new(None);

/// The OAuth token of a Claude login: the Keychain on macOS, `.credentials.json` elsewhere.
/// `config_dir`: `CLAUDE_CONFIG_DIR` of a kimo account, `None` for this computer's login.
fn claude_token(config_dir: Option<&Path>, user_home: &Path) -> Option<String> {
    use crate::core::launch::{claude_access_token, claude_keychain_service};
    // This computer's login is in the Keychain of the person running kimo: only read it when
    // kimo runs with their home (never from tests or sandboxes with another home).
    let own_home = std::env::var_os("HOME").is_some_and(|h| Path::new(&h) == user_home);
    if cfg!(target_os = "macos") && (config_dir.is_some() || own_home) {
        let service = claude_keychain_service(config_dir.map(|d| d.to_str().unwrap_or_default()));
        let out = crate::services::network::run_for(
            std::process::Command::new("security").args([
                "find-generic-password",
                "-s",
                &service,
                "-w",
            ]),
            std::time::Duration::from_secs(60),
        )
        .ok()
        .filter(|o| o.status.success());
        if let Some(token) =
            out.and_then(|o| claude_access_token(&String::from_utf8_lossy(&o.stdout)))
        {
            return Some(token);
        }
    }
    let dir = config_dir.map_or_else(|| user_home.join(".claude"), Path::to_path_buf);
    std::fs::read_to_string(dir.join(".credentials.json"))
        .ok()
        .and_then(|t| claude_access_token(&t))
}

/// Codex's plan limits per account home, asked at most every two minutes.
type CodexCache = HashMap<PathBuf, (std::time::Instant, Result<Limits, String>)>;
static CODEX_PLAN: std::sync::Mutex<Option<CodexCache>> = std::sync::Mutex::new(None);

/// Codex's plan limits of a login, from Codex itself (`codex app-server`, the way its own
/// apps ask): kimo never reads Codex's credentials.
fn codex_plan(home: PathBuf, now: u64) -> Result<Limits, String> {
    let mut cache = CODEX_PLAN.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((at, found)) = cache.get(&home) {
        if at.elapsed() < std::time::Duration::from_secs(120) {
            return found.clone();
        }
    }
    let found = ask_codex(&home, now);
    if let Err(e) = &found {
        tracing::info!("codex plan limits ({}): {e}", home.display());
    }
    cache.insert(home, (std::time::Instant::now(), found.clone()));
    found
}

fn ask_codex(home: &Path, now: u64) -> Result<Limits, String> {
    use std::io::{BufRead, Write};
    use std::process::{Command, Stdio};
    let mut child = Command::new("codex")
        .arg("app-server")
        .env("CODEX_HOME", home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run codex: {e}"))?;
    let requests = [
        serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": { "clientInfo": { "name": "kimo", "version": env!("CARGO_PKG_VERSION") } } }),
        serde_json::json!({ "jsonrpc": "2.0", "method": "initialized" }),
        serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "account/rateLimits/read" }),
    ];
    if let Some(mut stdin) = child.stdin.take() {
        for r in &requests {
            let _ = writeln!(stdin, "{r}");
        }
        // stdin stays open until the answer: Codex stops when it closes.
        let (tx, rx) = std::sync::mpsc::channel();
        let stdout = child.stdout.take();
        std::thread::spawn(move || {
            let Some(out) = stdout else { return };
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if v["id"] == 2 {
                    let _ = tx.send(v);
                    return;
                }
            }
        });
        let answer = rx.recv_timeout(std::time::Duration::from_secs(15));
        drop(stdin);
        let _ = child.kill();
        let _ = child.wait();
        let v = answer.map_err(|_| "Codex did not answer".to_string())?;
        if let Some(e) = v["error"]["message"].as_str() {
            return Err(if e.contains("auth") || e.contains("login") {
                "sign in to see its plan limits".into()
            } else {
                format!("Codex: {e}")
            });
        }
        return crate::core::usage::codex_app_server_limits(&v["result"], now)
            .ok_or_else(|| "Codex sent no plan limits".into());
    }
    let _ = child.kill();
    let _ = child.wait();
    Err("cannot talk to codex".into())
}

/// Claude's 5-hour and weekly use of a login, from Claude's usage API.
fn claude_plan(config_dir: Option<PathBuf>, user_home: &Path, now: u64) -> Result<Limits, String> {
    let mut cache = PLAN.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some((at, found)) = cache.get(&config_dir) {
        if at.elapsed() < std::time::Duration::from_secs(120) {
            return found.clone();
        }
    }
    let found = (|| {
        let token = claude_token(config_dir.as_deref(), user_home)
            .ok_or("sign in to see its plan limits")?;
        // The token goes to curl on stdin, never on its command line.
        let config = format!(
            "header = \"Authorization: Bearer {token}\"\nheader = \"anthropic-beta: oauth-2025-04-20\"\n"
        );
        let out = crate::services::network::run_with_input(
            std::process::Command::new("curl").args([
                "-sS",
                "--max-time",
                "10",
                "-w",
                "\n%{http_code}",
                "--config",
                "-",
                "https://api.anthropic.com/api/oauth/usage",
            ]),
            Some(config.as_bytes()),
            std::time::Duration::from_secs(15),
        )
        .map_err(|e| format!("cannot reach Claude: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let (body, code) = text.rsplit_once('\n').unwrap_or((&text, ""));
        match code.trim() {
            "200" => {
                claude_limits(body, now).ok_or_else(|| "Claude sent no plan limits".to_string())
            }
            "401" | "403" => Err("updates the next time Claude runs on it".into()),
            "429" => Err("Claude is busy: limits update in a few minutes".into()),
            c => Err(format!("Claude's usage API answered {c}")),
        }
    })();
    if let Err(e) = &found {
        tracing::info!("claude plan limits: {e}");
    }
    cache.insert(config_dir, (std::time::Instant::now(), found.clone()));
    found
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
            let (_, email) = crate::services::accounts::system_login(&self.cfg.user_home, provider);
            out.insert(
                (provider, None),
                AccountUsage {
                    provider: Some(provider),
                    name: email.unwrap_or_else(|| "this computer's login".into()),
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
        {
            let home = default_home(Provider::Codex, &self.cfg.user_home);
            for log in codex_logs(&home, week) {
                let mut it = lines(&log);
                let Some((cwd, started)) = it.next().and_then(|l| codex_meta(&l)) else {
                    continue;
                };
                let cwd = crate::services::sessions::canonical(&cwd);
                // Codex run outside kimo used this computer's own login.
                let account =
                    codex_run_for(&codex_runs, &cwd, started).and_then(|r| r.account.clone());
                let mut latest: Option<Limits> = None;
                for line in it {
                    if let Some(l) = codex_limits(&line) {
                        latest = Some(l);
                    }
                }
                if let (Some(l), Some(u)) = (latest, out.get_mut(&(Provider::Codex, account))) {
                    u.last_activity = Some(u.last_activity.unwrap_or(0).max(l.at));
                    if u.limits.as_ref().is_none_or(|old| old.at < l.at) {
                        u.limits = Some(l);
                    }
                }
            }
        }

        // Plan limits from Claude's API and from Codex itself (when on in the settings).
        let limits_on = self.settings().claude_limits;
        let now_secs = now / 1000;
        for ((provider, account), u) in out.iter_mut() {
            if !limits_on {
                u.limits_note = Some("plan limits are off (show plan limits, below)".into());
                continue;
            }
            if *provider == Provider::Codex {
                let home = account.as_ref().map_or_else(
                    || default_home(Provider::Codex, &self.cfg.user_home),
                    |id| crate::core::launch::account_home(&self.cfg.data_dir, id),
                );
                // Live from Codex; its logs are the fallback.
                match codex_plan(home, now) {
                    Ok(l) => u.limits = Some(l),
                    Err(e) if u.limits.is_none() => u.limits_note = Some(e),
                    Err(_) => {}
                }
                continue;
            }
            let dir = account
                .as_ref()
                .map(|id| crate::core::launch::account_home(&self.cfg.data_dir, id));
            match claude_plan(dir, &self.cfg.user_home, now) {
                Ok(l) => u.limits = Some(l),
                Err(e) => u.limits_note = Some(e),
            }
        }
        for u in out.values_mut() {
            u.limits = u.limits.take().map(|l| with_pace(l, now_secs));
        }

        let mut list: Vec<AccountUsage> = out.into_values().collect();
        list.sort_by(|a, b| (a.account.is_some(), &a.name).cmp(&(b.account.is_some(), &b.name)));
        list
    }
}

#[cfg(test)]
mod live {
    /// Asks a real Codex login (`KIMO_TEST_CODEX_HOME`); run by hand with `--ignored`.
    #[test]
    #[ignore]
    fn codex_answers_its_plan_limits() {
        let home = std::env::var("KIMO_TEST_CODEX_HOME").expect("KIMO_TEST_CODEX_HOME");
        let l = super::ask_codex(std::path::Path::new(&home), 1).unwrap();
        for limit in &l.limits {
            println!(
                "{} {}% resets {}",
                limit.name, limit.used_percent, limit.resets_at
            );
        }
        assert!(!l.limits.is_empty());
    }
}
