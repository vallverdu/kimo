//! `kimo agents | send | read | wait | new | whoami`: how agents (and people) talk to the other
//! agents from a terminal. Inside a kimo session `KIMO_URL` and `KIMO_SESSION` are set, so
//! messages are signed with the sending agent.

use crate::client::Client;
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub fn client(port: u16) -> Client {
    let url = std::env::var("KIMO_URL").unwrap_or_else(|_| format!("http://127.0.0.1:{port}"));
    Client::new(
        &url,
        std::env::var("KIMO_TOKEN").ok().filter(|t| !t.is_empty()),
    )
}

fn me() -> Option<String> {
    std::env::var("KIMO_SESSION").ok().filter(|s| !s.is_empty())
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn state(v: &Value) -> String {
    match (s(v, "status"), v["activity"].as_str()) {
        ("running", Some("waiting")) => "needs input".into(),
        ("running", Some(a)) => a.into(),
        (status, _) => status.into(),
    }
}

fn home(path: &str) -> String {
    std::env::var("HOME")
        .ok()
        .filter(|h| path.starts_with(h.as_str()))
        .map_or(path.to_string(), |h| format!("~{}", &path[h.len()..]))
}

fn enc(id: &str) -> String {
    id.replace('/', "%2F").replace(' ', "%20")
}

/// List every agent (and shell) of this kimo and of paired kimos (`host/id`).
pub fn agents(c: &Client, as_json: bool) -> Result<()> {
    let list: Value = c.get("/sessions?all=1")?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&list)?);
        return Ok(());
    }
    let me = me();
    println!(
        "{:<34} {:<20} {:<12} {:<7} {:<12} {:<38} FOLDER",
        "ID", "NAME", "HOST", "KIND", "STATE", "RESUME ID"
    );
    for a in list.as_array().into_iter().flatten() {
        let you = if me.as_deref() == Some(s(a, "id")) {
            " (you)"
        } else {
            ""
        };
        println!(
            "{:<34} {:<20} {:<12} {:<7} {:<12} {:<38} {}",
            format!("{}{you}", s(a, "id")),
            s(a, "name"),
            s(a, "host"),
            s(a, "kind"),
            state(a),
            a["conversation"].as_str().unwrap_or("-"),
            home(s(a, "cwd"))
        );
    }
    Ok(())
}

pub fn send(c: &Client, to: &str, message: &str) -> Result<()> {
    if message.trim().is_empty() {
        bail!("nothing to send");
    }
    let r = c.post(
        &format!("/sessions/{}/message", enc(to)),
        json!({ "text": message, "from": me() }),
    )?;
    println!("sent to {}", s(&r, "to"));
    Ok(())
}

pub fn read(c: &Client, agent: &str, screen: bool, lines: usize) -> Result<()> {
    let r: Value = if screen {
        c.get(&format!("/sessions/{}/screen?lines={lines}", enc(agent)))?
    } else {
        c.get(&format!("/sessions/{}/reply", enc(agent)))?
    };
    println!("{}", s(&r, "text"));
    Ok(())
}

/// Wait until the agent stops working (idle, needs input, or stopped).
pub fn wait(c: &Client, agent: &str, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        let v: Value = c.get(&format!("/sessions/{}", enc(agent)))?;
        let settled = s(&v, "status") != "running"
            || matches!(v["activity"].as_str(), Some("idle" | "waiting"));
        // The first moments after a message, the agent may not have started working yet.
        if settled && start.elapsed() > Duration::from_millis(1500) {
            println!(
                "{} is {} (read its answer with: kimo read {})",
                s(&v, "name"),
                state(&v),
                s(&v, "id")
            );
            return Ok(());
        }
        if start.elapsed() > timeout {
            bail!(
                "{} is still {} after {} s",
                s(&v, "name"),
                state(&v),
                timeout.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(700));
    }
}

