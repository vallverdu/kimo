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
    parse_interfaces(output)
        .into_iter()
        .find(|i| i.kind == NetKind::ZeroTier)
        .map(|i| IpAddr::V4(i.ip))
}

/// Which network an address belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NetKind {
    Localhost,
    Lan,
    Tailscale,
    ZeroTier,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub kind: NetKind,
}

fn classify(name: &str, ip: Ipv4Addr) -> Option<Interface> {
    let virtual_iface = [
        "docker", "br-", "veth", "virbr", "cni", "flannel", "lxc", "bridge", "vmenet", "anpi",
    ]
    .iter()
    .any(|p| name.starts_with(p));
    if virtual_iface || ip.is_link_local() {
        return None;
    }
    let in_cgnat = ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]);
    let kind = if ip.is_loopback() {
        NetKind::Localhost
    } else if name.starts_with("tailscale") || in_cgnat {
        NetKind::Tailscale
    } else if name.starts_with("zt") || name.starts_with("feth") {
        NetKind::ZeroTier
    } else {
        NetKind::Lan
    };
    Some(Interface {
        name: name.to_string(),
        ip,
        kind,
    })
}

/// Usable IPv4 interfaces from `ip -4 -o addr show` (Linux); containers, bridges and
/// link-local addresses skipped.
pub fn parse_interfaces(output: &str) -> Vec<Interface> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let _index = parts.next()?;
            let name = parts.next()?.trim_end_matches(':');
            if parts.next()? != "inet" {
                return None;
            }
            let ip: Ipv4Addr = parts.next()?.split('/').next()?.parse().ok()?;
            classify(name, ip)
        })
        .collect()
}

/// Usable IPv4 interfaces from `ifconfig` (macOS, BSD).
pub fn parse_ifconfig(output: &str) -> Vec<Interface> {
    let mut out = vec![];
    let mut name = String::new();
    for line in output.lines() {
        if !line.starts_with([' ', '\t']) {
            name = line.split(':').next().unwrap_or("").to_string();
        } else if let Some(rest) = line.trim().strip_prefix("inet ") {
            if let Some(ip) = rest
                .split_whitespace()
                .next()
                .and_then(|a| a.parse::<Ipv4Addr>().ok())
            {
                out.extend(classify(&name, ip));
            }
        }
    }
    out
}

/// This machine's MagicDNS name from `tailscale status --json` (without the trailing dot).
pub fn parse_tailscale_dns(status_json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(status_json).ok()?;
    let name = v["Self"]["DNSName"].as_str()?.trim_end_matches('.');
    (!name.is_empty()).then(|| name.to_string())
}

/// HTTPS URL under which `tailscale serve` already proxies to kimo's `port`, if any
/// (from `tailscale serve status --json`).
pub fn tailscale_https_url(serve_json: &str, port: u16) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(serve_json).ok()?;
    let target = format!(":{port}");
    v["Web"].as_object()?.iter().find_map(|(host_port, web)| {
        let proxies_kimo = web["Handlers"].as_object()?.values().any(|h| {
            h["Proxy"].as_str().is_some_and(|p| {
                p.trim_end_matches('/').ends_with(&target)
                    && (p.contains("127.0.0.1") || p.contains("localhost"))
            })
        });
        if !proxies_kimo {
            return None;
        }
        Some(match host_port.strip_suffix(":443") {
            Some(host) => format!("https://{host}"),
            None => format!("https://{host_port}"),
        })
    })
}

/// HTTPS ports `tailscale serve` already uses, so kimo never takes one over.
pub fn tailscale_used_ports(serve_json: &str) -> Vec<u16> {
    serde_json::from_str::<serde_json::Value>(serve_json)
        .ok()
        .and_then(|v| {
            v["TCP"]
                .as_object()
                .map(|tcp| tcp.keys().filter_map(|k| k.parse().ok()).collect())
        })
        .unwrap_or_default()
}

