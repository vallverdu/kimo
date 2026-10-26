//! The JavaScript SDK's own checks (tests/js/*.mjs), run with Node when it is installed.

use std::process::Command;

#[test]
fn sdk_scripts_pass() {
    let Ok(version) = Command::new("node").arg("--version").output() else {
        eprintln!("node is not installed: SDK checks skipped");
        return;
    };
    assert!(version.status.success());
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/js");
    let mut scripts: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "mjs"))
        .collect();
    scripts.sort();
    assert!(!scripts.is_empty());
    for script in scripts {
        let out = Command::new("node").arg(&script).output().unwrap();
        assert!(
            out.status.success(),
            "{}:\n{}{}",
            script.display(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
