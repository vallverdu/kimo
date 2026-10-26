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

/// One limit window of a plan (Codex reports them in its logs, Claude through its API).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Limit {
    pub name: String,
    pub used_percent: f64,
    pub window_minutes: u64,
    /// Unix seconds.
    pub resets_at: u64,
    /// Where the current pace leads by the reset (set by [`with_pace`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
}

/// Where the use of a limit is heading at its current pace.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Pace {
    /// Percent used at the reset if use goes on like since the window started.
    pub projected_percent: f64,
    /// Unix seconds when the limit runs out at this pace, if before the reset.
    pub runs_out_at: Option<u64>,
}

/// The pace of a limit at `now` (unix seconds): `None` too early in the window to tell
/// (under 5 % of it gone) or when the window is not known.
pub fn pace(l: &Limit, now: u64) -> Option<Pace> {
    let window = l.window_minutes * 60;
    if window == 0 || l.resets_at == 0 {
        return None;
    }
    let start = l.resets_at.saturating_sub(window);
    let elapsed = now.saturating_sub(start).min(window);
    if l.used_percent >= 100.0 {
        return Some(Pace {
            projected_percent: l.used_percent,
            runs_out_at: Some(now.min(l.resets_at)),
        });
    }
    if elapsed * 20 < window || now >= l.resets_at {
        return None;
    }
    let rate = l.used_percent / elapsed as f64;
    let left = (l.resets_at - now) as f64;
    let projected = l.used_percent + rate * left;
    let runs_out_at = (projected > 100.0 && rate > 0.0)
        .then(|| now + ((100.0 - l.used_percent) / rate).round() as u64);
    Some(Pace {
        projected_percent: projected,
        runs_out_at,
    })
}

/// The same limits with their pace filled in.
pub fn with_pace(mut limits: Limits, now: u64) -> Limits {
    for l in &mut limits.limits {
        l.pace = pace(l, now);
    }
    limits
}

/// `2026-10-26T22:00:00.123456+00:00` or `…Z` → unix seconds (Claude's usage API).
pub fn parse_iso_secs(s: &str) -> Option<u64> {
    let s = s.trim();
    let (main, offset) = match s.strip_suffix('Z') {
        Some(m) => (m, 0i64),
        None => {
            let at = s.rfind(['+', '-']).filter(|&i| i > 10)?;
            let (m, off) = s.split_at(at);
            let sign = if off.starts_with('-') { -1 } else { 1 };
            let mut hm = off[1..].split(':');
            let h: i64 = hm.next()?.parse().ok()?;
            let mi: i64 = hm.next().unwrap_or("0").parse().ok()?;
            (m, sign * (h * 3600 + mi * 60))
        }
    };
    let ms = parse_iso_ms(&format!("{main}Z"))? as i64;
    u64::try_from(ms / 1000 - offset).ok()
}

/// Plan limits from Claude's usage API (`/api/oauth/usage`): the 5-hour and weekly windows
/// and the weekly windows of single models.
pub fn claude_limits(json: &str, at_ms: u64) -> Option<Limits> {
    let v: Value = serde_json::from_str(json).ok()?;
    let window = |key: &str, name: &str, minutes: u64| -> Option<Limit> {
        let w = &v[key];
        Some(Limit {
            name: name.to_string(),
            used_percent: w["utilization"]
                .as_f64()
                .or(w["used_percentage"].as_f64())?,
            window_minutes: minutes,
            resets_at: w["resets_at"]
                .as_str()
                .and_then(parse_iso_secs)
                .unwrap_or(0),
            pace: None,
        })
    };
    let mut limits: Vec<Limit> = [
        window("five_hour", "5-hour", 300),
        window("seven_day", "weekly", 10_080),
        window("seven_day_opus", "weekly Opus", 10_080),
        window("seven_day_sonnet", "weekly Sonnet", 10_080),
    ]
    .into_iter()
    .flatten()
    .collect();
    for l in v["limits"].as_array().into_iter().flatten() {
        let model = l["scope"]["model"]["display_name"].as_str();
        if let (Some(model), Some(percent)) = (model, l["percent"].as_f64()) {
            limits.push(Limit {
                name: format!("weekly {model}"),
                used_percent: percent,
                window_minutes: 10_080,
                resets_at: l["resets_at"]
                    .as_str()
                    .and_then(parse_iso_secs)
                    .unwrap_or(0),
                pace: None,
            });
        }
    }
    if limits.is_empty() {
        return None;
    }
    let reached = limits
        .iter()
        .find(|l| l.used_percent >= 100.0)
        .map(|l| l.name.clone());
    Some(Limits {
        at: at_ms,
        plan: None,
        limits,
        reached,
    })
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
                pace: None,
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

/// A limit window's name: "5-hour", "weekly", else its length.
fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5-hour".into(),
        10_080 => "weekly".into(),
        m if m % 1440 == 0 => format!("{}-day", m / 1440),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-minute"),
    }
}

