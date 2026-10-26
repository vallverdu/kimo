//! Reading a repository's current branch from `.git/HEAD` (no git process needed), and naming
//! worktree branches.

/// Branch name, or the short commit for a detached HEAD.
pub fn parse_head(head: &str) -> Option<String> {
    let head = head.trim();
    match head.strip_prefix("ref: ") {
        Some(r) => Some(r.strip_prefix("refs/heads/").unwrap_or(r).to_string()),
        None if head.len() >= 7 && head.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Some(head[..7].to_string())
        }
        None => None,
    }
}

/// A branch (and folder) name from what the user typed: `Fix login bug` → `fix-login-bug`.
/// Slashes are kept (`feat/login`), anything git refuses is replaced or dropped.
pub fn branch_name(text: &str) -> String {
    let mut out = String::new();
    for part in text.trim().split('/') {
        let word: String = part
            .to_lowercase()
            .split(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'))
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        let word = word.trim_matches(['.', '-']).replace("..", ".");
        if word.is_empty() || word.ends_with(".lock") {
            continue;
        }
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names() {
        assert_eq!(branch_name("Fix login bug"), "fix-login-bug");
        assert_eq!(branch_name("feat/New API"), "feat/new-api");
        assert_eq!(branch_name(" ../..//x.. "), "x");
        assert_eq!(branch_name("a..b"), "a.b");
        assert_eq!(branch_name("///"), "");
    }

    #[test]
    fn branch_or_short_commit() {
        assert_eq!(
            parse_head("ref: refs/heads/main\n").as_deref(),
            Some("main")
        );
        assert_eq!(
            parse_head("ref: refs/heads/feat/x").as_deref(),
            Some("feat/x")
        );
        assert_eq!(parse_head("9d643a2f00aa").as_deref(), Some("9d643a2"));
        assert_eq!(parse_head("garbage"), None);
    }
}

/// One changed path in `git status`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Change {
    pub path: String,
    /// The old path of a rename or copy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Short letter, as editors show it: M, A, D, R, C, U (untracked), ! (conflict).
    pub letter: char,
    /// Something of it is staged (in the index).
    pub staged: bool,
    /// Something of it is not staged (in the working tree).
    pub unstaged: bool,
}

/// A repository's state from `git status --porcelain=v2 --branch -z`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Status {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
}

/// The letter for a status pair (index, working tree).
fn letter(x: char, y: char) -> char {
    // Added or renamed in the index stays so while edited further (`AM` is still new).
    let main = if matches!(x, 'A' | 'R' | 'C') || y == '.' {
        x
    } else {
        y
    };
    match main {
        'M' | 'T' => 'M',
        'A' => 'A',
        'D' => 'D',
        'R' => 'R',
        'C' => 'C',
        _ => 'M',
    }
}

/// Parse `git status --porcelain=v2 --branch -z` (NUL-separated records).
pub fn parse_status(out: &str) -> Status {
    let mut status = Status::default();
    let mut records = out.split('\0').filter(|r| !r.is_empty());
    while let Some(r) = records.next() {
        if let Some(h) = r.strip_prefix("# ") {
            if let Some(b) = h.strip_prefix("branch.head ") {
                status.branch = Some(b.to_string()).filter(|b| b != "(detached)");
            } else if let Some(u) = h.strip_prefix("branch.upstream ") {
                status.upstream = Some(u.to_string());
            } else if let Some(ab) = h.strip_prefix("branch.ab ") {
                let mut it = ab.split(' ');
                status.ahead = it
                    .next()
                    .and_then(|a| a.trim_start_matches('+').parse().ok())
                    .unwrap_or(0);
                status.behind = it
                    .next()
                    .and_then(|b| b.trim_start_matches('-').parse().ok())
                    .unwrap_or(0);
            }
            continue;
        }
        let fields: Vec<&str> = r.splitn(2, ' ').collect();
        match fields.first().copied() {
            Some("?") => status.changes.push(Change {
                path: fields.get(1).unwrap_or(&"").to_string(),
                from: None,
                letter: 'U',
                staged: false,
                unstaged: true,
            }),
            Some(kind @ ("1" | "2" | "u")) => {
                // 1: 8 fields then the path; 2: 9 then path (and the old path next); u: 10.
                let skip = match kind {
                    "1" => 7,
                    "2" => 8,
                    _ => 9,
                };
                let rest: Vec<&str> = r.splitn(skip + 2, ' ').collect();
                let xy: Vec<char> = rest.get(1).unwrap_or(&"..").chars().collect();
                let (x, y) = (
                    xy.first().copied().unwrap_or('.'),
                    xy.get(1).copied().unwrap_or('.'),
                );
                let path = rest.last().unwrap_or(&"").to_string();
                let from = (kind == "2").then(|| records.next().unwrap_or_default().to_string());
                status.changes.push(Change {
                    path,
                    from,
                    letter: if kind == "u" { '!' } else { letter(x, y) },
                    staged: x != '.' && kind != "u",
                    unstaged: y != '.' || kind == "u",
                });
            }
            _ => {}
        }
    }
    status
}

#[cfg(test)]
mod status_tests {
    #[test]
    fn porcelain_v2() {
        let out = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa aaa README.md\0\
1 AM N... 000000 100644 100644 000 bbb src/new file.rs\0\
2 R. N... 100644 100644 100644 ccc ccc R100 docs/new.md\0docs/old.md\0\
u UU N... 100644 100644 100644 100644 d e f both.txt\0\
? notes/todo.txt\0";
        let s = super::parse_status(out);
        assert_eq!(
            (
                s.branch.as_deref(),
                s.upstream.as_deref(),
                s.ahead,
                s.behind
            ),
            (Some("main"), Some("origin/main"), 2, 1)
        );
        let got: Vec<(String, char, bool, bool)> = s
            .changes
            .iter()
            .map(|c| (c.path.clone(), c.letter, c.staged, c.unstaged))
            .collect();
        assert_eq!(
            got,
            vec![
                ("README.md".into(), 'M', false, true),
                ("src/new file.rs".into(), 'A', true, true),
                ("docs/new.md".into(), 'R', true, false),
                ("both.txt".into(), '!', false, true),
                ("notes/todo.txt".into(), 'U', false, true),
            ]
        );
        assert_eq!(s.changes[2].from.as_deref(), Some("docs/old.md"));
        assert_eq!(
            super::parse_status("# branch.head (detached)\0").branch,
            None
        );
    }
}
