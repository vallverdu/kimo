//! Entry point: config → manager → HTTP server (or `kimo pair`). Nothing else.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use kimo::config::{user_home, Command, Config};
use kimo::core::network::parse_bind;
use kimo::services::sessions::{Manager, ManagerConfig};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kimo=info".into()),
        )
        .init();
    let cfg = Config::parse();
    // macOS always has a screen session for GUI apps; Linux needs X11 or Wayland.
    let has_display = cfg!(target_os = "macos")
        || std::env::var_os("DISPLAY").is_some()
        || std::env::var_os("WAYLAND_DISPLAY").is_some();
    let command = cfg
        .command
        .clone()
        .unwrap_or(if cfg!(feature = "desktop") && has_display {
            Command::App
        } else {
            Command::Serve
        });
    match command {
        Command::App => app(&cfg),
        Command::Serve => runtime()?.block_on(serve(cfg)),
        Command::Pair => runtime()?.block_on(pair(cfg.port)),
        Command::Agents { json } => kimo::cli::agents(&kimo::cli::client(cfg.port), json),
        Command::Send { to, message } => {
            kimo::cli::send(&kimo::cli::client(cfg.port), &to, &message.join(" "))
        }
        Command::Read {
            agent,
            screen,
            lines,
        } => kimo::cli::read(&kimo::cli::client(cfg.port), &agent, screen, lines),
        Command::Wait { agent, timeout } => kimo::cli::wait(
            &kimo::cli::client(cfg.port),
            &agent,
            std::time::Duration::from_secs(timeout),
        ),
        Command::New {
            kind,
            name,
            cwd,
            account,
            prompt,
        } => kimo::cli::new(
            &kimo::cli::client(cfg.port),
            &kind,
            name,
            cwd,
            account,
            prompt,
        ),
        Command::Whoami => kimo::cli::whoami(&kimo::cli::client(cfg.port)),
        Command::Peers => kimo::cli::peers(&kimo::cli::client(cfg.port)),
        Command::Usage { days, json } => kimo::cli::usage(&kimo::cli::client(cfg.port), days, json),
        Command::History { folder } => kimo::cli::history(&kimo::cli::client(cfg.port), folder),
        Command::Continue { id, folder } => {
            kimo::cli::continue_conversation(&kimo::cli::client(cfg.port), &id, folder)
        }
        Command::Peer { action } => match action {
            kimo::config::PeerAction::Add { url, code, my_url } => {
                kimo::cli::peer_add(&kimo::cli::client(cfg.port), &url, &code, my_url)
            }
            kimo::config::PeerAction::Remove { id } => {
                kimo::cli::peer_remove(&kimo::cli::client(cfg.port), &id)
            }
        },
    }
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?)
}

#[cfg(feature = "desktop")]
fn app(cfg: &Config) -> Result<()> {
    kimo::desktop::run(cfg.port, &cfg.data_dir())
}

#[cfg(not(feature = "desktop"))]
fn app(_: &Config) -> Result<()> {
    anyhow::bail!("this kimo was built without the window: run `kimo serve`, or build with --features desktop")
}

async fn serve(cfg: Config) -> Result<()> {
    kimo::services::update::remember_exe();
    let target = parse_bind(&cfg.bind).map_err(anyhow::Error::msg)?;
    let ip = kimo::services::network::resolve(&target)?;
    let manager = Manager::new(ManagerConfig {
        data_dir: cfg.data_dir(),
        user_home: user_home(),
        shell: Config::shell(),
        scrollback_bytes: cfg.scrollback_bytes,
        projects_dir: cfg.projects_dir(),
        port: cfg.port,
        host_name: cfg.host_name(),
    })?;
    let sampler = manager.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let m = sampler.clone();
            let memory = tokio::task::spawn_blocking(move || {
                m.sample();
                kimo::services::stats::system_memory()
            })
            .await;
            // Stop idle agents when memory runs out (or after the idle time in the settings).
            if let Ok(memory) = memory {
                sampler.guard_memory(memory).await;
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, cfg.port))
        .await
        .with_context(|| {
            format!(
                "cannot listen on port {}: is another program using it? (--port to pick another)",
                cfg.port
            )
        })?;
    println!("kimo is running on port {}", cfg.port);
    // The addresses come from helper commands (ip, ifconfig, tailscale): list them without
    // holding up the server, which answers from now on.
    let port = cfg.port;
    tokio::task::spawn_blocking(move || {
        for a in kimo::services::network::info(ip, port).addresses {
            println!("  {:<36} {}", a.label, a.url);
        }
        println!("  pair a phone: Devices tab on this machine, or `kimo pair`");
    });
    let app = kimo::api::router(
        manager.clone(),
        kimo::api::Options {
            token: cfg.token.clone().filter(|t| !t.is_empty()),
            admin_origins: cfg.origins.clone(),
            local_owner: cfg.local_owner,
            bound: ip,
        },
    );
    let resumed = manager.resume_after_restart();
    if resumed > 0 {
        println!("  continued {resumed} sessions that were running before the restart");
    }
    // Open terminal websockets never close by themselves, so stop on a signal instead of draining.
    let mut restart = false;
    tokio::select! {
        result = axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()) => result?,
        _ = shutdown_signal() => tracing::info!("stopping sessions"),
        _ = manager.restart_requested() => {
            tracing::info!("restarting: stopping sessions");
            restart = true;
        }
    }
    manager.shutdown().await;
    if restart {
        // The same process (pid, service manager, log) runs the newly installed binary.
        let exe = kimo::services::update::exe()?;
        let err = std::os::unix::process::CommandExt::exec(
            std::process::Command::new(&exe).args(std::env::args_os().skip(1)),
        );
        return Err(anyhow!("cannot restart {}: {err}", exe.display()));
    }
    Ok(())
}

/// Minimal HTTP call to the kimo on this machine (it treats local requests as the owner).
async fn local_call(port: u16, method: &str, path: &str) -> Result<serde_json::Value> {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .with_context(|| format!("kimo is not running on port {port}"))?;
    let request = format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await?;
    let text = String::from_utf8_lossy(&response);
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("bad answer from kimo"))?;
    let value: serde_json::Value = serde_json::from_str(body).unwrap_or_default();
    if !head.starts_with("HTTP/1.1 200") {
        return Err(anyhow!(
            "kimo said: {}",
            value["error"]
                .as_str()
                .unwrap_or(head.lines().next().unwrap_or(""))
        ));
    }
    Ok(value)
}

async fn pair(port: u16) -> Result<()> {
    let network = local_call(port, "GET", "/api/network").await?;
    let pairing = local_call(port, "POST", "/api/pairing").await?;
    let base = network["addresses"]
        .as_array()
        .and_then(|a| a.iter().find(|a| a["kind"] != "localhost"))
        .and_then(|a| a["url"].as_str())
        .ok_or_else(|| {
            anyhow!("kimo is only reachable from this machine: start it with --bind all")
        })?;
    let url = format!(
        "{base}/#pair={}",
        pairing["code"].as_str().unwrap_or_default()
    );
    let qr = qrcode::QrCode::new(url.as_bytes())?;
    let art = qr
        .render::<qrcode::render::unicode::Dense1x2>()
        .dark_color(qrcode::render::unicode::Dense1x2::Light)
        .light_color(qrcode::render::unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build();
    println!("{art}\nScan with the phone camera, or open:\n  {url}\nThe code works once, for 10 minutes.");
    Ok(())
}

async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
