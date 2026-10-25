//! Daily usage across every agent conversation on the machine: what the agents' own logs
//! record (tokens, prompts, folder, time), grouped by day, project and account.

use super::models::Provider;
use super::usage::{parse_iso_ms, Tokens};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// One thing that happened in a conversation: an answer (with tokens) or a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub at: u64,
    pub agent: Provider,
    /// Folder the conversation ran in.
    pub cwd: Arc<str>,
    /// The agent's conversation id.
    pub conversation: Arc<str>,
    pub tokens: Tokens,
    pub prompt: bool,
    /// Hash of the message id, to count streamed or copied messages once.
    pub id: Option<u64>,
}

fn hash_id(id: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

fn prompt_text(content: &Value) -> bool {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|c| c["type"] == "text" || c["type"] == "input_text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join(" "),
        _ => return false,
    };
    let t = text.trim();
    !t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")
}

/// The event of one Claude Code transcript line (`conversation`: the file's id, used when the
/// line names none).
pub fn claude_event(line: &str, conversation: &str) -> Option<Event> {
    if !(line.contains("\"assistant\"") || line.contains("\"user\"")) {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let at = parse_iso_ms(v["timestamp"].as_str()?)?;
    let cwd: Arc<str> = v["cwd"].as_str().unwrap_or_default().into();
    let conversation: Arc<str> = v["sessionId"].as_str().unwrap_or(conversation).into();
    let base = |tokens, prompt, id: Option<&str>| Event {
        at,
        agent: Provider::Claude,
        cwd: cwd.clone(),
        conversation: conversation.clone(),
        tokens,
        prompt,
        id: id.map(hash_id),
    };
    match v["type"].as_str()? {
        "assistant" => {
            let u = &v["message"]["usage"];
            if !u.is_object() {
                return None;
            }
            let n = |k: &str| u[k].as_u64().unwrap_or(0);
            let tokens = Tokens {
                input: n("input_tokens"),
                output: n("output_tokens"),
                cache_write: n("cache_creation_input_tokens"),
                cache_read: n("cache_read_input_tokens"),
            };
            Some(base(tokens, false, v["message"]["id"].as_str()))
        }
        "user" if prompt_text(&v["message"]["content"]) => {
            Some(base(Tokens::default(), true, v["uuid"].as_str()))
        }
        _ => None,
    }
}

/// Reads a Codex session log line by line (the folder and id come from its first line).
#[derive(Debug, Clone)]
pub struct CodexReader {
    pub cwd: Arc<str>,
    pub conversation: Arc<str>,
}

impl Default for CodexReader {
    fn default() -> Self {
        Self {
            cwd: "".into(),
            conversation: "".into(),
        }
    }
}

impl CodexReader {
    pub fn event(&mut self, line: &str) -> Option<Event> {
        let v: Value = serde_json::from_str(line).ok()?;
        let p = &v["payload"];
        if v["type"] == "session_meta" {
            self.cwd = p["cwd"].as_str().unwrap_or_default().into();
            self.conversation = p["id"]
                .as_str()
                .or(p["session_id"].as_str())
                .unwrap_or_default()
                .into();
            return None;
        }
        let at = parse_iso_ms(v["timestamp"].as_str()?)?;
        let base = |tokens, prompt| Event {
            at,
            agent: Provider::Codex,
            cwd: self.cwd.clone(),
            conversation: self.conversation.clone(),
            tokens,
            prompt,
            id: None,
        };
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("event_msg"), Some("token_count")) => {
                let u = &p["info"]["last_token_usage"];
                if !u.is_object() {
                    return None;
                }
                let n = |k: &str| u[k].as_u64().unwrap_or(0);
                let cached = n("cached_input_tokens");
                Some(base(
                    Tokens {
                        input: n("input_tokens").saturating_sub(cached),
                        output: n("output_tokens"),
                        cache_write: 0,
                        cache_read: cached,
                    },
                    false,
                ))
            }
            (Some("event_msg"), Some("item_completed"))
                if p["item"]["type"] == "UserMessage" && prompt_text(&p["item"]["content"]) =>
            {
                Some(base(Tokens::default(), true))
            }
            _ => None,
        }
    }
}

/// Days since 1970-01-01 in local time (`offset_secs` east of UTC).
pub fn local_day(at_ms: u64, offset_secs: i64) -> i64 {
    (at_ms as i64 / 1000 + offset_secs).div_euclid(86_400)
}

