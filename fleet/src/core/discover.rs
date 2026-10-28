//! Machines on the coordinator's networks that could join the fleet: Tailscale's devices
//! (`tailscale status --json`), ZeroTier's members (its Central API, or the hosts of the
//! ZeroTier network this machine is on). Pure parsing; the looking is in services.

use serde::Serialize;
use std::net::Ipv4Addr;

/// A machine seen on a network.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Seen {
    pub name: String,
    /// The address its kimo would answer on.
    pub address: String,
    /// `tailscale` or `zerotier`.
    pub network: &'static str,
    /// `linux`, `macOS`, `iOS`… (empty when the network does not say).
    pub os: String,
    pub online: bool,
    /// The coordinator's own machine.
    #[serde(rename = "self")]
    pub this: bool,
    /// It can run kimo (Linux or macOS, or unknown); phones and Windows cannot.
    pub can_run_kimo: bool,
    /// Its full name on the network (`nuc.tail1234.ts.net`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_name: Option<String>,
}

fn can_run(os: &str) -> bool {
    let os = os.to_ascii_lowercase();
    os.is_empty() || os == "linux" || os == "macos"
}

/// Machines from `tailscale status --json`: this one first, then the others (IPv4 address).
pub fn parse_tailscale(status_json: &str) -> Vec<Seen> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(status_json) else {
        return vec![];
    };
    let one = |d: &serde_json::Value, this: bool| -> Option<Seen> {
        let address = d["TailscaleIPs"]
            .as_array()?
            .iter()
            .filter_map(|i| i.as_str())
            .find(|i| i.parse::<Ipv4Addr>().is_ok())?
            .to_string();
        let dns = d["DNSName"]
            .as_str()
            .unwrap_or_default()
            .trim_end_matches('.');
        let host = d["HostName"].as_str().unwrap_or_default();
        let name = dns
            .split('.')
            .next()
            .filter(|n| !n.is_empty())
            .unwrap_or(host)
            .to_string();
        let os = d["OS"].as_str().unwrap_or_default().to_string();
        Some(Seen {
            can_run_kimo: can_run(&os),
            name,
            address,
            network: "tailscale",
            os,
            online: this || d["Online"].as_bool().unwrap_or(false),
            this,
            dns_name: (!dns.is_empty()).then(|| dns.to_string()),
        })
    };
    let mut out: Vec<Seen> = one(&v["Self"], true).into_iter().collect();
    let mut peers: Vec<Seen> = v["Peer"]
        .as_object()
        .map(|p| p.values().filter_map(|d| one(d, false)).collect())
        .unwrap_or_default();
    peers.sort_by(|a, b| (!a.online, &a.name).cmp(&(!b.online, &b.name)));
    out.extend(peers);
    out
}

/// A ZeroTier network this machine is on (`zerotier-cli -j listnetworks`): its id and this
/// machine's IPv4 address with the prefix.
pub fn parse_zerotier_networks(json: &str) -> Vec<(String, Ipv4Addr, u8)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|n| {
            let id = n["nwid"].as_str()?.to_string();
            let (ip, prefix) = n["assignedAddresses"]
                .as_array()?
                .iter()
                .filter_map(|a| a.as_str()?.split_once('/'))
                .find_map(|(ip, p)| Some((ip.parse::<Ipv4Addr>().ok()?, p.parse::<u8>().ok()?)))?;
            Some((id, ip, prefix))
        })
        .collect()
}

/// Members of a ZeroTier network from its Central API
/// (`GET https://api.zerotier.com/api/v1/network/{id}/member`), authorized ones only.
pub fn parse_zerotier_members(json: &str, now_ms: u64) -> Vec<Seen> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return vec![];
    };
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|m| m["config"]["authorized"] != false)
        .filter_map(|m| {
            let address = m["config"]["ipAssignments"]
                .as_array()?
                .iter()
                .filter_map(|i| i.as_str())
                .find(|i| i.parse::<Ipv4Addr>().is_ok())?
                .to_string();
            let name = m["name"]
                .as_str()
                .filter(|n| !n.is_empty())
                .or(m["nodeId"].as_str())
                .unwrap_or("zerotier")
                .to_string();
            let seen = m["lastSeen"]
                .as_u64()
                .or(m["lastOnline"].as_u64())
                .unwrap_or(0);
            Some(Seen {
                name,
                address,
                network: "zerotier",
                os: String::new(),
                online: now_ms.saturating_sub(seen) < 5 * 60_000,
                this: false,
                can_run_kimo: true,
                dns_name: None,
            })
        })
        .collect()
}

