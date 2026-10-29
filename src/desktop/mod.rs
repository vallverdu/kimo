//! The native kimo window. It is a client of the kimo server on this machine (started in the
//! background when needed), so closing the window never stops your agents.

mod app;
mod chart;
mod input;
mod terminal;
mod theme;

use crate::client::Client;
use anyhow::{anyhow, Context, Result};
use eframe::egui;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// This binary, found at start: after an update replaces the file, the window restarts on it.
pub(crate) static EXE: OnceLock<PathBuf> = OnceLock::new();

/// Start `kimo serve` detached (its own session), logging to `~/.kimo/kimo.log`.
pub(crate) fn start_server(port: u16, data_dir: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(data_dir.join("kimo.log"))?;
    let exe = match EXE.get() {
        Some(exe) => exe.clone(),
        None => std::env::current_exe()?,
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["serve", "--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // SAFETY: setsid only detaches the child from our session; called between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().context("starting the kimo server")?;
    Ok(())
}

/// The window's icon. macOS shows it in the Dock while kimo runs (in place of the app's), so
/// it is the app icon's rounded square there; Linux desktops draw their own frame.
pub(crate) fn window_icon(dark: bool) -> egui::IconData {
    if cfg!(target_os = "macos") {
        egui::IconData {
            rgba: if dark {
                include_bytes!("../../assets/icons/mac-256-dark.rgba").to_vec()
            } else {
                include_bytes!("../../assets/icons/mac-256.rgba").to_vec()
            },
            width: 256,
            height: 256,
        }
    } else {
        egui::IconData {
            rgba: include_bytes!("../../assets/icons/window-128.rgba").to_vec(),
            width: 128,
            height: 128,
        }
    }
}

/// Show the light or dark app icon in the Dock now (macOS; a window icon does not reach the
/// Dock there). Called on the UI thread.
#[cfg(target_os = "macos")]
pub(crate) fn set_dock_icon(dark: bool) {
    use objc2::{AnyThread, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSImage};
    use objc2_foundation::NSData;
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let data = NSData::with_bytes(crate::services::macapp::icns(dark));
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        // SAFETY: on the main thread (the marker proves it), with an image AppKit decoded.
        unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn set_dock_icon(_dark: bool) {}

pub fn run(port: u16, data_dir: &std::path::Path) -> Result<()> {
    if let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) {
        let _ = EXE.set(exe);
    }
    // macOS: kimo.app in Applications, at the newest kimo (in the background).
    // (Not for a capture: a development build must never replace the installed app.)
    if std::env::var_os("KIMO_CAPTURE").is_none() {
        std::thread::spawn(|| crate::services::macapp::ensure(&crate::config::user_home()));
    }
    let client = Client::local(port);
    if client.call("GET", "/status", None).is_err() {
        start_server(port, data_dir)?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while client.call("GET", "/status", None).is_err() {
            if Instant::now() > deadline {
                // The end of the server's log says why.
                let log = data_dir.join("kimo.log");
                let text = std::fs::read_to_string(&log).unwrap_or_default();
                let lines: Vec<&str> = text.lines().collect();
                let tail = lines[lines.len().saturating_sub(8)..].join("\n");
                return Err(anyhow!(
                    "the kimo server did not start (log: {}){}",
                    log.display(),
                    if tail.is_empty() {
                        String::new()
                    } else {
                        format!(":\n{tail}")
                    }
                ));
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
    // The icon style chosen in Settings → Appearance (kept with the window's preferences).
    let dark_icon = std::fs::read_to_string(data_dir.join("window.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v["icon"] == "dark");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("kimo")
            .with_app_id("kimo")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(window_icon(dark_icon))),
        ..Default::default()
    };
    let prefs = data_dir.join("window.json");
    eframe::run_native(
        "kimo",
        options,
        Box::new(move |cc| {
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app::KimoApp::new(
                cc.egui_ctx.clone(),
                client,
                prefs,
            )))
        }),
    )
    .map_err(|e| anyhow!("cannot open the window: {e}"))
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn both_app_icons_decode_for_the_dock() {
        use objc2::AnyThread;
        use objc2_app_kit::NSImage;
        use objc2_foundation::NSData;
        for dark in [false, true] {
            let data = NSData::with_bytes(crate::services::macapp::icns(dark));
            let image = NSImage::initWithData(NSImage::alloc(), &data);
            assert!(image.is_some(), "dark: {dark}");
        }
    }
}
