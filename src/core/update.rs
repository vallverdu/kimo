//! New versions: comparing release tags and naming the release build for this machine.

/// `v1.2.3` / `1.2.3` → (1, 2, 3); anything after a `-` or `+` is ignored.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let v = version.trim().trim_start_matches('v');
    let v = v.split(['-', '+']).next()?;
    let mut parts = v.split('.').map(|p| p.parse::<u64>().ok());
    Some((
        parts.next()??,
        parts.next().flatten().unwrap_or(0),
        parts.next().flatten().unwrap_or(0),
    ))
}

/// Is `latest` a newer version than `current`?
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

/// The release archive for a machine (`kimo-aarch64-apple-darwin`), as the release workflow
/// and install.sh name it; `None` where kimo publishes no build.
pub fn asset_name(os: &str, arch: &str, window: bool) -> Option<String> {
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };
    let (target, variant) = match os {
        "macos" => ("apple-darwin", "kimo"),
        "linux" => (
            "unknown-linux-gnu",
            if window { "kimo" } else { "kimo-server" },
        ),
        _ => return None,
    };
    Some(format!("{variant}-{arch}-{target}"))
}

/// The latest release from GitHub's `releases/latest` answer: tag, page and notes.
pub fn parse_release(json: &str) -> Option<(String, String, String)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some((
        v["tag_name"].as_str()?.to_string(),
        v["html_url"].as_str().unwrap_or_default().to_string(),
        v["body"].as_str().unwrap_or_default().to_string(),
    ))
}

/// The hash in a `.sha256` file (`<hash>  <file>`).
pub fn parse_sha256(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?.to_ascii_lowercase();
    (hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit())).then_some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("v0.2.0", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
        assert!(!is_newer("v0.1.0", "0.1.1"));
        assert!(!is_newer("nightly", "0.1.1"));
        assert!(!is_newer("v0.1.1-rc1", "0.1.1"));
    }

    #[test]
    fn assets_and_files() {
        assert_eq!(
            asset_name("macos", "aarch64", false).as_deref(),
            Some("kimo-aarch64-apple-darwin")
        );
        assert_eq!(
            asset_name("linux", "x86_64", false).as_deref(),
            Some("kimo-server-x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            asset_name("linux", "aarch64", true).as_deref(),
            Some("kimo-aarch64-unknown-linux-gnu")
        );
        assert_eq!(asset_name("windows", "x86_64", true), None);
        assert_eq!(asset_name("linux", "riscv64", true), None);
        let (tag, url, notes) = parse_release(
            r#"{"tag_name":"v0.1.2","html_url":"https://github.com/x/kimo/releases/tag/v0.1.2","body":"Fixes"}"#,
        )
        .unwrap();
        assert_eq!((tag.as_str(), notes.as_str()), ("v0.1.2", "Fixes"));
        assert!(url.ends_with("v0.1.2"));
        assert_eq!(parse_release(r#"{"message":"Not Found"}"#), None);
        let hash = "a".repeat(64);
        assert_eq!(parse_sha256(&format!("{hash}  kimo.tar.gz\n")), Some(hash));
        assert_eq!(parse_sha256("nope"), None);
    }
}
