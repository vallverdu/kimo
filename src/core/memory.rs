//! Memory: how tight the machine is, which agents to stop to free memory, and what to call
//! the processes inside an agent (Claude Code itself, its MCP servers, tools it runs).

use serde::Serialize;

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

/// How much memory the machine has left.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Pressure {
    Ok,
    /// Under 15 % (or 1.5 GiB) available: warn, and suggest stopping idle agents.
    Low,
    /// Under 7 % (or 600 MiB) available: the machine starts swapping; free memory now.
    Critical,
}

/// Available memory below which the machine counts as low.
pub fn low_mark(total: u64) -> u64 {
    (total * 15 / 100).min(4 * GIB).max(GIB.min(total / 4))
}

/// Available memory below which the machine counts as critical.
pub fn critical_mark(total: u64) -> u64 {
    (total * 7 / 100)
        .min(2 * GIB)
        .max((600 * MIB).min(total / 8))
}

pub fn pressure(total: u64, available: u64) -> Pressure {
    if total == 0 {
        Pressure::Ok
    } else if available < critical_mark(total) {
        Pressure::Critical
    } else if available < low_mark(total) {
        Pressure::Low
    } else {
        Pressure::Ok
    }
}

/// A running agent that may be stopped to free memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub id: String,
    pub rss: u64,
    /// Milliseconds since its last output.
    pub quiet_ms: u64,
    /// Waiting at its prompt (not working, not asking you something).
    pub idle: bool,
    /// Its conversation continues when it starts again (Claude, Codex; not shells).
    pub resumable: bool,
}

/// Agents quiet for less than this are never stopped to free memory (they may be about to
/// answer, or you just read their answer).
pub const MIN_QUIET_MS: u64 = 60_000;

/// Which agents to stop: idle, resumable ones, biggest first, until `need` bytes are freed.
pub fn to_free(candidates: &[Candidate], need: u64) -> Vec<String> {
    let mut idle: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| c.idle && c.resumable && c.quiet_ms >= MIN_QUIET_MS)
        .collect();
    idle.sort_by_key(|c| (std::cmp::Reverse(c.rss), std::cmp::Reverse(c.quiet_ms)));
    let mut freed = 0;
    let mut out = vec![];
    for c in idle {
        if freed >= need {
            break;
        }
        freed += c.rss;
        out.push(c.id.clone());
    }
    out
}

/// Agents idle for longer than `minutes` (0 = never stop idle agents on a timer).
pub fn idle_too_long(candidates: &[Candidate], minutes: u32) -> Vec<String> {
    if minutes == 0 {
        return vec![];
    }
    let limit = u64::from(minutes) * 60_000;
    candidates
        .iter()
        .filter(|c| c.idle && c.resumable && c.quiet_ms >= limit.max(MIN_QUIET_MS))
        .map(|c| c.id.clone())
        .collect()
}

/// A short name for a process in an agent's tree, from its command line: the MCP server it
/// is ("kicad-mcp-server", "freecad-mcp"), else the program ("claude", "node", "cargo").
pub fn part_label(argv: &[String]) -> String {
    let words = argv.iter().flat_map(|a| a.split(['/', '\\', ' ']));
    if let Some(mcp) = words
        .filter(|w| w.to_ascii_lowercase().contains("mcp"))
        .find(|w| !w.starts_with('-'))
    {
        return mcp
            .trim_end_matches(".js")
            .trim_end_matches(".py")
            .to_string();
    }
    let program = argv
        .first()
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .unwrap_or_default();
    // Interpreters: name the script they run instead.
    let interpreters = [
        "node", "python", "python3", "bun", "deno", "uv", "uvx", "npx", "bash", "sh",
    ];
    if interpreters
        .iter()
        .any(|i| program == *i || program.starts_with("python"))
    {
        // Inline code (`python -c`, `node -e`, `sh -c`) names nothing useful.
        if argv.iter().skip(1).any(|a| a == "-c" || a == "-e") {
            return program;
        }
        if let Some(script) = argv.iter().skip(1).find(|a| !a.starts_with('-')) {
            let name = script.rsplit('/').next().unwrap_or(script);
            if !name.is_empty() {
                return format!("{program} {}", name.chars().take(40).collect::<String>());
            }
        }
    }
    if program.is_empty() {
        "process".into()
    } else {
        program
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_levels_scale_with_the_machine() {
        let g = |n: u64| n * GIB;
        assert_eq!(pressure(g(16), g(8)), Pressure::Ok);
        // 16 GiB: low under 2.4 GiB, critical under 1.12 GiB.
        assert_eq!(pressure(g(16), g(2)), Pressure::Low);
        assert_eq!(pressure(g(16), g(1)), Pressure::Critical);
        // 8 GiB: low under 1.2 GiB, critical under 600 MiB.
        assert_eq!(pressure(g(8), 1100 * MIB), Pressure::Low);
        assert_eq!(pressure(g(8), 500 * MIB), Pressure::Critical);
        // 128 GiB: the marks stop growing (4 GiB, 2 GiB).
        assert_eq!(pressure(g(128), g(5)), Pressure::Ok);
        assert_eq!(pressure(g(128), g(3)), Pressure::Low);
        assert_eq!(pressure(0, 0), Pressure::Ok);
    }

    fn c(id: &str, mb: u64, quiet_s: u64, idle: bool, resumable: bool) -> Candidate {
        Candidate {
            id: id.into(),
            rss: mb * MIB,
            quiet_ms: quiet_s * 1000,
            idle,
            resumable,
        }
    }

    #[test]
    fn frees_the_biggest_idle_resumable_agents_first() {
        let all = [
            c("busy", 2000, 0, false, true),
            c("asking", 1500, 600, false, true),
            c("shell", 1800, 600, true, false),
            c("small", 300, 900, true, true),
            c("big", 1200, 300, true, true),
            c("fresh", 1300, 10, true, true),
        ];
        assert_eq!(to_free(&all, 1000 * MIB), vec!["big"]);
        assert_eq!(to_free(&all, 1400 * MIB), vec!["big", "small"]);
        assert_eq!(to_free(&all, 10 * GIB), vec!["big", "small"]);
        assert!(to_free(&all, 0).is_empty());
    }

    #[test]
    fn idle_timer() {
        let all = [
            c("a", 100, 40 * 60, true, true),
            c("b", 100, 5 * 60, true, true),
        ];
        assert_eq!(idle_too_long(&all, 30), vec!["a"]);
        assert!(idle_too_long(&all, 0).is_empty());
    }

    #[test]
    fn names_agent_processes() {
        let v = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        assert_eq!(
            part_label(&v(
                "/home/u/.local/bin/node /home/u/kicad-mcp-server/dist/index.js"
            )),
            "kicad-mcp-server"
        );
        assert_eq!(
            part_label(&v("/home/u/.local/bin/uv tool uvx freecad-mcp")),
            "freecad-mcp"
        );
        assert_eq!(
            part_label(&v("claude --dangerously-skip-permissions")),
            "claude"
        );
        assert_eq!(
            part_label(&v("/usr/bin/python3 -u train.py")),
            "python3 train.py"
        );
        assert_eq!(part_label(&v("/usr/bin/cargo test")), "cargo");
        assert_eq!(part_label(&v("python3 -c import time")), "python3");
        assert_eq!(
            part_label(&v("python3 -m http.server")),
            "python3 http.server"
        );
        assert_eq!(part_label(&[]), "process");
    }
}
