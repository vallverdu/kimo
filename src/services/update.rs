//! New kimo versions: the latest GitHub release (checked at most every 6 hours, only when a
//! window or app asks) and installing it over this kimo's own binary.

use crate::core::update::{asset_name, is_newer, parse_release, parse_sha256};
use crate::services::network::run_for;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REPO: &str = "vallverdu/kimo";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    /// This kimo's version.
    pub current: String,
    /// The latest published version (`v0.1.2`), when known.
    pub latest: Option<String>,
    pub available: bool,
    /// The release page (notes).
    pub url: Option<String>,
    pub notes: String,
    /// Why kimo cannot install it by itself (then: the install script), if so.
    pub manual: Option<String>,
    /// Why the check failed, if it did.
    pub error: Option<String>,
}

static LAST: Mutex<Option<(Instant, Release)>> = Mutex::new(None);
static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Remember where this binary is, at startup: once replaced, the running file's path reads
/// as deleted on Linux.
pub fn remember_exe() {
    if let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) {
        let _ = EXE.set(exe);
    }
}

pub fn exe() -> Result<PathBuf> {
    EXE.get()
        .cloned()
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| anyhow!("cannot tell where kimo is installed"))
}

/// The release build for this machine.
fn asset() -> Option<String> {
    asset_name(
        std::env::consts::OS,
        std::env::consts::ARCH,
        cfg!(feature = "desktop"),
    )
}

/// Why kimo cannot replace its own binary, if it cannot.
fn manual_reason() -> Option<String> {
    if asset().is_none() {
        return Some("no release build for this system: build it from source".into());
    }
    let exe = match exe() {
        Ok(e) => e,
        Err(e) => return Some(e.to_string()),
    };
    let dir = exe.parent()?;
    if exe.components().any(|c| c.as_os_str() == "target") {
        return Some("this kimo was built from source: pull and build again".into());
    }
    let probe = dir.join(format!(".kimo-write-test-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(probe);
            None
        }
        Err(_) => Some(format!(
            "{} is not writable by you: update with the install script",
            dir.display()
        )),
    }
}

fn curl(args: &[&str], limit: Duration) -> Result<Vec<u8>> {
    let out = run_for(
        Command::new("curl")
            .args(["-fsSL", "--max-time", &limit.as_secs().to_string()])
            .args(args),
        limit + Duration::from_secs(5),
    )
    .context("running curl")?;
    if !out.status.success() {
        bail!(
            "download failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// The latest release (cached for 6 hours; `force` asks GitHub again).
pub fn check(force: bool) -> Release {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, release)) = last.as_ref() {
        if !force && at.elapsed() < CHECK_EVERY {
            return release.clone();
        }
    }
    let current = env!("CARGO_PKG_VERSION").to_string();
    let api = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let found = curl(
        &[
            "-H",
            "Accept: application/vnd.github+json",
            "-A",
            "kimo",
            &api,
        ],
        Duration::from_secs(15),
    )
    .and_then(|body| {
        parse_release(&String::from_utf8_lossy(&body)).ok_or_else(|| anyhow!("no release found"))
    });
    let release = match found {
        Ok((tag, url, notes)) => Release {
            available: is_newer(&tag, &current),
            manual: manual_reason(),
            current,
            latest: Some(tag),
            url: Some(url).filter(|u| !u.is_empty()),
            notes,
            error: None,
        },
        Err(e) => Release {
            current,
            latest: None,
            available: false,
            url: None,
            notes: String::new(),
            manual: None,
            error: Some(format!("{e:#}")),
        },
    };
    *last = Some((Instant::now(), release.clone()));
    release
}

fn sha256(file: &Path) -> Result<String> {
    let path = file.display().to_string();
    let out = run_for(
        Command::new("sha256sum").arg(&path),
        Duration::from_secs(30),
    )
    .or_else(|_| {
        run_for(
            Command::new("shasum").args(["-a", "256", &path]),
            Duration::from_secs(30),
        )
    })?;
    parse_sha256(&String::from_utf8_lossy(&out.stdout))
        .ok_or_else(|| anyhow!("cannot compute the checksum"))
}

/// Download the latest release, check it and put it in place of this kimo's binary. The
/// running server and window keep the old one until they restart. Returns the new version.
pub fn install(data_dir: &Path) -> Result<String> {
    let release = check(true);
    let tag = release
        .latest
        .clone()
        .ok_or_else(|| anyhow!(release.error.clone().unwrap_or_default()))?;
    if !release.available {
        bail!("kimo {} is the latest version", release.current);
    }
    if let Some(reason) = release.manual {
        bail!("{reason}");
    }
    let name = asset().ok_or_else(|| anyhow!("no release build for this system"))?;
    let exe = exe()?;
    let work = data_dir.join("update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let archive = work.join("kimo.tar.gz");
    let base = format!("https://github.com/{REPO}/releases/download/{tag}/{name}.tar.gz");
    curl(
        &["-o", &archive.display().to_string(), &base],
        Duration::from_secs(600),
    )?;
    let expected = parse_sha256(&String::from_utf8_lossy(&curl(
        &[&format!("{base}.sha256")],
        Duration::from_secs(30),
    )?))
    .ok_or_else(|| anyhow!("the release has no checksum"))?;
    if sha256(&archive)? != expected {
        bail!("the download is damaged (checksum mismatch): try again");
    }
    let out = run_for(
        Command::new("tar")
            .args(["-xzf", "kimo.tar.gz"])
            .current_dir(&work),
        Duration::from_secs(60),
    )?;
    if !out.status.success() {
        bail!("cannot unpack the release");
    }
    let new = work.join(&name).join("kimo");
    let version = run_for(Command::new(&new).arg("--version"), Duration::from_secs(10))
        .ok()
        .filter(|o| o.status.success())
        .ok_or_else(|| anyhow!("the new kimo does not run on this machine"))?;
    let version = String::from_utf8_lossy(&version.stdout)
        .trim()
        .trim_start_matches("kimo ")
        .to_string();
    // Copy next to the binary, then rename over it: running processes keep the old file.
    let staged = exe.with_file_name(".kimo-update");
    std::fs::copy(&new, &staged).with_context(|| format!("writing {}", staged.display()))?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&staged, &exe).with_context(|| format!("replacing {}", exe.display()))?;
    let _ = std::fs::remove_dir_all(&work);
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = None;
    tracing::info!("installed kimo {version} at {}", exe.display());
    // macOS: the app in Applications and the command in ~/.local/bin get it too.
    crate::services::macapp::after_update(&exe, &crate::config::user_home());
    Ok(version)
}
