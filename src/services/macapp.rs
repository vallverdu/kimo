//! macOS: kimo as an app in Applications (Dock, Launchpad, Spotlight), kept at the newest
//! kimo on the machine, and the PATH of a login shell for a kimo opened from the Dock.

use crate::core::update::is_newer;
use crate::services::network::run_for;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const ICON: &[u8] = include_bytes!("../../assets/icons/kimo.icns");

/// The app's `Info.plist` for a kimo version.
pub fn info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>kimo</string>
	<key>CFBundleDisplayName</key><string>kimo</string>
	<key>CFBundleIdentifier</key><string>ai.kimo.app</string>
	<key>CFBundleExecutable</key><string>kimo</string>
	<key>CFBundleIconFile</key><string>kimo</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>{version}</string>
	<key>CFBundleVersion</key><string>{version}</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
"#
    )
}

/// Where the app goes: /Applications when it can be written, else ~/Applications.
pub fn app_path(user_home: &Path) -> PathBuf {
    let system = Path::new("/Applications");
    let existing = system.join("kimo.app");
    let writable = |dir: &Path| {
        let probe = dir.join(".kimo-write-test");
        let ok = std::fs::write(&probe, b"").is_ok();
        let _ = std::fs::remove_file(&probe);
        ok
    };
    if existing.exists() || writable(system) {
        existing
    } else {
        user_home.join("Applications").join("kimo.app")
    }
}

/// The version a kimo binary reports (`kimo 0.1.6`).
fn version_of(binary: &Path) -> Option<String> {
    let out = run_for(
        Command::new(binary).arg("--version"),
        Duration::from_secs(10),
    )
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().nth(1).map(String::from)
}

/// Copy a binary over `to` without ever leaving a half-written file there.
fn copy_binary(from: &Path, to: &Path) -> Result<()> {
    let tmp = to.with_extension("new");
    std::fs::copy(from, &tmp).with_context(|| format!("copying kimo to {}", tmp.display()))?;
    std::fs::rename(&tmp, to).with_context(|| format!("replacing {}", to.display()))?;
    Ok(())
}

/// Make (or refresh) kimo.app with `exe` inside. Returns the app's path.
pub fn install(exe: &Path, user_home: &Path) -> Result<PathBuf> {
    let app = app_path(user_home);
    let contents = app.join("Contents");
    let (bin_dir, res_dir) = (contents.join("MacOS"), contents.join("Resources"));
    std::fs::create_dir_all(&bin_dir).with_context(|| format!("creating {}", app.display()))?;
    std::fs::create_dir_all(&res_dir)?;
    let target = bin_dir.join("kimo");
    if exe.canonicalize().ok() != target.canonicalize().ok() {
        copy_binary(exe, &target)?;
    }
    let version = version_of(&target).unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    std::fs::write(contents.join("Info.plist"), info_plist(&version))?;
    std::fs::write(res_dir.join("kimo.icns"), ICON)?;
    // A sealed (ad hoc) signature, so macOS treats it as one app; best effort.
    let _ = run_for(
        Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app),
        Duration::from_secs(60),
    );
    // Tell the Dock and Finder about the new icon and version.
    let _ = run_for(
        Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
            .args(["-f"])
            .arg(&app),
        Duration::from_secs(20),
    );
    Ok(app)
}

/// At start on macOS: make sure kimo.app exists and every copy of kimo (the app, the command
/// in ~/.local/bin) is the newest one on the machine, so the Dock and the terminal agree.
pub fn ensure(user_home: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return;
    };
    // A kimo being built (cargo's target folder) is not the one to install.
    if exe.components().any(|c| c.as_os_str() == "target") {
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let app_bin = app_path(user_home).join("Contents/MacOS/kimo");
    let cli = user_home.join(".local/bin/kimo");
    let stale = |copy: &Path| -> bool {
        if copy.canonicalize().ok().as_deref() == Some(exe.as_path()) {
            return false;
        }
        match version_of(copy) {
            Some(v) => is_newer(current, &v),
            None => true,
        }
    };
    if !app_bin.exists() || stale(&app_bin) {
        match install(&exe, user_home) {
            Ok(app) => tracing::info!("kimo app ready at {}", app.display()),
            Err(e) => tracing::warn!("cannot set up kimo.app: {e:#}"),
        }
    }
    if cli.exists() && stale(&cli) {
        if let Err(e) = copy_binary(&exe, &cli) {
            tracing::warn!("cannot update {}: {e:#}", cli.display());
        }
    }
}

/// After an update replaced `exe`: the other copies (the app, the command) follow.
pub fn after_update(exe: &Path, user_home: &Path) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let app_bin = app_path(user_home).join("Contents/MacOS/kimo");
    if app_bin.exists() || exe.starts_with(app_path(user_home)) {
        if let Err(e) = install(exe, user_home) {
            tracing::warn!("cannot update kimo.app: {e:#}");
        }
    }
    let cli = user_home.join(".local/bin/kimo");
    if cli.exists() && cli.canonicalize().ok() != exe.canonicalize().ok() {
        if let Err(e) = copy_binary(exe, &cli) {
            tracing::warn!("cannot update {}: {e:#}", cli.display());
        }
    }
}

/// Opened from the Dock or Finder, an app gets the system's bare PATH, and agents
/// (`claude`, `codex`, `node`) would not be found: take the PATH a login shell sets.
pub fn login_path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let out = run_for(
        Command::new(&shell).args([
            "-l",
            "-i",
            "-c",
            "printf '\\n__KIMO_PATH__%s__KIMO_PATH__\\n' \"$PATH\"",
        ]),
        Duration::from_secs(8),
    )
    .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find("__KIMO_PATH__")? + "__KIMO_PATH__".len();
    let end = text[start..].find("__KIMO_PATH__")? + start;
    Some(text[start..end].to_string()).filter(|p| !p.is_empty())
}

/// Use the login shell's PATH when kimo was not started from a terminal.
pub fn fix_path() {
    let done = std::env::var_os("KIMO_LOGIN_PATH").is_some();
    if !cfg!(target_os = "macos") || std::env::var_os("TERM").is_some() || done {
        return;
    }
    if let Some(path) = login_path() {
        std::env::set_var("PATH", path);
    }
    // The server the window starts inherits it: no need to ask the shell again.
    std::env::set_var("KIMO_LOGIN_PATH", "1");
}

#[cfg(test)]
mod tests {
    #[test]
    fn plist_names_the_binary_and_version() {
        let p = super::info_plist("0.1.6");
        assert!(p.contains("<key>CFBundleExecutable</key><string>kimo</string>"));
        assert!(p.contains("<string>0.1.6</string>"));
        assert!(p.contains("ai.kimo.app"));
    }
}
