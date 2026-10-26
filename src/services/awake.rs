//! Keeping the computer awake (Settings → keep awake): `caffeinate` on macOS,
//! `systemd-inhibit` on Linux, held by kimo while it is wanted and ended with kimo.

use crate::core::awake::{stay_awake, KeepAwake};
use serde::Serialize;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::Instant;

#[derive(Default)]
struct Holder {
    child: Option<Child>,
    last_working: Option<Instant>,
    mode: KeepAwake,
    /// The helper could not run (missing, refused): not tried again until the mode changes.
    failed: bool,
}

static HOLDER: Mutex<Option<Holder>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
pub struct Awake {
    pub mode: KeepAwake,
    /// Kept awake right now.
    pub active: bool,
    /// This machine has a way to stay awake.
    pub supported: bool,
}

fn spawn() -> Option<Child> {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("caffeinate");
        // Idle and system sleep, until kimo ends.
        c.args(["-i", "-s", "-w", &std::process::id().to_string()]);
        c
    } else {
        let mut c = Command::new("systemd-inhibit");
        c.args([
            "--what=idle:sleep",
            "--who=kimo",
            "--why=agents are working",
            "--mode=block",
            "sleep",
            "infinity",
        ]);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: prctl only asks the kernel to end the child with kimo.
            unsafe {
                c.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
                    Ok(())
                });
            }
        }
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    cmd.spawn()
        .map_err(|e| tracing::warn!("cannot keep the computer awake: {e}"))
        .ok()
}

fn supported() -> bool {
    let helper = if cfg!(target_os = "macos") {
        "caffeinate"
    } else {
        "systemd-inhibit"
    };
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(helper).is_file()))
        || std::path::Path::new("/usr/bin").join(helper).is_file()
}

/// Called every few seconds with the mode and whether an agent is working now.
pub fn tick(mode: KeepAwake, working: bool) {
    let mut guard = HOLDER.lock().unwrap_or_else(|e| e.into_inner());
    let h = guard.get_or_insert_with(Holder::default);
    if h.mode != mode {
        h.mode = mode;
        h.failed = false;
    }
    if working {
        h.last_working = Some(Instant::now());
    }
    // A helper that ended by itself (killed, refused) is gone.
    if let Some(child) = h.child.as_mut() {
        if !matches!(child.try_wait(), Ok(None)) {
            h.child = None;
            h.failed = true;
        }
    }
    let want = stay_awake(mode, h.last_working.map(|t| t.elapsed()));
    match (want, h.child.is_some()) {
        (true, false) if !h.failed => {
            h.child = spawn();
            h.failed = h.child.is_none();
            if h.child.is_some() {
                tracing::info!("keeping the computer awake ({mode:?})");
            }
        }
        (false, true) => {
            if let Some(mut child) = h.child.take() {
                let _ = child.kill();
                let _ = child.wait();
                tracing::info!("letting the computer sleep again");
            }
        }
        _ => {}
    }
}

pub fn status(mode: KeepAwake) -> Awake {
    let guard = HOLDER.lock().unwrap_or_else(|e| e.into_inner());
    Awake {
        mode,
        active: guard.as_ref().is_some_and(|h| h.child.is_some()),
        supported: supported(),
    }
}