/// Start a new agent in a folder, optionally with a first message once it is ready.
pub fn new(
    c: &Client,
    kind: &str,
    name: Option<String>,
    cwd: Option<String>,
    account: Option<String>,
    prompt: Option<String>,
) -> Result<()> {
    let cwd = cwd.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default()
    });
    let name = name.unwrap_or_else(|| format!("{kind} helper"));
    let created = c.post(
        "/sessions",
        json!({ "name": name, "kind": kind, "cwd": cwd, "account": account }),
    )?;
    let id = s(&created, "id").to_string();
    println!("started {id}");
    if let Some(prompt) = prompt.filter(|p| !p.trim().is_empty()) {
        // Wait for the agent's interface to be ready (quiet at its prompt), then send.
        let start = Instant::now();
        loop {
            let v: Value = c.get(&format!("/sessions/{id}"))?;
            if v["activity"] == "idle" && start.elapsed() > Duration::from_secs(2) {
                break;
            }
            if start.elapsed() > Duration::from_secs(90) {
                bail!("{id} did not get ready in 90 s; send the prompt with: kimo send {id} \"…\"");
            }
            std::thread::sleep(Duration::from_millis(700));
        }
        send(c, &id, &prompt)?;
    }
    Ok(())
}

pub fn whoami(c: &Client) -> Result<()> {
    let Some(id) = me() else {
        bail!("not inside a kimo session (KIMO_SESSION is not set)")
    };
    let v: Value = c.get(&format!("/sessions/{}", enc(&id)))?;
    println!(
        "{} ({}) · {} · {}",
        s(&v, "id"),
        s(&v, "name"),
        s(&v, "kind"),
        home(s(&v, "cwd"))
    );
    Ok(())
}

/// Paired kimos and whether they answer.
pub fn peers(c: &Client) -> Result<()> {
    let list: Value = c.get("/peers")?;
    if list.as_array().is_none_or(Vec::is_empty) {
        println!("No paired kimos. Pair one with: kimo peer add <its address> <pairing code>");
        return Ok(());
    }
    println!(
        "{:<16} {:<20} {:<36} {:<8} SESSIONS",
        "ID", "NAME", "ADDRESS", "ONLINE"
    );
    for p in list.as_array().into_iter().flatten() {
        println!(
            "{:<16} {:<20} {:<36} {:<8} {}",
            s(p, "id"),
            s(p, "name"),
            s(p, "url"),
            if p["online"] == true { "yes" } else { "no" },
            p["sessions"].as_u64().map_or("-".into(), |n| n.to_string())
        );
    }
    Ok(())
}

/// Pair with another kimo: its address and a pairing code shown there (`kimo pair` / Devices).
pub fn peer_add(c: &Client, url: &str, code: &str, my_url: Option<String>) -> Result<()> {
    let peer = c.post(
        "/peers",
        json!({ "url": url, "code": code, "my_url": my_url }),
    )?;
    println!(
        "paired with {} ({}); its agents are {}/<id> in `kimo agents`",
        s(&peer, "name"),
        s(&peer, "url"),
        s(&peer, "id")
    );
    Ok(())
}

pub fn peer_remove(c: &Client, id: &str) -> Result<()> {
    c.delete(&format!("/peers/{}", enc(id)))?;
    println!("removed {id}");
    Ok(())
}

fn ago(ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    let m = now.saturating_sub(ms) / 60_000;
    match m {
        0..=59 => format!("{m} min ago"),
        60..=1439 => format!("{} h ago", m / 60),
        _ => format!("{} d ago", m / 1440),
    }
}

fn folder_or_here(folder: Option<String>) -> String {
    folder.unwrap_or_else(|| {
        std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default()
    })
}

/// Earlier Claude and Codex conversations in a folder (default: the current one).
pub fn history(c: &Client, folder: Option<String>) -> Result<()> {
    let folder = folder_or_here(folder);
    let list: Value = c.get(&format!("/conversations?path={}", enc(&folder)))?;
    if list.as_array().is_none_or(Vec::is_empty) {
        println!("No Claude or Codex conversations in {}", home(&folder));
        return Ok(());
    }
    println!(
        "{:<7} {:<38} {:<11} {:<5} {:<20} TITLE",
        "AGENT", "CONVERSATION", "UPDATED", "ASKS", "SESSION"
    );
    for v in list.as_array().into_iter().flatten() {
        println!(
            "{:<7} {:<38} {:<11} {:<5} {:<20} {}",
            s(v, "agent"),
            s(v, "id"),
            ago(v["updated_at"].as_u64().unwrap_or(0)),
            v["messages"],
            v["session"].as_str().unwrap_or("-"),
            s(v, "title")
        );
    }
    Ok(())
}

