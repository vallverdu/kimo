//! Resolving a bind target to an address (asks tailscale / ip when needed).

use crate::core::network::{parse_tailscale_ip, parse_zerotier_ip, BindTarget};
use anyhow::{anyhow, Result};
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
