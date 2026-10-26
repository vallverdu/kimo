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
