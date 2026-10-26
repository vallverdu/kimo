//! Files sent to an agent from a phone or app (photos, screenshots): their names on disk.

/// Most a single upload may weigh (decoded).
pub const MAX_UPLOAD_BYTES: usize = 20 << 20;

/// A safe file name for an upload: `<stamp>-<name>` with only `[A-Za-z0-9._-]`, never hidden,
/// at most 80 characters, keeping the extension (agents tell images by it).
pub fn file_name(original: &str, stamp: &str) -> String {
    let base = original.rsplit(['/', '\\']).next().unwrap_or("");
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let clean = clean.trim_start_matches(['.', '-']);
    let clean = if clean.is_empty() { "file" } else { clean };
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && e.len() <= 8 => (s, format!(".{e}")),
        _ => (clean, String::new()),
    };
    let room = 80usize.saturating_sub(stamp.len() + 1 + ext.len());
    let stem: String = stem.chars().take(room.max(1)).collect();
    format!("{stamp}-{stem}{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_names() {
        assert_eq!(
            file_name("IMG_0042.HEIC", "20261026-0710"),
            "20261026-0710-IMG_0042.HEIC"
        );
        assert_eq!(file_name("../../etc/passwd", "s"), "s-passwd");
        assert_eq!(file_name(".hidden.png", "s"), "s-hidden.png");
        assert_eq!(file_name("my photo (1).jpg", "s"), "s-my-photo--1-.jpg");
        assert_eq!(file_name("", "s"), "s-file");
        let long = format!("{}.png", "a".repeat(200));
        let name = file_name(&long, "s");
        assert!(name.len() <= 80 && name.ends_with(".png"), "{name}");
    }
}
