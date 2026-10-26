//! System notifications (Notification Center on macOS, the desktop's notifications on Linux).
//! Best effort: nothing happens where there is no notification service.

use crate::services::network::run_for;
use std::process::Command;
use std::time::Duration;

pub fn system(title: &str, body: &str) {
    let mut cmd = if cfg!(target_os = "macos") {
        // Title and text as arguments: no quoting of them inside the script.
        let mut c = Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            title,
            body,
        ]);
        c
    } else {
        let mut c = Command::new("notify-send");
        c.args(["--app-name=kimo", title, body]);
        c
    };
    match run_for(&mut cmd, Duration::from_secs(10)) {
        Ok(out) if out.status.success() => {}
        Ok(out) => tracing::info!(
            "no system notification: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(e) => tracing::info!("no system notification: {e:#}"),
    }
}
