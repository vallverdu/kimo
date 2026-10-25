//! An agent's replies from its own log, for other agents to read (`kimo read`).

use serde_json::Value;

fn texts(content: &Value, kind: &str) -> Option<String> {
    let parts: Vec<&str> = content
        .as_array()?
        .iter()
        .filter(|c| c["type"] == kind)
        .filter_map(|c| c["text"].as_str())
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

/// Text of an assistant message in a Claude Code transcript line.
pub fn claude_reply(line: &str) -> Option<String> {
    if !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    (v["type"] == "assistant").then(|| texts(&v["message"]["content"], "text"))?
}

/// Text of an assistant message in a Codex session log line.
pub fn codex_reply(line: &str) -> Option<String> {
    if !line.contains("\"assistant\"") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let p = &v["payload"];
    (v["type"] == "response_item" && p["type"] == "message" && p["role"] == "assistant")
        .then(|| texts(&p["content"], "output_text"))?
}

/// The last `lines` non-empty lines of a text.
pub fn last_lines(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// What another agent receives: who it is from and how to answer.
pub fn envelope(from: Option<(&str, &str)>, text: &str) -> String {
    match from {
        Some((id, name)) => format!(
            "[kimo message from agent {name} (id {id}); reply with: kimo send {id} \"…\"] {text}"
        ),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replies() {
        let claude = r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"Tests pass."}]}}"#;
        assert_eq!(claude_reply(claude).as_deref(), Some("Tests pass."));
        assert_eq!(
            claude_reply(r#"{"type":"assistant","message":{"content":[{"type":"tool_use"}]}}"#),
            None
        );
        let codex = r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Done."}]}}"#;
        assert_eq!(codex_reply(codex).as_deref(), Some("Done."));
        assert_eq!(
            codex_reply(
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[]}}"#
            ),
            None
        );
    }

    #[test]
    fn helpers() {
        assert_eq!(last_lines("a\n\nb\nc\n   \n", 2), "b\nc");
        assert!(envelope(Some(("api-1a2b", "api")), "hi").starts_with(
            "[kimo message from agent api (id api-1a2b); reply with: kimo send api-1a2b"
        ));
        assert_eq!(envelope(None, "hi"), "hi");
    }
}
