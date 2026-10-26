//! New kimo versions: the latest GitHub release (checked every hour in the background, with a
//! system notification once per version), installing it over this kimo's own binary, and
//! updating by itself when the settings say so and no agent is working.

use crate::core::activity::Activity;
use crate::core::update::{asset_name, offer, parse_release, parse_sha256};
use crate::services::network::run_for;
use crate::services::sessions::Manager;
use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const REPO: &str = "vallverdu/kimo";
const CHECK_EVERY: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Serialize)]
pub struct Release {
    /// This kimo's version.
    pub current: String,
    /// The latest published version (`v0.1.2`), when known.
    pub latest: Option<String>,
    pub available: bool,
    /// The version of kimo's binary on disk, when it differs from the running one (after an
    /// update or the install script, until kimo restarts).
    pub installed: Option<String>,
    /// A newer kimo is installed: restarting runs it.
    pub restart: bool,
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

/// The version a kimo binary reports (`kimo 0.1.6`).
pub fn version_of(binary: &Path) -> Option<String> {
    let out = run_for(
        Command::new(binary).arg("--version"),
        Duration::from_secs(10),
    )
    .ok()
    .filter(|o| o.status.success())?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split_whitespace().nth(1).map(String::from)
}

/// The version of this kimo's binary on disk (asked again only when the file changes).
fn installed() -> Option<String> {
    static SEEN: Mutex<Option<(std::time::SystemTime, Option<String>)>> = Mutex::new(None);
    let exe = exe().ok()?;
    let modified = std::fs::metadata(&exe).and_then(|m| m.modified()).ok()?;
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, version)) = seen.as_ref() {
        if *at == modified {
            return version.clone();
        }
    }
    let version = version_of(&exe);
    *seen = Some((modified, version.clone()));
    version
}

/// The latest release (asked every hour; `force` asks GitHub again now).
pub fn check(force: bool) -> Release {
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let fresh = last
        .as_ref()
        .filter(|(at, _)| !force && at.elapsed() < CHECK_EVERY)
        .map(|(_, r)| r.clone());
    let mut release = match fresh {
        Some(r) => r,
        None => {
            let r = fetch();
            *last = Some((Instant::now(), r.clone()));
            r
        }
    };
    drop(last);
    let installed = installed();
    let (available, restart) = offer(
        release.latest.as_deref(),
        &release.current,
        installed.as_deref(),
    );
    release.available = available;
    release.restart = restart;
    release.installed = installed.filter(|_| restart);
    release
}

fn fetch() -> Release {
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
    match found {
        Ok((tag, url, notes)) => Release {
            manual: manual_reason(),
            current,
            latest: Some(tag),
            available: false,
            installed: None,
            restart: false,
            url: Some(url).filter(|u| !u.is_empty()),
            notes,
            error: None,
        },
        Err(e) => Release {
            current,
            latest: None,
            available: false,
            installed: None,
            restart: false,
            url: None,
            notes: String::new(),
            manual: None,
            error: Some(format!("{e:#}")),
        },
    }
}

/// In the background, for as long as the server runs: tell the person about new versions
/// (once each) and, when the settings say so, update and restart while no agent is working.
pub fn watch(manager: std::sync::Arc<Manager>) {
    let seen_file = manager.cfg.data_dir.join("update-notified");
    std::thread::sleep(Duration::from_secs(30));
    loop {
        let release = check(false);
        let settings = manager.settings().updates;
        if let Some(latest) = release.latest.as_deref().filter(|_| release.available) {
            let told = std::fs::read_to_string(&seen_file).unwrap_or_default();
            if settings.notify && !settings.auto && told.trim() != latest {
                crate::services::notify::system(
                    &format!("kimo {latest} is ready"),
                    "Open kimo and click Update: running agents continue their conversations.",
                );
                let _ = std::fs::write(&seen_file, latest);
            }
        }
        let wanted = release.restart || (release.available && release.manual.is_none());
        let working = manager
            .sessions()
            .iter()
            .any(|s| s.activity == Some(Activity::Working));
        if settings.auto && wanted && !working {
            match update_by_itself(&manager, &release) {
                Ok(version) => {
                    crate::services::notify::system(
                        &format!("kimo updated to {version}"),
                        "Running agents restarted and continue their conversations.",
                    );
                    return;
                }
                Err(e) => tracing::warn!("cannot update kimo by itself: {e:#}"),
            }
        }
        // Often enough to catch a quiet moment; GitHub is asked at most every hour.
        std::thread::sleep(Duration::from_secs(if settings.auto { 120 } else { 600 }));
    }
}

fn update_by_itself(manager: &Manager, release: &Release) -> Result<String> {
    let version = if release.available {
        install(&manager.cfg.data_dir)?
    } else {
        release.installed.clone().unwrap_or_default()
    };
    tracing::info!("updating by itself to kimo {version}: restarting");
    manager.request_restart()?;
    Ok(version)
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
        if release.restart {
            bail!(
                "kimo {} is installed already: restart kimo to use it",
                release.installed.unwrap_or_default()
            );
        }
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
    let version =
        version_of(&new).ok_or_else(|| anyhow!("the new kimo does not run on this machine"))?;
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
