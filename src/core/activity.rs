//! What a running session is doing, from its output: working, waiting for you, or idle.
//!
//! Agents animate a spinner while they work, so recent output means working. When the output
//! stops, the last screen tells whether the agent asks something (approval, question) or simply
//! waits at its prompt.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Activity {
    /// Output in the last few seconds.
    Working,
    /// Quiet, and the screen asks for an answer or an approval.
    Waiting,
    /// Quiet at its prompt.
    Idle,
}

/// Output younger than this means the session is working.
pub const WORKING_MS: u64 = 2500;

/// Phrases of approval prompts and questions (Claude Code, Codex, common CLIs), lowercase.
const WAITING: &[&str] = &[
    "do you want to proceed",
    "do you want to make this edit",
    "do you want to create",
    "do you want to allow",
    "would you like to run",
    "would you like to make",
    "allow command?",
    "approve this",
    "❯ 1. yes",
    "› 1. yes",
    "(y/n)",
    "[y/n]",
    "(yes/no)",
    "press enter to continue",
];

/// Text of terminal output without escape sequences (CSI, OSC, two-byte ESC sequences).
pub fn strip_ansi(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') | Some('P') | Some('_') => {
                    while let Some(c) = chars.next() {
                        if c == '\x07' || (c == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                            break;
                        }
                    }
                }
                // ESC + intermediate bytes (e.g. `ESC ( B`, charset selection) + a final byte.
                Some(c) if (' '..='/').contains(&c) => {
                    for c in chars.by_ref() {
                        if ('0'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                _ => {}
            },
            '\r' => out.push('\n'),
            c if c.is_control() && c != '\n' && c != '\t' => {}
            c => out.push(c),
        }
    }
    out
}

/// Classify from the time since the last output and the recent output.
pub fn classify(quiet_ms: u64, recent: &[u8]) -> Activity {
    if quiet_ms < WORKING_MS {
        return Activity::Working;
    }
    // Only the end matters: what is on screen now.
    let tail = &recent[recent.len().saturating_sub(3000)..];
    let text = strip_ansi(tail).to_lowercase();
    let screen: String = text
        .chars()
        .rev()
        .take(1200)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if WAITING.iter().any(|p| screen.contains(p)) {
        Activity::Waiting
    } else {
        Activity::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escape_sequences() {
        let raw = b"\x1b[1;32mgreen\x1b[0m \x1b]0;title\x07done\r\n\x1b(B\x1b[?25lok";
        assert_eq!(strip_ansi(raw), "green done\n\nok");
    }

    #[test]
    fn classifies() {
        assert_eq!(classify(100, b"anything"), Activity::Working);
        let claude = "\x1b[1mBash command\x1b[0m\r\n  rm -rf build\r\n\x1b[2mDo you want to proceed?\x1b[0m\r\n\x1b[36m❯ 1. Yes\x1b[0m\r\n  2. No";
        assert_eq!(classify(5000, claude.as_bytes()), Activity::Waiting);
        assert_eq!(classify(5000, b"Overwrite file? (y/N)"), Activity::Waiting);
        assert_eq!(
            classify(5000, b"\x1b[2m? for shortcuts\x1b[0m\r\n> "),
            Activity::Idle
        );
        // An old question far above the current screen does not count.
        let mut old = b"Do you want to proceed?\n".to_vec();
        old.extend(std::iter::repeat_n(b'x', 4000));
        assert_eq!(classify(5000, &old), Activity::Idle);
    }
}
