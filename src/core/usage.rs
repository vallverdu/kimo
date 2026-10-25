//! Usage per subscription from the agents' own logs (pure parsing and attribution).

use super::models::{Provider, Run};
use serde::Serialize;
use serde_json::Value;

/// `2026-10-13T23:14:18.015Z` → unix milliseconds (UTC only, as the agents write it).
pub fn parse_iso_ms(s: &str) -> Option<u64> {
    let s = s.trim().strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (hms, frac) = time.split_once('.').unwrap_or((time, "0"));
    let mut t = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (hh, mm, ss) = (t.next()??, t.next()??, t.next()??);
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    // Days from civil (Howard Hinnant).
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let total = ((days * 24 + hh) * 60 + mm) * 60 + ss;
    u64::try_from(total * 1000 + ms).ok()
}

/// Tokens of one Claude assistant message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaudeMessage {
    pub id: String,
    pub at: u64,
    pub tokens: Tokens,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Tokens {
    pub fn add(&mut self, o: Tokens) {
        self.input += o.input;
        self.output += o.output;
        self.cache_write += o.cache_write;
        self.cache_read += o.cache_read;
    }

    /// What counts against a plan the most: new input, output and cache writes.
    pub fn billable(&self) -> u64 {
        self.input + self.output + self.cache_write
    }
}

/// An assistant message with usage from a Claude Code transcript line.
pub fn claude_message(line: &str) -> Option<ClaudeMessage> {
    if !line.contains("\"usage\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "assistant" {
        return None;
    }
    let u = &v["message"]["usage"];
    let n = |k: &str| u[k].as_u64().unwrap_or(0);
    Some(ClaudeMessage {
        id: v["message"]["id"].as_str().unwrap_or_default().to_string(),
        at: parse_iso_ms(v["timestamp"].as_str()?)?,
        tokens: Tokens {
            input: n("input_tokens"),
            output: n("output_tokens"),
            cache_write: n("cache_creation_input_tokens"),
            cache_read: n("cache_read_input_tokens"),
        },
    })
}

/// One limit window of a Codex plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limit {
    pub name: String,
    pub used_percent: f64,
    pub window_minutes: u64,
    /// Unix seconds.
    pub resets_at: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limits {
    pub at: u64,
    pub plan: Option<String>,
    pub limits: Vec<Limit>,
    /// Set by Codex when a limit has been hit.
    pub reached: Option<String>,
}

/// Rate-limit snapshot from a Codex session log line.
pub fn codex_limits(line: &str) -> Option<Limits> {
    if !line.contains("\"rate_limits\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let r = &v["payload"]["rate_limits"];
    if !r.is_object() {
        return None;
    }
    let limits = ["primary", "secondary"]
        .iter()
        .filter_map(|name| {
            let l = &r[*name];
            Some(Limit {
                name: name.to_string(),
                used_percent: l["used_percent"].as_f64()?,
                window_minutes: l["window_minutes"].as_u64().unwrap_or(0),
                resets_at: l["resets_at"].as_u64().unwrap_or(0),
            })
        })
        .collect();
    Some(Limits {
        at: parse_iso_ms(v["timestamp"].as_str()?)?,
        plan: r["plan_type"].as_str().map(String::from),
        limits,
        reached: r["rate_limit_reached_type"].as_str().map(String::from),
    })
}

/// `(cwd, started)` from the first line of a Codex session log.
pub fn codex_meta(line: &str) -> Option<(String, u64)> {
    let v: Value = serde_json::from_str(line).ok()?;
    if v["type"] != "session_meta" {
        return None;
    }
    let p = &v["payload"];
    Some((
        p["cwd"].as_str()?.to_string(),
        parse_iso_ms(p["timestamp"].as_str().or(v["timestamp"].as_str())?)?,
    ))
}

/// The run of `session` that was active at `at` (runs of one session never overlap).
pub fn run_at<'a>(runs: &'a [Run], session: &str, at: u64) -> Option<&'a Run> {
    runs.iter().rev().find(|r| {
        r.session == session && r.started_at <= at && r.ended_at.is_none_or(|end| at <= end)
    })
}

/// The Codex run a session log belongs to: same folder, started during the run.
pub fn codex_run_for<'a>(runs: &'a [Run], cwd: &str, started: u64) -> Option<&'a Run> {
    runs.iter().rev().find(|r| {
        r.provider == Provider::Codex
            && r.cwd == cwd
            && r.started_at.saturating_sub(5_000) <= started
            && r.ended_at.is_none_or(|end| started <= end)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        session: &str,
        provider: Provider,
        account: Option<&str>,
        start: u64,
        end: Option<u64>,
    ) -> Run {
        Run {
            session: session.into(),
            provider,
            account: account.map(String::from),
            cwd: "/w".into(),
            started_at: start,
            ended_at: end,
            extra: Default::default(),
        }
    }

    #[test]
    fn iso_timestamps() {
        assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_iso_ms("2026-10-13T23:14:18.015Z"),
            Some(1_791_933_258_015)
        );
        assert_eq!(
            parse_iso_ms("2024-02-29T12:00:00.5Z"),
            Some(1_709_208_000_500)
        );
        assert_eq!(parse_iso_ms("2026-10-13 23:14:18"), None);
    }

    #[test]
    fn claude_lines() {
        let line = r#"{"type":"assistant","timestamp":"1970-01-01T00:00:01Z","message":{"id":"msg_1","usage":{"input_tokens":2,"cache_creation_input_tokens":100,"cache_read_input_tokens":5000,"output_tokens":30}}}"#;
        let m = claude_message(line).unwrap();
        assert_eq!((m.id.as_str(), m.at), ("msg_1", 1000));
        assert_eq!(m.tokens.billable(), 132);
        assert!(claude_message(r#"{"type":"user","message":{"usage":{}}}"#).is_none());
    }

    #[test]
    fn codex_lines() {
        let line = r#"{"timestamp":"1970-01-01T00:00:02Z","type":"event_msg","payload":{"type":"token_count","rate_limits":{"primary":{"used_percent":15.0,"window_minutes":10080,"resets_at":99},"secondary":null,"plan_type":"pro","rate_limit_reached_type":null}}}"#;
        let l = codex_limits(line).unwrap();
        assert_eq!(
            (l.at, l.plan.as_deref(), l.limits.len()),
            (2000, Some("pro"), 1)
        );
        assert_eq!(l.limits[0].used_percent, 15.0);
        let meta = r#"{"timestamp":"1970-01-01T00:00:03Z","type":"session_meta","payload":{"cwd":"/w","timestamp":"1970-01-01T00:00:03Z"}}"#;
        assert_eq!(codex_meta(meta), Some(("/w".into(), 3000)));
    }

    #[test]
    fn attribution() {
        let runs = vec![
            run("s", Provider::Claude, Some("a"), 0, Some(100)),
            run("s", Provider::Claude, Some("b"), 100, None),
            run("c", Provider::Codex, Some("x"), 1000, Some(2000)),
        ];
        assert_eq!(
            run_at(&runs, "s", 50).unwrap().account.as_deref(),
            Some("a")
        );
        assert_eq!(
            run_at(&runs, "s", 5000).unwrap().account.as_deref(),
            Some("b")
        );
        assert!(run_at(&runs, "other", 50).is_none());
        assert_eq!(
            codex_run_for(&runs, "/w", 1500).unwrap().account.as_deref(),
            Some("x")
        );
        assert!(codex_run_for(&runs, "/w", 2500).is_none());
        assert!(codex_run_for(&runs, "/elsewhere", 1500).is_none());
    }
}