/// Of this kimo's addresses, the one a host at `remote_url` most likely reaches: the same
/// network kind (Tailscale, ZeroTier) or the same /24 on a LAN.
pub fn pick_address(candidates: &[(NetKind, String)], remote_url: &str) -> Option<String> {
    let host = remote_url
        .split("://")
        .nth(1)
        .unwrap_or(remote_url)
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    let ip: Option<Ipv4Addr> = host.parse().ok();
    let kind = match ip {
        _ if host.ends_with(".ts.net") => NetKind::Tailscale,
        Some(ip) if ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]) => {
            NetKind::Tailscale
        }
        Some(ip) if ip.is_loopback() => NetKind::Localhost,
        _ => NetKind::Lan,
    };
    let same_subnet = |url: &str| {
        let h = url
            .split("://")
            .nth(1)
            .unwrap_or(url)
            .split(':')
            .next()
            .unwrap_or("");
        match (h.parse::<Ipv4Addr>(), ip) {
            (Ok(a), Some(b)) => a.octets()[..3] == b.octets()[..3],
            _ => false,
        }
    };
    candidates
        .iter()
        .find(|(k, u)| *k == kind && (kind != NetKind::Lan || same_subnet(u)))
        .or_else(|| {
            candidates
                .iter()
                .find(|(k, u)| same_subnet(u) && *k != NetKind::Localhost)
        })
        .or_else(|| candidates.iter().find(|(k, _)| *k == kind))
        .or_else(|| candidates.iter().find(|(k, _)| *k != NetKind::Localhost))
        .map(|(_, u)| u.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IP_ADDR: &str = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
        2: enp89s0    inet 169.254.97.139/16 brd 169.254.255.255 scope link enp89s0\n\
        3: wlp0s20f3    inet 192.168.1.142/24 brd 192.168.1.255 scope global dynamic wlp0s20f3\n\
        4: tailscale0    inet 100.101.1.10/32 scope global tailscale0\n\
        5: docker0    inet 172.17.0.1/16 brd 172.17.255.255 scope global docker0\n\
        6: ztabc    inet 10.147.17.4/24 scope global ztabc\n";

    #[test]
    fn picks_the_address_a_peer_reaches() {
        let mine = vec![
            (NetKind::Tailscale, "http://100.101.1.10:7777".to_string()),
            (NetKind::Lan, "http://192.168.1.142:7777".to_string()),
            (NetKind::ZeroTier, "http://10.147.17.4:7777".to_string()),
            (NetKind::Localhost, "http://localhost:7777".to_string()),
        ];
        assert_eq!(
            pick_address(&mine, "http://100.70.1.2:7777").unwrap(),
            "http://100.101.1.10:7777"
        );
        assert_eq!(
            pick_address(&mine, "https://lab.tail1234.ts.net").unwrap(),
            "http://100.101.1.10:7777"
        );
        assert_eq!(
            pick_address(&mine, "http://192.168.1.50:7777").unwrap(),
            "http://192.168.1.142:7777"
        );
        assert_eq!(
            pick_address(&mine, "http://10.147.17.9:7777").unwrap(),
            "http://10.147.17.4:7777"
        );
        assert_eq!(
            pick_address(&mine, "http://127.0.0.1:7796").unwrap(),
            "http://localhost:7777"
        );
    }

    #[test]
    fn reads_ifconfig() {
        let out = "lo0: flags=8049<UP,LOOPBACK,RUNNING,MULTICAST> mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\n\
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500\n\tether 3c:22:fb:00:00:01\n\tinet 192.168.1.20 netmask 0xffffff00 broadcast 192.168.1.255\n\
utun4: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1280\n\tinet 100.88.1.2 --> 100.88.1.2 netmask 0xffffffff\n\
bridge100: flags=8863<UP> mtu 1500\n\tinet 192.168.64.1 netmask 0xffffff00\n";
        let found: Vec<_> = parse_ifconfig(out)
            .into_iter()
            .map(|i| (i.name, i.kind))
            .collect();
        assert_eq!(
            found,
            vec![
                ("lo0".into(), NetKind::Localhost),
                ("en0".into(), NetKind::Lan),
                ("utun4".into(), NetKind::Tailscale)
            ]
        );
    }

    #[test]
    fn classifies_interfaces() {
        let found: Vec<_> = parse_interfaces(IP_ADDR)
            .into_iter()
            .map(|i| (i.name, i.ip.to_string(), i.kind))
            .collect();
        assert_eq!(
            found,
            vec![
                ("lo".into(), "127.0.0.1".into(), NetKind::Localhost),
                ("wlp0s20f3".into(), "192.168.1.142".into(), NetKind::Lan),
                (
                    "tailscale0".into(),
                    "100.101.1.10".into(),
                    NetKind::Tailscale
                ),
                ("ztabc".into(), "10.147.17.4".into(), NetKind::ZeroTier),
            ]
        );
    }

    #[test]
    fn reads_tailscale_status_and_serve() {
        assert_eq!(
            parse_tailscale_dns(r#"{"Self":{"DNSName":"nuc.tail1234.ts.net."}}"#)
                .as_deref(),
            Some("nuc.tail1234.ts.net")
        );
        let serve = r#"{"TCP":{"443":{"HTTPS":true},"8443":{"HTTPS":true}},"Web":{
            "nuc.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:8800"}}},
            "nuc.ts.net:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7777"}}}}}"#;
        assert_eq!(
            tailscale_https_url(serve, 7777).as_deref(),
            Some("https://nuc.ts.net:8443")
        );
        assert_eq!(
            tailscale_https_url(serve, 8800).as_deref(),
            Some("https://nuc.ts.net")
        );
        assert_eq!(tailscale_https_url(serve, 9999), None);
        let mut ports = tailscale_used_ports(serve);
        ports.sort();
        assert_eq!(ports, vec![443, 8443]);
    }

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
