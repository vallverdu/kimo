//! Earlier agent conversations in a folder (to continue them when the folder becomes a project).

use serde_json::Value;

/// The folder name Claude Code uses for a project path under `~/.claude/projects`
/// (every character that is not a letter or digit becomes `-`).
pub fn claude_dir_name(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// A conversation's short handle, to say and type: the first 8 characters of its id
/// (`c9c7d155` for `c9c7d155-162c-…`), like a short git hash.
pub fn short_id(id: &str) -> &str {
    id.char_indices().nth(8).map_or(id, |(at, _)| &id[..at])
}

/// The one id that `prefix` names (at least 4 characters, or the whole id).
pub fn resolve<'a>(ids: &[&'a str], prefix: &str) -> Result<&'a str, String> {
    let prefix = prefix.trim();
    if let Some(id) = ids.iter().find(|id| **id == prefix) {
        return Ok(id);
    }
    if prefix.len() < 4 {
        return Err("give at least 4 characters of the conversation id".into());
    }
    let mut found: Vec<&str> = ids
        .iter()
        .copied()
        .filter(|id| id.starts_with(prefix))
        .collect();
    found.sort_unstable();
    found.dedup();
    match found.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no conversation {prefix}")),
        many => Err(format!(
            "{prefix} names {} conversations ({}): give more of it",
            many.len(),
            many.iter()
                .map(|id| short_id(id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// What one conversation is about, from its log lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub id: Option<String>,
    pub cwd: Option<String>,
    /// The folder of its latest line: where it went on after being moved or continued elsewhere.
    pub last_cwd: Option<String>,
    pub title: Option<String>,
    /// First user prompt, a fallback title.
    pub first_prompt: Option<String>,
    pub messages: u32,
}

impl Summary {
    pub fn best_title(&self) -> String {
        let t = self
            .title
            .clone()
            .or_else(|| self.first_prompt.clone())
            .unwrap_or_else(|| "untitled".into());
        let t = t
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        if t.chars().count() > 90 {
            format!("{}…", t.chars().take(89).collect::<String>())
        } else {
            t
        }
    }
}

/// A user prompt worth showing (not tool results, commands or injected context).
fn prompt_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter(|c| c["type"] == "text" || c["type"] == "input_text")
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("Caveat:")).then(|| t.to_string())
}

/// Fold one Claude Code transcript line into the summary.
pub fn claude_line(s: &mut Summary, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    if let Some(c) = v["cwd"].as_str() {
        s.last_cwd = Some(c.to_string());
    }
    match v["type"].as_str() {
        Some("ai-title") => s.title = v["aiTitle"].as_str().map(String::from).or(s.title.take()),
        Some("summary") if s.title.is_none() => s.title = v["summary"].as_str().map(String::from),
        Some("user") => {
            if s.cwd.is_none() {
                s.cwd = v["cwd"].as_str().map(String::from);
            }
            if let Some(p) = prompt_text(&v["message"]["content"]) {
                s.messages += 1;
                if s.first_prompt.is_none() {
                    s.first_prompt = Some(p);
                }
            }
        }
        Some("assistant") if s.cwd.is_none() => s.cwd = v["cwd"].as_str().map(String::from),
        _ => {}
    }
    if s.id.is_none() {
        s.id = v["sessionId"].as_str().map(String::from);
    }
}

/// Fold one Codex session log line into the summary.
pub fn codex_line(s: &mut Summary, line: &str) {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let p = &v["payload"];
    match (v["type"].as_str(), p["type"].as_str()) {
        (Some("session_meta"), _) => {
            s.id = p["id"]
                .as_str()
                .or(p["session_id"].as_str())
                .map(String::from);
            s.cwd = p["cwd"].as_str().map(String::from);
            s.last_cwd.clone_from(&s.cwd);
        }
        (Some("event_msg"), Some("item_completed")) if p["item"]["type"] == "UserMessage" => {
            if let Some(t) = prompt_text(&p["item"]["content"]) {
                s.messages += 1;
                if s.first_prompt.is_none() {
                    s.first_prompt = Some(t);
                }
            }
        }
        (Some("event_msg"), Some("user_message")) => {
            if let Some(t) = p["message"].as_str().filter(|t| !t.trim().is_empty()) {
                s.messages += 1;
                if s.first_prompt.is_none() {
                    s.first_prompt = Some(t.to_string());
                }
            }
        }
        (Some("event_msg"), Some("thread_name_updated")) => {
            s.title = p["thread_name"]
                .as_str()
                .map(String::from)
                .or(s.title.take());
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_folder_names() {
        assert_eq!(claude_dir_name("/home/me/dev/kimo"), "-home-me-dev-kimo");
        assert_eq!(claude_dir_name("/home/me/my.app_v2"), "-home-me-my-app-v2");
    }

    #[test]
    fn claude_summary() {
        let mut s = Summary::default();
        for line in [
            r#"{"type":"user","cwd":"/w","sessionId":"abc","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","cwd":"/w","message":{"role":"user","content":[{"type":"text","text":"Fix the parser\nplease"}]}}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"ok"}]}}"#,
            r#"{"type":"assistant","cwd":"/w","message":{"content":[{"type":"text","text":"Done"}]}}"#,
            r#"{"type":"user","message":{"content":"Thanks, now the tests"}}"#,
        ] {
            claude_line(&mut s, line);
        }
        assert_eq!(
            (s.id.as_deref(), s.cwd.as_deref(), s.messages),
            (Some("abc"), Some("/w"), 2)
        );
        assert_eq!(s.best_title(), "Fix the parser");
        claude_line(&mut s, r#"{"type":"ai-title","aiTitle":"Parser fixes"}"#);
        assert_eq!(s.best_title(), "Parser fixes");
    }

    #[test]
    fn codex_summary() {
        let mut s = Summary::default();
        for line in [
            r#"{"type":"session_meta","payload":{"id":"019a","cwd":"/w"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}"#,
            r#"{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"Build the game"}]}}}"#,
        ] {
            codex_line(&mut s, line);
        }
        assert_eq!(
            (s.id.as_deref(), s.cwd.as_deref(), s.messages),
            (Some("019a"), Some("/w"), 1)
        );
        assert_eq!(s.best_title(), "Build the game");
        assert_eq!(
            Summary {
                title: Some("x".repeat(100)),
                ..Default::default()
            }
            .best_title()
            .chars()
            .count(),
            90
        );
    }

    #[test]
    fn short_ids_and_prefixes() {
        assert_eq!(short_id("c9c7d155-162c-412f-a4c9-0ea2c7798f92"), "c9c7d155");
        assert_eq!(short_id("019a"), "019a");
        let ids = ["c9c7d155-162c", "c9c7aaaa-0000", "4ce978b3-055c"];
        assert_eq!(resolve(&ids, "4ce9"), Ok("4ce978b3-055c"));
        assert_eq!(resolve(&ids, "c9c7d155-162c"), Ok("c9c7d155-162c"));
        assert!(resolve(&ids, "c9c7")
            .unwrap_err()
            .contains("(c9c7aaaa, c9c7d155)"));
        assert!(resolve(&ids, "4ce").is_err());
        assert!(resolve(&ids, "ffff")
            .unwrap_err()
            .starts_with("no conversation"));
    }

    #[test]
    fn a_moved_conversation_remembers_where_it_went() {
        let mut s = Summary::default();
        for line in [
            r#"{"type":"user","cwd":"/a","sessionId":"x","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"user","cwd":"/a/b","message":{"role":"user","content":"go on"}}"#,
        ] {
            claude_line(&mut s, line);
        }
        assert_eq!(
            (s.cwd.as_deref(), s.last_cwd.as_deref()),
            (Some("/a"), Some("/a/b"))
        );
    }
}
