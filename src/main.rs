//! Entry point: config → manager → HTTP server. Nothing else.

use anyhow::Result;
use clap::Parser;
use kimo::config::{user_home, Config};
use kimo::core::network::{parse_bind, BindTarget};
use kimo::services::sessions::{Manager, ManagerConfig};
use std::net::SocketAddr;
use std::time::Duration;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kimo=info".into()),
        )
        .init();
    let cfg = Config::parse();
    let target = parse_bind(&cfg.bind).map_err(anyhow::Error::msg)?;
    let ip = kimo::services::network::resolve(&target)?;
    let token = match (&cfg.token, target) {
        (Some(t), _) if !t.is_empty() => Some(t.clone()),
        (_, BindTarget::Localhost) => None,
        _ => Some(uuid::Uuid::new_v4().simple().to_string()),
    };

    let manager = Manager::new(ManagerConfig {
        data_dir: cfg.data_dir(),
        user_home: user_home(),
        shell: Config::shell(),
        scrollback_bytes: cfg.scrollback_bytes,
    })?;
    let sampler = manager.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(2));
        loop {
            tick.tick().await;
            let m = sampler.clone();
            let _ = tokio::task::spawn_blocking(move || m.sample()).await;
        }
    });

    let addr = SocketAddr::new(ip, cfg.port);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let shown = if ip.is_unspecified() {
        "<this-machine>".to_string()
    } else {
        ip.to_string()
    };
    match &token {
        Some(t) => println!("kimo on http://{shown}:{}/#token={t}", cfg.port),
        None => println!("kimo on http://{shown}:{}/", cfg.port),
    }
    let app = kimo::api::router(manager.clone(), token, cfg.origins.clone());
    // Open terminal websockets never close by themselves, so stop on a signal instead of draining.
    tokio::select! {
        result = axum::serve(listener, app) => result?,
        _ = shutdown_signal() => tracing::info!("stopping sessions"),
    }
    manager.shutdown().await;
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
