//! Choosing the address to listen on (loopback, LAN, Tailscale, ZeroTier, explicit IP).

use std::net::{IpAddr, Ipv4Addr};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindTarget {
    Localhost,
    /// All interfaces.
    Lan,
    Tailscale,
    ZeroTier,
    Ip(IpAddr),
}

pub fn parse_bind(s: &str) -> Result<BindTarget, String> {
    match s.to_ascii_lowercase().as_str() {
        "localhost" | "local" | "loopback" => Ok(BindTarget::Localhost),
        "lan" | "all" | "any" => Ok(BindTarget::Lan),
        "tailscale" | "ts" => Ok(BindTarget::Tailscale),
        "zerotier" | "zt" => Ok(BindTarget::ZeroTier),
        other => other.parse().map(BindTarget::Ip).map_err(|_| {
            format!("'{s}' is not localhost, lan, tailscale, zerotier or an IP address")
        }),
    }
}

/// First IPv4 address printed by `tailscale ip -4`.
pub fn parse_tailscale_ip(output: &str) -> Option<IpAddr> {
    output
        .lines()
        .find_map(|l| l.trim().parse::<Ipv4Addr>().ok())
        .map(IpAddr::V4)
}

/// IPv4 of the first ZeroTier interface (`zt*`) in `ip -4 -o addr show` output.
pub fn parse_zerotier_ip(output: &str) -> Option<IpAddr> {
    output.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let _index = parts.next()?;
        let iface = parts.next()?;
        if !iface.starts_with("zt") || parts.next()? != "inet" {
            return None;
        }
        let cidr = parts.next()?;
        cidr.split('/')
            .next()?
            .parse::<Ipv4Addr>()
            .ok()
            .map(IpAddr::V4)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets() {
        assert_eq!(parse_bind("Tailscale"), Ok(BindTarget::Tailscale));
        assert_eq!(parse_bind("zt"), Ok(BindTarget::ZeroTier));
        assert_eq!(
            parse_bind("10.0.0.2"),
            Ok(BindTarget::Ip("10.0.0.2".parse().unwrap()))
        );
        assert!(parse_bind("nope").is_err());
    }

    #[test]
    fn reads_tailscale_and_zerotier_addresses() {
        assert_eq!(
            parse_tailscale_ip("100.101.5.6\n"),
            Some("100.101.5.6".parse().unwrap())
        );
        assert_eq!(parse_tailscale_ip(""), None);
        let ip = "1: lo    inet 127.0.0.1/8 scope host lo\n\
                  3: ztabcdef12    inet 10.147.17.4/24 brd 10.147.17.255 scope global ztabcdef12\n";
        assert_eq!(parse_zerotier_ip(ip), Some("10.147.17.4".parse().unwrap()));
        assert_eq!(
            parse_zerotier_ip("1: lo    inet 127.0.0.1/8 scope host lo"),
            None
        );
    }
}