/// The other hosts of a small network (at most a /24): where to look for machines when the
/// network gives no list.
pub fn hosts(ip: Ipv4Addr, prefix: u8) -> Vec<Ipv4Addr> {
    if !(24..=30).contains(&prefix) {
        return vec![];
    }
    let mask = u32::MAX << (32 - u32::from(prefix));
    let net = u32::from(ip) & mask;
    let last = net | !mask;
    (net + 1..last)
        .map(Ipv4Addr::from)
        .filter(|h| *h != ip)
        .collect()
}

/// A device id from a name (`Jordi's MacBook Air` → `jordis-macbook-air`).
pub fn id_from(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c == '\'' {
            continue;
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_end_matches('-').chars().take(48).collect();
    if out.is_empty() {
        "machine".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_devices() {
        let json = r#"{
          "Self": {"HostName": "pi4", "DNSName": "pi4.tail1234.ts.net.", "OS": "linux", "TailscaleIPs": ["100.64.0.1", "fd7a::1"], "Online": true},
          "Peer": {
            "a": {"HostName": "NUC", "DNSName": "nuc-extreme.tail1234.ts.net.", "OS": "linux", "TailscaleIPs": ["100.64.0.2"], "Online": false},
            "b": {"HostName": "iPhone", "DNSName": "iphone.tail1234.ts.net.", "OS": "iOS", "TailscaleIPs": ["100.64.0.3"], "Online": true},
            "c": {"HostName": "Jordi's MacBook Air", "DNSName": "jordis-macbook-air.tail1234.ts.net.", "OS": "macOS", "TailscaleIPs": ["100.64.0.4"], "Online": true}
          }}"#;
        let s = parse_tailscale(json);
        assert_eq!(s.len(), 4);
        assert!(s[0].this && s[0].name == "pi4" && s[0].address == "100.64.0.1");
        // Online first, then by name.
        let names: Vec<&str> = s.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            ["pi4", "iphone", "jordis-macbook-air", "nuc-extreme"]
        );
        assert!(!s[1].can_run_kimo && s[2].can_run_kimo);
        assert!(!s[3].online);
        assert_eq!(
            s[3].dns_name.as_deref(),
            Some("nuc-extreme.tail1234.ts.net")
        );
        assert!(parse_tailscale("not json").is_empty());
    }

    #[test]
    fn zerotier_networks_and_members() {
        let nets =
            r#"[{"nwid":"8056c2e21c000001","assignedAddresses":["fd80::1/88","10.147.17.5/24"]}]"#;
        assert_eq!(
            parse_zerotier_networks(nets),
            vec![(
                "8056c2e21c000001".into(),
                "10.147.17.5".parse().unwrap(),
                24
            )]
        );
        let members = r#"[
          {"nodeId":"aaaa","name":"nuc-extreme","config":{"authorized":true,"ipAssignments":["10.147.17.21"]},"lastSeen":1000000},
          {"nodeId":"bbbb","name":"","config":{"authorized":true,"ipAssignments":["10.147.17.30"]},"lastSeen":1},
          {"nodeId":"cccc","name":"stranger","config":{"authorized":false,"ipAssignments":["10.147.17.99"]}}
        ]"#;
        let m = parse_zerotier_members(members, 1_100_000);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].name, "nuc-extreme");
        assert!(m[0].online && !m[1].online);
        assert_eq!(m[1].name, "bbbb");
    }

    #[test]
    fn subnets_and_ids() {
        let h = hosts("10.147.17.5".parse().unwrap(), 24);
        assert_eq!(h.len(), 253);
        assert!(!h.contains(&"10.147.17.5".parse().unwrap()));
        assert!(hosts("10.147.0.5".parse().unwrap(), 16).is_empty());
        assert_eq!(id_from("Jordi's MacBook Air"), "jordis-macbook-air");
        assert_eq!(id_from("NUC 11 / Extreme"), "nuc-11-extreme");
        assert_eq!(id_from("…"), "machine");
    }
}
