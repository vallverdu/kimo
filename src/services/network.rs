//! Resolving a bind target to an address (asks tailscale / ip when needed).

use crate::core::network::{
    parse_interfaces, parse_tailscale_dns, parse_tailscale_ip, parse_zerotier_ip,
    tailscale_https_url, tailscale_used_ports, BindTarget, NetKind,
};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;

fn output(program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| anyhow!("running {program}: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn resolve(target: &BindTarget) -> Result<IpAddr> {
    match target {
        BindTarget::Localhost => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        BindTarget::Lan => Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        BindTarget::Ip(ip) => Ok(*ip),
        BindTarget::Tailscale => parse_tailscale_ip(&output("tailscale", &["ip", "-4"])?)
            .ok_or_else(|| anyhow!("no Tailscale address: is tailscale up?")),
        BindTarget::ZeroTier => parse_zerotier_ip(&output("ip", &["-4", "-o", "addr", "show"])?)
            .ok_or_else(|| anyhow!("no ZeroTier interface (zt*) with an IPv4 address")),
    }
}

/// Every IPv4 address of this machine (to recognise requests coming from the machine itself).
pub fn local_ips() -> Vec<IpAddr> {
    let out = output("ip", &["-4", "-o", "addr", "show"]).unwrap_or_default();
    let mut ips: Vec<IpAddr> = parse_interfaces(&out)
        .into_iter()
        .map(|i| IpAddr::V4(i.ip))
        .collect();
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
    let ip_out = output("ip", &["-4", "-o", "addr", "show"]).unwrap_or_default();
    let mut addresses: Vec<Address> = parse_interfaces(&ip_out)
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
    let tailscale = which("tailscale").then(|| {
        let dns_name = output("tailscale", &["status", "--json"])
            .ok()
            .and_then(|s| parse_tailscale_dns(&s));
        let https_url = output("tailscale", &["serve", "status", "--json"])
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

fn which(program: &str) -> bool {
    Command::new(program)
        .arg("version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// Serve kimo over HTTPS on the tailnet with `tailscale serve`, on a free HTTPS port
/// (never one another service already uses). Returns the URL.
pub fn enable_tailscale_https(port: u16) -> Result<String> {
    let status = output("tailscale", &["serve", "status", "--json"]).unwrap_or_default();
    if let Some(url) = tailscale_https_url(&status, port) {
        return Ok(url);
    }
    let used = tailscale_used_ports(&status);
    let https_port = [443u16, 8443, 9443, 10443]
        .into_iter()
        .find(|p| !used.contains(p) && *p != port)
        .ok_or_else(|| anyhow!("no free HTTPS port for tailscale serve"))?;
    let out = Command::new("tailscale")
        .args([
            "serve",
            "--bg",
            &format!("--https={https_port}"),
            &format!("http://127.0.0.1:{port}"),
        ])
        .output()
        .map_err(|e| anyhow!("running tailscale: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("Access denied") || err.contains("permission") {
            bail!("tailscale needs permission once: run `sudo tailscale set --operator=$USER` on the server");
        }
        bail!("tailscale serve failed: {}", err.trim());
    }
    let status = output("tailscale", &["serve", "status", "--json"]).unwrap_or_default();
    tailscale_https_url(&status, port)
        .ok_or_else(|| anyhow!("tailscale serve did not report the new address"))
}
