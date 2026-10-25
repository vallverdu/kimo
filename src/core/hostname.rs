//! Names of kimo hosts as they appear in agent ids (`gpu-lab/api-1a2b`).

/// A host name usable as an id prefix: lowercase letters, digits and dashes.
pub fn host_slug(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() {
        "kimo".into()
    } else {
        s[..s.len().min(32)].to_string()
    }
}

/// `host/rest` → `(host, rest)` for ids of agents on other kimos.
pub fn split_remote(id: &str) -> Option<(&str, &str)> {
    id.split_once('/')
        .filter(|(h, r)| !h.is_empty() && !r.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(host_slug("GPU Lab.local"), "gpu-lab-local");
        assert_eq!(host_slug("--"), "kimo");
        assert_eq!(
            split_remote("gpu-lab/api-1a2b"),
            Some(("gpu-lab", "api-1a2b"))
        );
        assert_eq!(split_remote("api-1a2b"), None);
        assert_eq!(split_remote("/x"), None);
    }
}
