//! Entry point: the coordinator (`kimo-fleet serve`) or a command for it.

use anyhow::{Context, Result};
use clap::Parser;
use kimo::client::Client;
use kimo_fleet::config::{Command, Config};
use kimo_fleet::services::fleet::{self, Fleet};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kimo_fleet=info".into()),
        )
        .init();
    let cfg = Config::parse();
    let client = || {
        Client::new(
            &format!("http://127.0.0.1:{}", cfg.port),
            cfg.token.clone().filter(|t| !t.is_empty()),
        )
    };
    match cfg.command.clone().unwrap_or(Command::Serve) {
        Command::Serve => serve(&cfg),
        Command::Devices => devices(&client()),
        Command::Pair { device, code } => {
            client().call(
                "POST",
                &format!("/devices/{device}/pair"),
                Some(&json!({ "code": code })),
            )?;
            println!("paired with {device}: the coordinator can see it, run jobs and switch it on and off");
            Ok(())
        }
        Command::Wake { device } => {
            client().post(&format!("/devices/{device}/wake"), json!({}))?;
            println!("waking {device}");
            Ok(())
        }
        Command::Shutdown { device } => {
            client().post(&format!("/devices/{device}/shutdown"), json!({}))?;
            println!("switching {device} off");
            Ok(())
        }
        Command::Hold {
            device,
            hours,
            never,
            manual,
            auto,
        } => {
            let mut body = json!({});
            if let Some(h) = hours {
                body["hours"] = json!(h);
            }
            if never {
                body["never"] = json!(true);
            }
            if manual {
                body["manual"] = json!(true);
            }
            if auto {
                body["never"] = json!(false);
                body["manual"] = json!(false);
            }
            let d = client().post(&format!("/devices/{device}/hold"), body)?;
            println!("{device}: {}", d["state"].as_str().unwrap_or(""));
            Ok(())
        }
        Command::Run { file } => run(&client(), &file),
        Command::Jobs => jobs(&client()),
        Command::Log { job } => log(&client(), &job),
        Command::Cancel { id } => {
            let path = if id.starts_with("w-") {
                format!("/workflows/{id}/cancel")
            } else {
                format!("/jobs/{id}/cancel")
            };
            client().post(&path, json!({}))?;
            println!("cancelled {id}");
            Ok(())
        }
        Command::Approve { job, reject } => {
            let what = if reject { "reject" } else { "approve" };
            client().post(&format!("/jobs/{job}/{what}"), json!({}))?;
            println!("{job}: {}", if reject { "rejected" } else { "approved" });
            Ok(())
        }
        Command::Events { device, limit } => {
            let q = match device {
                Some(d) => format!("/events?device={d}&limit={limit}"),
                None => format!("/events?limit={limit}"),
            };
            let list = client().call("GET", &q, None)?;
            for e in list.as_array().into_iter().flatten().rev() {
                println!(
                    "{}  {:<14} {:<10} {}",
                    time(e["at"].as_u64().unwrap_or(0)),
                    e["device"].as_str().or(e["job"].as_str()).unwrap_or(""),
                    e["kind"].as_str().unwrap_or(""),
                    e["detail"].as_str().unwrap_or("")
                );
            }
            Ok(())
        }
        Command::Reload => {
            let r = client().post("/reload", json!({}))?;
            println!("{} machines", r["devices"]);
            Ok(())
        }
        Command::Backup { to } => {
            let fleet = Fleet::open(fleet::Config::new(cfg.data_dir(), cfg.host_name()))?;
            fleet.backup(&to)?;
            println!("copied to {}", to.display());
            Ok(())
        }
    }
}

fn serve(cfg: &Config) -> Result<()> {
    let fleet = Fleet::open(fleet::Config::new(cfg.data_dir(), cfg.host_name()))?;
    if let Some(e) = fleet.registry_error() {
        eprintln!("{e}");
    }
    let looper = fleet.clone();
    std::thread::Builder::new()
        .name("fleet-loop".into())
        .spawn(move || loop {
            looper.tick();
            std::thread::sleep(Duration::from_secs(2));
        })?;
    let ip: std::net::IpAddr = cfg
        .bind
        .parse()
        .with_context(|| format!("{} is not an address", cfg.bind))?;
    if !ip.is_loopback() && cfg.token.as_deref().unwrap_or("").is_empty() {
        eprintln!("note: only this machine can use the coordinator without --token");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, cfg.port))
            .await
            .with_context(|| format!("cannot listen on port {}", cfg.port))?;
        println!(
            "kimo-fleet is running on {}:{} with {} machines ({})",
            ip,
            cfg.port,
            fleet.devices().len(),
            fleet.registry_path().display()
        );
        println!(
            "  the kimo here reaches it with: kimo serve --fleet http://127.0.0.1:{}",
            cfg.port
        );
        let app = kimo_fleet::api::router(fleet, cfg.token.clone());
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
        anyhow::Ok(())
    })
}

fn time(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    format!("{h:02}:{m:02}:{s:02} UTC")
}

fn devices(c: &Client) -> Result<()> {
    let list = c.call("GET", "/devices", None)?;
    for d in list.as_array().into_iter().flatten() {
        let facts = &d["facts"];
        let gpu = facts["gpus"][0]["name"].as_str().unwrap_or("");
        println!(
            "{:<16} {:<12} {:<8} {}{}",
            d["id"].as_str().unwrap_or(""),
            d["state"].as_str().unwrap_or(""),
            if d["paired"] == true {
                "paired"
            } else {
                "unpaired"
            },
            d["labels"]
                .as_array()
                .map(|l| l
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(","))
                .unwrap_or_default(),
            if gpu.is_empty() {
                String::new()
            } else {
                format!("  {gpu}")
            },
        );
        if let Some(e) = d["error"].as_str() {
            println!("{:<16} {e}", "");
        }
    }
    Ok(())
}

fn run(c: &Client, file: &std::path::Path) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&text)?;
    let workflow = value.get("workflow").is_some();
    let path = if workflow { "/workflows" } else { "/jobs" };
    let (status, _, body) = c.raw("POST", path, text.into_bytes(), Some("text/yaml"), 30)?;
    let answer: Value = serde_json::from_slice(&body).unwrap_or_default();
    if status >= 300 {
        anyhow::bail!("{}", answer["error"].as_str().unwrap_or("refused"));
    }
    println!(
        "{} {}",
        if workflow { "workflow" } else { "job" },
        answer["id"].as_str().unwrap_or("")
    );
    Ok(())
}

fn jobs(c: &Client) -> Result<()> {
    let list = c.call("GET", "/jobs", None)?;
    for j in list.as_array().into_iter().flatten() {
        println!(
            "{:<14} {:<20} {:<16} {:<14} {}",
            j["id"].as_str().unwrap_or(""),
            j["spec"]["task"].as_str().unwrap_or(""),
            j["state"].as_str().unwrap_or(""),
            j["device"].as_str().unwrap_or("-"),
            j["reason"].as_str().unwrap_or("")
        );
    }
    Ok(())
}

fn log(c: &Client, job: &str) -> Result<()> {
    let mut offset = 0;
    loop {
        let chunk = c.call("GET", &format!("/jobs/{job}/log?offset={offset}"), None)?;
        print!("{}", chunk["text"].as_str().unwrap_or(""));
        let next = chunk["next"].as_u64().unwrap_or(offset);
        if next <= offset {
            return Ok(());
        }
        offset = next;
    }
}
