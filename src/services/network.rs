//! Resolving a bind target to an address (asks tailscale / ip when needed).

use crate::core::network::{
    parse_ifconfig, parse_interfaces, parse_tailscale_dns, parse_tailscale_ip, tailscale_https_url,
    tailscale_used_ports, BindTarget, Interface, NetKind,
};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long a helper command (ip, ifconfig, tailscale) may take before kimo gives up on it.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Run a command with a time limit (a stuck CLI, such as the Tailscale app's on macOS when its
/// app is not running, must never block kimo). Returns its output.
fn run(cmd: &mut Command) -> Result<std::process::Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Read both pipes on threads so a chatty command cannot fill them and stall.
    let mut out = child.stdout.take().expect("piped");
    let mut err = child.stderr.take().expect("piped");
    let out = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = out.read_to_end(&mut b);
        b
    });
    let err = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("timed out after {} s", COMMAND_TIMEOUT.as_secs());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(std::process::Output {
        status,
        stdout: out.join().unwrap_or_default(),
        stderr: err.join().unwrap_or_default(),
    })
}

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out =
        run(Command::new(program).args(args)).map_err(|e| anyhow!("running {program}: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The Tailscale CLI (on macOS it can live inside the app bundle), if it works. Looked up once.
fn tailscale_cli() -> Option<&'static str> {
    static CLI: OnceLock<Option<&'static str>> = OnceLock::new();
    *CLI.get_or_init(|| {
        const APP: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
        let works = |p: &str| run(Command::new(p).arg("version")).is_ok_and(|o| o.status.success());
        if works("tailscale") {
            Some("tailscale")
        } else if cfg!(target_os = "macos") && std::path::Path::new(APP).exists() && works(APP) {
            Some(APP)
        } else {
            None
        }
    })
}

fn tailscale() -> &'static str {
    tailscale_cli().unwrap_or("tailscale")
}

/// This machine's IPv4 interfaces (`ip` on Linux, `ifconfig` elsewhere).
pub fn interfaces() -> Vec<Interface> {
    if cfg!(target_os = "linux") {
        parse_interfaces(&output("ip", &["-4", "-o", "addr", "show"]).unwrap_or_default())
    } else {
        parse_ifconfig(&output("ifconfig", &[]).unwrap_or_default())
    }
}

pub fn resolve(target: &BindTarget) -> Result<IpAddr> {
    match target {
        BindTarget::Localhost => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        BindTarget::Lan => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        BindTarget::Ip(ip) => Ok(*ip),
        BindTarget::Tailscale => parse_tailscale_ip(&output(tailscale(), &["ip", "-4"])?)
            .ok_or_else(|| anyhow!("no Tailscale address: is tailscale up?")),
        BindTarget::ZeroTier => interfaces()
            .into_iter()
            .find(|i| i.kind == NetKind::ZeroTier)
            .map(|i| IpAddr::V4(i.ip))
            .ok_or_else(|| anyhow!("no ZeroTier interface (zt*) with an IPv4 address")),
    }
}

/// Every IPv4 address of this machine (to recognise requests coming from the machine itself).
pub fn local_ips() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = interfaces().into_iter().map(|i| IpAddr::V4(i.ip)).collect();
    ips.push(IpAddr::V4(Ipv4Addr::LOCALHOST));
    ips
}

#[derive(Debug, Clone, Serialize)]
pub struct Address {
    pub kind: NetKind,
    pub url: String,
    /// Installable as an app (HTTPS) and reachable from other devices.
    pub https: bool,
    pub label: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkInfo {
    pub port: u16,
    /// Best first: Tailscale HTTPS, Tailscale, ZeroTier, LAN, this machine.
    pub addresses: Vec<Address>,
    pub tailscale: Option<TailscaleInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TailscaleInfo {
    pub dns_name: Option<String>,
    pub https_url: Option<String>,
}

/// Where kimo can be reached, given the address it listens on.
pub fn info(bound: IpAddr, port: u16) -> NetworkInfo {
    let mut addresses: Vec<Address> = interfaces()
        .into_iter()
        .filter(|i| {
            bound.is_unspecified()
                || IpAddr::V4(i.ip) == bound
                || i.kind == NetKind::Localhost && bound.is_loopback()
        })
        .map(|i| Address {
            kind: i.kind,
            url: if i.kind == NetKind::Localhost {
                format!("http://localhost:{port}")
            } else {
                format!("http://{}:{port}", i.ip)
            },
            https: false,
            label: match i.kind {
                NetKind::Localhost => "this machine".into(),
                NetKind::Lan => format!("local network ({})", i.name),
                NetKind::Tailscale => "Tailscale".into(),
                NetKind::ZeroTier => "ZeroTier".into(),
            },
        })
        .collect();
    let tailscale = tailscale_cli().is_some().then(|| {
        let dns_name = output(tailscale(), &["status", "--json"])
            .ok()
            .and_then(|s| parse_tailscale_dns(&s));
        let https_url = output(tailscale(), &["serve", "status", "--json"])
            .ok()
            .and_then(|s| tailscale_https_url(&s, port));
        TailscaleInfo {
            dns_name,
            https_url,
        }
    });
    if let Some(url) = tailscale.as_ref().and_then(|t| t.https_url.clone()) {
        addresses.push(Address {
            kind: NetKind::Tailscale,
            url,
            https: true,
            label: "Tailscale HTTPS (installable app)".into(),
        });
    }
    let rank = |a: &Address| match (a.https, a.kind) {
        (true, _) => 0,
        (_, NetKind::Tailscale) => 1,
        (_, NetKind::ZeroTier) => 2,
        (_, NetKind::Lan) => 3,
        (_, NetKind::Localhost) => 4,
    };
    addresses.sort_by_key(rank);
    NetworkInfo {
        port,
        addresses,
        tailscale,
    }
}

/// Serve kimo over HTTPS on the tailnet with `tailscale serve`, on a free HTTPS port
/// (never one another service already uses). Returns the URL.
pub fn enable_tailscale_https(port: u16) -> Result<String> {
    let status = output(tailscale(), &["serve", "status", "--json"]).unwrap_or_default();
    if let Some(url) = tailscale_https_url(&status, port) {
        return Ok(url);
    }
    let used = tailscale_used_ports(&status);
    let https_port = [443u16, 8443, 9443, 10443]
        .into_iter()
        .find(|p| !used.contains(p) && *p != port)
        .ok_or_else(|| anyhow!("no free HTTPS port for tailscale serve"))?;
    let out = run(Command::new(tailscale()).args([
        "serve",
        "--bg",
        &format!("--https={https_port}"),
        &format!("http://127.0.0.1:{port}"),
    ]))
    .map_err(|e| anyhow!("running tailscale: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Access denied") || err.contains("permission") {
            bail!("tailscale needs permission once: run `sudo tailscale set --operator=$USER` on the server");
        }
        bail!("tailscale serve failed: {}", err.trim());
    }
    let status = output(tailscale(), &["serve", "status", "--json"]).unwrap_or_default();
    tailscale_https_url(&status, port)
        .ok_or_else(|| anyhow!("tailscale serve did not report the new address"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stuck_commands_are_given_up() {
        let started = Instant::now();
        assert!(run(Command::new("sleep").arg("30")).is_err());
        assert!(started.elapsed() < COMMAND_TIMEOUT + Duration::from_secs(2));
        assert_eq!(output("echo", &["hi"]).unwrap(), "hi\n");
    }
}