/// Continue a conversation (from `kimo history`) as a kimo session.
pub fn continue_conversation(c: &Client, id: &str, folder: Option<String>) -> Result<()> {
    let folder = folder_or_here(folder);
    let list: Value = c.get(&format!("/conversations?path={}", enc(&folder)))?;
    let Some(conv) = list
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| s(v, "id") == id || s(v, "id").starts_with(id))
    else {
        bail!(
            "no conversation {id} in {}: see `kimo history`",
            home(&folder)
        );
    };
    let session = c.post(
        "/conversations/continue",
        json!({ "agent": conv["agent"], "cwd": folder, "id": conv["id"], "start": true }),
    )?;
    println!("continuing {} as {}", s(conv, "title"), s(&session, "id"));
    Ok(())
}

/// 1234 → 1.2K, 3400000 → 3.4M.
fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}K", n as f64 / 1e3),
        1_000_000..=999_999_999 => format!("{:.1}M", n as f64 / 1e6),
        _ => format!("{:.1}B", n as f64 / 1e9),
    }
}

/// Daily usage on this machine: per day, per project and per account.
pub fn usage(c: &Client, days: u32, as_json: bool) -> Result<()> {
    let r: Value = c.get(&format!("/usage/daily?days={days}"))?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&r)?);
        return Ok(());
    }
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let split = |v: &Value| {
        format!(
            "{} tokens (claude {} · codex {})",
            compact(n(&v["claude"]) + n(&v["codex"])),
            compact(n(&v["claude"])),
            compact(n(&v["codex"]))
        )
    };
    println!(
        "Last {days} days: {}, {} prompts, {} conversations",
        split(&r["total"]),
        n(&r["prompts"]),
        n(&r["conversations"])
    );
    println!(
        "Today {} · last 7 days {}",
        compact(n(&r["today"]["claude"]) + n(&r["today"]["codex"])),
        compact(n(&r["last_7_days"]["claude"]) + n(&r["last_7_days"]["codex"]))
    );
    println!("Tokens are new input, output and cache writes (cache reads not counted).\n");

    let list = |k: &str| r[k].as_array().cloned().unwrap_or_default();
    let max = list("days")
        .iter()
        .map(|d| n(&d["tokens"]["claude"]) + n(&d["tokens"]["codex"]))
        .max()
        .unwrap_or(0)
        .max(1);
    println!(
        "{:<11} {:>7} {:>7} {:>7}",
        "DAY", "CLAUDE", "CODEX", "PROMPTS"
    );
    for d in list("days") {
        let total = n(&d["tokens"]["claude"]) + n(&d["tokens"]["codex"]);
        let bar = "█".repeat(((total * 30).div_ceil(max)) as usize);
        println!(
            "{:<11} {:>7} {:>7} {:>7}  {bar}",
            s(&d, "date"),
            compact(n(&d["tokens"]["claude"])),
            compact(n(&d["tokens"]["codex"])),
            n(&d["prompts"])
        );
    }
    let places = list("places");
    if !places.is_empty() {
        println!(
            "\n{:<24} {:>7} {:>7} {:>7} {:>7} {:>5}  FOLDER",
            "PROJECT", "TOKENS", "CLAUDE", "CODEX", "PROMPTS", "CONVS"
        );
        for p in &places {
            let name = if p["project"].is_string() {
                s(p, "name").to_string()
            } else {
                format!("{} *", s(p, "name"))
            };
            println!(
                "{:<24} {:>7} {:>7} {:>7} {:>7} {:>5}  {}",
                name.chars().take(24).collect::<String>(),
                compact(n(&p["tokens"]["claude"]) + n(&p["tokens"]["codex"])),
                compact(n(&p["tokens"]["claude"])),
                compact(n(&p["tokens"]["codex"])),
                n(&p["prompts"]),
                n(&p["conversations"]),
                home(s(p, "path"))
            );
        }
        println!("* not a kimo project");
    }
    let accounts = list("accounts");
    if !accounts.is_empty() || n(&r["outside_kimo"]["claude"]) + n(&r["outside_kimo"]["codex"]) > 0
    {
        println!("\n{:<28} {:>7} {:>7}", "ACCOUNT", "TOKENS", "PROMPTS");
        for a in &accounts {
            println!(
                "{:<28} {:>7} {:>7}",
                s(a, "name"),
                compact(n(&a["tokens"])),
                n(&a["prompts"])
            );
        }
        println!(
            "{:<28} {:>7}",
            "run outside kimo",
            compact(n(&r["outside_kimo"]["claude"]) + n(&r["outside_kimo"]["codex"]))
        );
    }
    Ok(())
}