/// Codex's plan limits from its app server (`account/rateLimits/read`), read at `at` (ms).
/// Limits of a particular model carry its name ("weekly gpt-6.1").
pub fn codex_app_server_limits(result: &Value, at: u64) -> Option<Limits> {
    let by_id = result["rateLimitsByLimitId"].as_object();
    let entries: Vec<&Value> = match by_id {
        Some(map) if !map.is_empty() => map.values().collect(),
        _ => vec![&result["rateLimits"]],
    };
    let mut limits = Vec::new();
    let (mut plan, mut reached) = (None, None);
    for e in entries.into_iter().filter(|e| e.is_object()) {
        plan = plan.or_else(|| e["planType"].as_str().map(String::from));
        reached = reached.or_else(|| e["rateLimitReachedType"].as_str().map(String::from));
        let model = e["limitName"]
            .as_str()
            .or(e["normalModelSlug"].as_str())
            .filter(|_| e["limitId"] != "codex");
        for which in ["primary", "secondary"] {
            let l = &e[which];
            let Some(used) = l["usedPercent"].as_f64() else {
                continue;
            };
            let minutes = l["windowDurationMins"].as_u64().unwrap_or(0);
            let name = match model {
                Some(m) => format!("{} {m}", window_name(minutes)),
                None => window_name(minutes),
            };
            limits.push(Limit {
                name,
                used_percent: used,
                window_minutes: minutes,
                resets_at: l["resetsAt"].as_u64().unwrap_or(0),
                pace: None,
            });
        }
    }
    (!limits.is_empty()).then_some(Limits {
        at,
        plan,
        limits,
        reached,
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

/// Codex's own session id from the first line of its session log.
pub fn codex_session_id(line: &str) -> Option<String> {
    let v: Value = serde_json::from_str(line).ok()?;
    let p = &v["payload"];
    (v["type"] == "session_meta").then(|| {
        p["id"]
            .as_str()
            .or(p["session_id"].as_str())
            .map(String::from)
    })?
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
    #[test]
    fn codex_app_server() {
        let v: Value = serde_json::from_str(r#"{"ordinaryUsageAllowed":true,
            "rateLimits":{"limitId":"codex","limitName":null,"primary":{"usedPercent":4,"windowDurationMins":10080,"resetsAt":1793606504},"secondary":null,"planType":"self_serve_business_prolite","rateLimitReachedType":null},
            "rateLimitsByLimitId":{
              "codex":{"limitId":"codex","limitName":null,"primary":{"usedPercent":4,"windowDurationMins":10080,"resetsAt":1793606504},"secondary":{"usedPercent":12.5,"windowDurationMins":300,"resetsAt":1793000000},"planType":"pro"},
              "gpt-6.1":{"limitId":"gpt-6.1","limitName":"gpt-6.1","primary":{"usedPercent":15,"windowDurationMins":10080,"resetsAt":1793606504}}}}"#).unwrap();
        let l = codex_app_server_limits(&v, 7).unwrap();
        let mut got: Vec<(String, f64)> = l
            .limits
            .iter()
            .map(|l| (l.name.clone(), l.used_percent))
            .collect();
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            [
                ("5-hour".into(), 12.5),
                ("weekly".into(), 4.0),
                ("weekly gpt-6.1".into(), 15.0)
            ]
        );
        assert_eq!((l.at, l.plan.as_deref()), (7, Some("pro")));
        assert_eq!(window_name(1440 * 3), "3-day");
        assert!(codex_app_server_limits(&serde_json::json!({}), 1).is_none());
    }

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
            conversation: None,
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
        assert_eq!(
            codex_session_id(r#"{"type":"session_meta","payload":{"id":"019a","cwd":"/w"}}"#)
                .as_deref(),
            Some("019a")
        );
        assert_eq!(
            codex_session_id(r#"{"type":"event_msg","payload":{"id":"x"}}"#),
            None
        );
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

    #[test]
    fn pace_of_a_limit() {
        let l = |used: f64, resets_in: u64| Limit {
            name: "5-hour".into(),
            used_percent: used,
            window_minutes: 300,
            resets_at: 100_000 + resets_in,
            pace: None,
        };
        // Half the window gone, 60 % used: 120 % by the reset, out in 100 minutes.
        let p = pace(&l(60.0, 9000), 100_000).unwrap();
        assert!((p.projected_percent - 120.0).abs() < 1e-9);
        assert_eq!(p.runs_out_at, Some(100_000 + 6000));
        // On track: 20 % used halfway.
        let p = pace(&l(20.0, 9000), 100_000).unwrap();
        assert!((p.projected_percent - 40.0).abs() < 1e-9);
        assert_eq!(p.runs_out_at, None);
        // Too early to tell; already out.
        assert!(pace(&l(3.0, 17_900), 100_000).is_none());
        assert_eq!(
            pace(&l(100.0, 600), 100_000).unwrap().runs_out_at,
            Some(100_000)
        );
    }

    #[test]
    fn claude_usage_api() {
        assert_eq!(parse_iso_secs("1970-01-01T01:00:00.5+01:00"), Some(0));
        assert_eq!(parse_iso_secs("1970-01-01T00:00:10Z"), Some(10));
        let json = r#"{"five_hour":{"utilization":42.0,"resets_at":"1970-01-01T05:00:00+00:00"},
            "seven_day":{"utilization":100,"resets_at":"1970-01-08T00:00:00Z"},"seven_day_opus":null,
            "limits":[{"kind":"weekly_scoped","percent":7,"resets_at":"1970-01-08T00:00:00Z","scope":{"model":{"display_name":"Fable"}}}]}"#;
        let l = claude_limits(json, 1).unwrap();
        let names: Vec<&str> = l.limits.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["5-hour", "weekly", "weekly Fable"]);
        assert_eq!(l.limits[0].resets_at, 18_000);
        assert_eq!(l.reached.as_deref(), Some("weekly"));
        assert!(claude_limits("{}", 1).is_none());
    }
}