/// `YYYY-MM-DD` of a day number (civil from days, Howard Hinnant).
pub fn day_label(day: i64) -> String {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct Split {
    pub claude: u64,
    pub codex: u64,
}

impl Split {
    fn add(&mut self, agent: Provider, n: u64) {
        match agent {
            Provider::Claude => self.claude += n,
            Provider::Codex => self.codex += n,
        }
    }

    pub fn total(&self) -> u64 {
        self.claude + self.codex
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Day {
    pub date: String,
    /// Tokens (new input + output + cache writes) per agent.
    pub tokens: Split,
    pub prompts: u64,
    pub cache_read: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Place {
    pub name: String,
    pub path: String,
    /// kimo project id, when the folder is (inside) a project.
    pub project: Option<String>,
    pub tokens: Split,
    pub prompts: u64,
    pub conversations: u64,
    pub last_active: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountTotal {
    /// kimo account id, `None` for the default login.
    pub account: Option<String>,
    pub name: String,
    pub provider: Provider,
    pub tokens: u64,
    pub prompts: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub days: Vec<Day>,
    pub places: Vec<Place>,
    pub accounts: Vec<AccountTotal>,
    /// Tokens of conversations not run through kimo (account unknown).
    pub outside_kimo: Split,
    pub today: Split,
    pub last_7_days: Split,
    pub total: Split,
    pub prompts: u64,
    pub conversations: u64,
}

/// Where a folder belongs: (name, path, kimo project id).
pub type PlaceOf<'a> = dyn Fn(&str) -> (String, String, Option<String>) + 'a;
/// Which kimo account a conversation used at a time: `None` = not run through kimo,
/// `Some(None)` = run through kimo on the default login.
pub type AccountOf<'a> = dyn Fn(Provider, &str, u64) -> Option<Option<String>> + 'a;

/// Group events of `days` local days ending on `today` (inclusive).
pub fn aggregate(
    events: &[Event],
    today: i64,
    days: i64,
    offset: i64,
    place_of: &PlaceOf,
    account_of: &AccountOf,
) -> Report {
    let first = today - days + 1;
    let mut by_day: BTreeMap<i64, Day> = (first..=today)
        .map(|d| {
            (
                d,
                Day {
                    date: day_label(d),
                    ..Default::default()
                },
            )
        })
        .collect();
    let mut places: HashMap<String, Place> = HashMap::new();
    let mut place_convs: HashMap<String, HashSet<Arc<str>>> = HashMap::new();
    let mut accounts: HashMap<(Provider, Option<String>), AccountTotal> = HashMap::new();
    let mut seen: HashSet<(Provider, u64)> = HashSet::new();
    let mut convs: HashSet<(Provider, Arc<str>)> = HashSet::new();
    let mut report = Report::default();
    for e in events {
        let day = local_day(e.at, offset);
        if day < first || day > today {
            continue;
        }
        if let Some(id) = e.id {
            if !seen.insert((e.agent, id)) {
                continue;
            }
        }
        let tokens = e.tokens.billable();
        let d = by_day.get_mut(&day).expect("day in range");
        d.tokens.add(e.agent, tokens);
        d.cache_read += e.tokens.cache_read;
        d.prompts += u64::from(e.prompt);
        report.total.add(e.agent, tokens);
        report.prompts += u64::from(e.prompt);
        if day == today {
            report.today.add(e.agent, tokens);
        }
        if day > today - 7 {
            report.last_7_days.add(e.agent, tokens);
        }
        convs.insert((e.agent, e.conversation.clone()));
        let (name, path, project) = place_of(&e.cwd);
        let place = places.entry(path.clone()).or_insert_with(|| Place {
            name,
            path: path.clone(),
            project,
            ..Default::default()
        });
        place.tokens.add(e.agent, tokens);
        place.prompts += u64::from(e.prompt);
        place.last_active = place.last_active.max(e.at);
        place_convs
            .entry(path)
            .or_default()
            .insert(e.conversation.clone());
        match account_of(e.agent, &e.conversation, e.at) {
            Some(account) => {
                let a = accounts
                    .entry((e.agent, account.clone()))
                    .or_insert_with(|| AccountTotal {
                        account,
                        name: String::new(),
                        provider: e.agent,
                        tokens: 0,
                        prompts: 0,
                    });
                a.tokens += tokens;
                a.prompts += u64::from(e.prompt);
            }
            None => report.outside_kimo.add(e.agent, tokens),
        }
    }
    report.days = by_day.into_values().collect();
    let mut places: Vec<Place> = places
        .into_values()
        .map(|mut p| {
            p.conversations = place_convs.get(&p.path).map_or(0, |c| c.len() as u64);
            p
        })
        .filter(|p| p.tokens.total() > 0 || p.prompts > 0)
        .collect();
    places.sort_by_key(|p| std::cmp::Reverse(p.tokens.total()));
    // Folders with the same name (`a/app`, `b/app`) show their parent too.
    let mut names: HashMap<String, usize> = HashMap::new();
    for p in &places {
        *names.entry(p.name.clone()).or_default() += 1;
    }
    for p in &mut places {
        if names[&p.name] > 1 {
            if let Some(parent) = std::path::Path::new(&p.path)
                .parent()
                .and_then(|d| d.file_name())
            {
                p.name = format!("{}/{}", parent.to_string_lossy(), p.name);
            }
        }
    }
    report.places = places;
    let mut accounts: Vec<AccountTotal> = accounts.into_values().collect();
    accounts.sort_by_key(|a| std::cmp::Reverse(a.tokens));
    report.accounts = accounts;
    report.conversations = convs.len() as u64;
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days() {
        assert_eq!(day_label(0), "1970-01-01");
        assert_eq!(day_label(20_386), "2025-10-25");
        assert_eq!(local_day(86_400_000 - 1, 0), 0);
        assert_eq!(
            local_day(86_400_000 - 1000, 3600),
            1,
            "an hour east is already tomorrow"
        );
        assert_eq!(local_day(1000, -3600), -1);
    }

    #[test]
    fn claude_and_codex_events() {
        let a = r#"{"type":"assistant","timestamp":"1970-01-02T00:00:00Z","cwd":"/w","message":{"id":"m1","usage":{"input_tokens":10,"output_tokens":5,"cache_creation_input_tokens":2,"cache_read_input_tokens":100}}}"#;
        let e = claude_event(a, "c1").unwrap();
        assert_eq!(
            (
                e.at,
                &*e.cwd,
                &*e.conversation,
                e.tokens.billable(),
                e.prompt
            ),
            (86_400_000, "/w", "c1", 17, false)
        );
        let u = r#"{"type":"user","timestamp":"1970-01-02T00:00:00Z","cwd":"/w","uuid":"u1","message":{"content":"Fix it"}}"#;
        assert!(claude_event(u, "c1").unwrap().prompt);
        assert!(claude_event(r#"{"type":"user","timestamp":"1970-01-02T00:00:00Z","message":{"content":[{"type":"tool_result"}]}}"#, "c1").is_none());

        let mut r = CodexReader::default();
        assert!(r
            .event(r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/g"}}"#)
            .is_none());
        let t = r.event(r#"{"timestamp":"1970-01-02T00:00:00Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":80,"output_tokens":7}}}}"#).unwrap();
        assert_eq!(
            (
                &*t.cwd,
                &*t.conversation,
                t.tokens.billable(),
                t.tokens.cache_read
            ),
            ("/g", "x1", 27, 80)
        );
        let p = r.event(r#"{"timestamp":"1970-01-02T00:00:00Z","type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"Go"}]}}}"#).unwrap();
        assert!(p.prompt);
    }

    #[test]
    fn aggregates_by_day_place_and_account() {
        let ev =
            |at_day: u64, agent, cwd: &str, conv: &str, out: u64, prompt, id: Option<&str>| Event {
                at: at_day * 86_400_000 + 3_600_000,
                agent,
                cwd: cwd.into(),
                conversation: conv.into(),
                tokens: Tokens {
                    output: out,
                    ..Default::default()
                },
                prompt,
                id: id.map(hash_id),
            };
        let events = vec![
            ev(
                10,
                Provider::Claude,
                "/p/app/src",
                "c1",
                100,
                false,
                Some("m1"),
            ),
            ev(
                10,
                Provider::Claude,
                "/p/app/src",
                "c1",
                100,
                false,
                Some("m1"),
            ), // streamed copy
            ev(10, Provider::Claude, "/p/app", "c1", 0, true, None),
            ev(12, Provider::Codex, "/p/app", "x1", 50, false, None),
            ev(12, Provider::Codex, "/tmp/scratch", "x2", 7, false, None),
            ev(12, Provider::Codex, "/srv/scratch", "x3", 3, false, None),
            ev(2, Provider::Codex, "/p/app", "x1", 999, false, None), // before the range
        ];
        let place_of = |cwd: &str| {
            if cwd.starts_with("/p/app") {
                (
                    "app".to_string(),
                    "/p/app".to_string(),
                    Some("app-1".to_string()),
                )
            } else {
                ("scratch".to_string(), cwd.to_string(), None)
            }
        };
        let account_of = |agent: Provider, conv: &str, _at: u64| match (agent, conv) {
            (Provider::Claude, "c1") => Some(Some("work".to_string())),
            (Provider::Codex, "x1") => Some(None),
            _ => None,
        };
        let r = aggregate(&events, 12, 3, 0, &place_of, &account_of);
        assert_eq!(
            r.days
                .iter()
                .map(|d| (d.date.as_str(), d.tokens.total(), d.prompts))
                .collect::<Vec<_>>(),
            vec![
                ("1970-01-11", 100, 1),
                ("1970-01-12", 0, 0),
                ("1970-01-13", 60, 0),
            ]
        );
        assert_eq!(
            (
                r.total.claude,
                r.total.codex,
                r.today.total(),
                r.prompts,
                r.conversations
            ),
            (100, 60, 60, 1, 4)
        );
        assert_eq!(
            (
                r.places[0].name.as_str(),
                r.places[0].tokens.total(),
                r.places[0].conversations,
                r.places[0].project.as_deref()
            ),
            ("app", 150, 2, Some("app-1"))
        );
        assert_eq!(
            (r.places[1].name.as_str(), r.places[2].name.as_str()),
            ("tmp/scratch", "srv/scratch"),
            "same names show their parent"
        );
        assert_eq!(r.accounts[0].account.as_deref(), Some("work"));
        assert_eq!(
            (
                r.accounts[0].tokens,
                r.accounts[1].tokens,
                r.outside_kimo.codex
            ),
            (100, 50, 10)
        );
    }
}
