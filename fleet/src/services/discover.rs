//! Finding machines to add: Tailscale's devices, ZeroTier's members (Central API with a token)
//! or the hosts of the ZeroTier network this machine is on, each checked for a kimo and SSH.

use super::Fleet;
use crate::core::discover::{self, Seen};
use crate::core::hardware::{self, Clues};
use crate::services::net;
use serde::Serialize;
use std::net::{SocketAddr, TcpStream};
use std::process::Command;
use std::time::Duration;

/// A machine found, with what answers on it and what it would be added as.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    #[serde(flatten)]
    pub seen: Seen,
    /// A free device id for it.
    pub id: String,
    /// The machine of the fleet it already is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_fleet: Option<String>,
    /// Its kimo answers (`paired`: with this coordinator's key).
    pub kimo: &'static str,
    /// SSH answers: kimo can be installed there from here.
    pub ssh: bool,
    /// The hardware model and role it would be added with.
    pub model: &'static str,
    pub role: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct Discovery {
    pub tailscale: bool,
    pub zerotier: bool,
    /// What could not be looked at, in plain words.
    pub notes: Vec<String>,
    pub machines: Vec<Candidate>,
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn open(addr: &str, port: u16, ms: u64) -> bool {
    addr.parse::<std::net::IpAddr>().is_ok_and(|ip| {
        TcpStream::connect_timeout(&SocketAddr::new(ip, port), Duration::from_millis(ms)).is_ok()
    })
}

impl Fleet {
    fn tailscale_cli(&self) -> Option<String> {
        if let Some(t) = &self.cfg.tailscale {
            return Some(t.clone());
        }
        [
            "tailscale",
            "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
        ]
        .into_iter()
        .find(|p| {
            Command::new(p)
                .arg("version")
                .output()
                .is_ok_and(|o| o.status.success())
        })
        .map(str::to_string)
    }

    /// The machines on Tailscale and ZeroTier that could join the fleet.
    pub fn discover(&self) -> Discovery {
        let mut notes = vec![];
        let mut seen: Vec<Seen> = vec![];
        let tailscale = match self.tailscale_cli() {
            Some(cli) => match output(&cli, &["status", "--json"]) {
                Some(json) => {
                    seen.extend(discover::parse_tailscale(&json));
                    true
                }
                None => {
                    notes.push("Tailscale is installed here but not connected".into());
                    false
                }
            },
            None => false,
        };
        let zerotier = self.zerotier(&mut seen, &mut notes);
        if !tailscale && !zerotier {
            notes.push("neither Tailscale nor ZeroTier is on this machine".into());
        }
        // Each machine once (Tailscale's name wins), the ones this machine is on left as found.
        let mut unique: Vec<Seen> = vec![];
        for s in seen {
            if !unique.iter().any(|u| u.address == s.address) {
                unique.push(s);
            }
        }
        let (registry, keys) = {
            let inner = self.lock();
            (inner.registry.clone(), inner.keys.clone())
        };
        let port = self.cfg.kimo_port;
        let mut machines: Vec<Candidate> = std::thread::scope(|scope| {
            let handles: Vec<_> = unique
                .into_iter()
                .map(|s| {
                    let (registry, keys) = (&registry, &keys);
                    scope.spawn(move || {
                        let in_fleet = registry
                            .devices
                            .iter()
                            .find(|(_, d)| {
                                let host = crate::core::registry::host_of(&d.kimo);
                                host.as_deref() == Some(s.address.as_str())
                                    || (s.dns_name.is_some() && host == s.dns_name)
                            })
                            .map(|(id, _)| id.clone());
                        let look = s.online && s.can_run_kimo;
                        let url = format!("http://{}:{port}", s.address);
                        let kimo = if !look {
                            "unknown"
                        } else {
                            match net::kimo(&url, None).raw("GET", "/status", vec![], None, 2) {
                                Ok((200 | 401, _, _))
                                    if in_fleet.as_ref().is_some_and(|i| keys.contains_key(i)) =>
                                {
                                    "paired"
                                }
                                Ok((200 | 401, _, _)) => "answers",
                                _ => "no",
                            }
                        };
                        let ssh = look && open(&s.address, 22, 1500);
                        let m = hardware::guess(&Clues {
                            name: &s.name,
                            os: &s.os,
                            facts: None,
                        });
                        let mut id = discover::id_from(&s.name);
                        let mut n = 2;
                        while registry.devices.contains_key(&id)
                            && in_fleet.as_deref() != Some(id.as_str())
                        {
                            id = format!("{}-{n}", discover::id_from(&s.name));
                            n += 1;
                        }
                        Candidate {
                            id: in_fleet.clone().unwrap_or(id),
                            in_fleet,
                            kimo,
                            ssh,
                            model: m.id,
                            role: m.role,
                            seen: s,
                        }
                    })
                })
                .collect();
            handles.into_iter().filter_map(|h| h.join().ok()).collect()
        });
        machines.sort_by_key(|c| (c.in_fleet.is_some(), !c.seen.can_run_kimo, !c.seen.online));
        Discovery {
            tailscale,
            zerotier,
            notes,
            machines,
        }
    }

    /// ZeroTier members: from Central with a token (`KIMO_FLEET_ZEROTIER_TOKEN` or the file
    /// `zerotier-token` in the data folder), else the hosts of this machine's ZeroTier network
    /// where kimo or SSH answers.
    fn zerotier(&self, seen: &mut Vec<Seen>, notes: &mut Vec<String>) -> bool {
        if !self.cfg.zerotier {
            return false;
        }
        // This machine's ZeroTier cards (no root needed, unlike zerotier-cli).
        let cards: Vec<(std::net::Ipv4Addr, u8)> = kimo::services::node::nics()
            .into_iter()
            .filter(|n| n.name.starts_with("zt") || n.name.starts_with("feth"))
            .filter_map(|n| Some((n.ipv4.parse().ok()?, n.prefix)))
            .collect();
        let cli = self
            .cfg
            .zerotier_cli
            .clone()
            .unwrap_or_else(|| "zerotier-cli".into());
        let networks = output(&cli, &["-j", "listnetworks"])
            .map(|j| discover::parse_zerotier_networks(&j))
            .unwrap_or_default();
        if cards.is_empty() && networks.is_empty() {
            return false;
        }
        let token = std::env::var("KIMO_FLEET_ZEROTIER_TOKEN").ok().or_else(|| {
            std::fs::read_to_string(self.cfg.data_dir.join("zerotier-token"))
                .ok()
                .map(|t| t.trim().to_string())
        });
        if let (Some(token), false) = (token.filter(|t| !t.is_empty()), networks.is_empty()) {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(10)))
                .http_status_as_error(false)
                .build()
                .into();
            for (nwid, _, _) in &networks {
                let url = format!("https://api.zerotier.com/api/v1/network/{nwid}/member");
                match agent
                    .get(&url)
                    .header("Authorization", &format!("token {token}"))
                    .call()
                    .ok()
                    .filter(|r| r.status().is_success())
                    .and_then(|mut r| r.body_mut().read_to_string().ok())
                {
                    Some(json) => {
                        seen.extend(discover::parse_zerotier_members(&json, super::now_ms()))
                    }
                    None => notes.push(format!("ZeroTier Central did not list network {nwid}")),
                }
            }
            return true;
        }
        // No list: the hosts of the network where kimo or SSH answers.
        let mut addrs: Vec<(std::net::Ipv4Addr, u8)> = cards;
        addrs.extend(networks.iter().map(|(_, ip, p)| (*ip, *p)));
        addrs.sort();
        addrs.dedup();
        let hosts: Vec<std::net::Ipv4Addr> = addrs
            .iter()
            .flat_map(|(ip, p)| discover::hosts(*ip, *p))
            .collect();
        if hosts.is_empty() {
            notes.push("the ZeroTier network is larger than a /24: add a Central token to list its members".into());
            return true;
        }
        let port = self.cfg.kimo_port;
        let found: Vec<std::net::Ipv4Addr> = std::thread::scope(|scope| {
            hosts
                .chunks(4)
                .map(|chunk| {
                    scope.spawn(move || {
                        chunk
                            .iter()
                            .filter(|h| {
                                open(&h.to_string(), port, 300) || open(&h.to_string(), 22, 300)
                            })
                            .copied()
                            .collect::<Vec<_>>()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .filter_map(|h| h.join().ok())
                .flatten()
                .collect()
        });
        seen.extend(found.into_iter().map(|ip| Seen {
            name: ip.to_string(),
            address: ip.to_string(),
            network: "zerotier",
            os: String::new(),
            online: true,
            this: false,
            can_run_kimo: true,
            dns_name: None,
        }));
        true
    }
}
