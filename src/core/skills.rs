//! Reading a skill's `SKILL.md` front matter (name, description).

/// `(name, description)` from the YAML-ish front matter between `---` lines.
pub fn parse_front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        // Only top-level keys (nested metadata is indented).
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value
            .trim()
            .trim_matches(|c| c == '"' || c == '\'')
            .to_string();
        match key.trim() {
            "name" if !value.is_empty() => name = Some(value),
            "description" if !value.is_empty() => description = Some(value),
            _ => {}
        }
    }
    (name, description)
}

/// A skill name usable as a folder name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `owner/repo[/sub/dir]` GitHub shorthand → (clone URL, subdirectory).
pub fn github_shorthand(source: &str) -> Option<(String, String)> {
    let s = source
        .trim()
        .trim_start_matches("https://github.com/")
        .trim_end_matches('/');
    if s.contains("://")
        || s.starts_with('/')
        || s.starts_with('.')
        || s.starts_with('~')
        || s.contains('@')
    {
        return None;
    }
    let mut parts = s.splitn(3, '/');
    let (owner, repo) = (parts.next()?, parts.next()?);
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) || !ok(repo) {
        return None;
    }
    let sub = parts
        .next()
        .unwrap_or("")
        .trim_start_matches("tree/")
        .to_string();
    // `tree/<branch>/path` from a pasted GitHub URL: drop the branch.
    let sub = if source.contains("/tree/") {
        sub.split_once('/')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    } else {
        sub
    };
    Some((
        format!(
            "https://github.com/{owner}/{}.git",
            repo.trim_end_matches(".git")
        ),
        sub,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn front_matter() {
        let text = "---\nname: skill-creator\ndescription: \"Create a skill\"\nmetadata:\n  name: nested\n---\n# Body";
        assert_eq!(
            parse_front_matter(text),
            (Some("skill-creator".into()), Some("Create a skill".into()))
        );
        assert_eq!(parse_front_matter("# no front matter"), (None, None));
    }

    #[test]
    fn names_and_sources() {
        assert!(valid_name("pdf-tools_2"));
        assert!(!valid_name("../x") && !valid_name(".hidden") && !valid_name(""));
        assert_eq!(
            github_shorthand("anthropics/skills/document-skills/pdf"),
            Some((
                "https://github.com/anthropics/skills.git".into(),
                "document-skills/pdf".into()
            ))
        );
        assert_eq!(
            github_shorthand("https://github.com/stablyai/orca/tree/main/skills/orca-cli"),
            Some((
                "https://github.com/stablyai/orca.git".into(),
                "skills/orca-cli".into()
            ))
        );
        assert_eq!(github_shorthand("/home/me/skills"), None);
        assert_eq!(github_shorthand("git@github.com:me/x.git"), None);
    }
}
