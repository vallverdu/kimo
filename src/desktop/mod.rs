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

pub fn run(port: u16, data_dir: &std::path::Path) -> Result<()> {
    if let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) {
        let _ = EXE.set(exe);
    }
    // macOS: kimo.app in Applications, at the newest kimo (in the background).
    std::thread::spawn(|| crate::services::macapp::ensure(&crate::config::user_home()));
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
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("kimo")
            .with_app_id("kimo")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([760.0, 480.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: include_bytes!("../../assets/icons/window-128.rgba").to_vec(),
                width: 128,
                height: 128,
            })),
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
