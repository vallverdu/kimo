//! A project's files as the API shows them: paths inside the project only, text or binary.

use std::path::{Component, Path, PathBuf};

/// A path relative to a project folder, from what a client sent (`src/main.rs`, `./a`, ``):
/// never absolute and never above the folder. `None` when it tries.
pub fn relative(path: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in Path::new(path.trim()).components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(out)
}

/// Does the start of a file look binary (a NUL byte, as git decides)?
pub fn looks_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|&b| b == 0)
}

/// Folders a file list leaves out (version control internals).
pub fn hidden(name: &str) -> bool {
    name == ".git" || name == ".DS_Store"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside() {
        assert_eq!(relative("src/main.rs"), Some(PathBuf::from("src/main.rs")));
        assert_eq!(relative("./a/./b"), Some(PathBuf::from("a/b")));
        assert_eq!(relative(""), Some(PathBuf::new()));
        for bad in ["../x", "a/../../x", "/etc/passwd", "a/.."] {
            assert_eq!(relative(bad), None, "{bad}");
        }
        assert!(looks_binary(b"PNG\0\x01") && !looks_binary("héllo\n".as_bytes()));
        assert!(hidden(".git") && !hidden(".github"));
    }
}
