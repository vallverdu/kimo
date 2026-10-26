//! Listening TCP ports: parsing `lsof -F pcn`, `netstat -an -p tcp` (macOS) and `ss -Hltnp`
//! (Linux), and merging what they say into one entry per port and process.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Listener {
    pub port: u16,
    /// Where it listens (`127.0.0.1`, `*`, `::1`), one per address family.
    pub addresses: Vec<String>,
    /// The process, when the system tells (your own processes; others need root).
    pub pid: Option<i32>,
    pub process: Option<String>,
}

/// `127.0.0.1:7777`, `*:22`, `[::1]:5199`, `0.0.0.0:631` → (address, port).
fn split_colon(addr: &str) -> Option<(String, u16)> {
    let (host, port) = addr.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    Some((host.to_string(), port.parse().ok()?))
}

/// `lsof -nP -iTCP -sTCP:LISTEN -Fpcn`: `p<pid>`, `c<command>`, then `n<address>` per socket.
pub fn parse_lsof(out: &str) -> Vec<Listener> {
    let (mut pid, mut command) = (None, None);
    let mut found = Vec::new();
    for line in out.lines() {
        let (tag, value) = line.split_at(line.len().min(1));
        match tag {
            "p" => {
                pid = value.parse().ok();
                command = None;
            }
            "c" => command = Some(value.to_string()),
            "n" => {
                if let Some((address, port)) = split_colon(value) {
                    found.push(Listener {
                        port,
                        addresses: vec![address],
                        pid,
                        process: command.clone(),
                    });
                }
            }
            _ => {}
        }
    }
    merge(found)
}

/// `netstat -an -p tcp` (macOS): `tcp4 0 0 127.0.0.1.7777 *.* LISTEN`; no processes.
pub fn parse_netstat(out: &str) -> Vec<Listener> {
    let found = out
        .lines()
        .filter(|l| l.split_whitespace().last() == Some("LISTEN"))
        .filter_map(|l| {
            let local = l.split_whitespace().nth(3)?;
            let (host, port) = local.rsplit_once('.')?;
            Some(Listener {
                port: port.parse().ok()?,
                addresses: vec![host.to_string()],
                pid: None,
                process: None,
            })
        })
        .collect();
    merge(found)
}

/// `ss -Hltnp` (Linux): `LISTEN 0 4096 127.0.0.1:631 0.0.0.0:* users:(("cupsd",pid=1,fd=7))`.
pub fn parse_ss(out: &str) -> Vec<Listener> {
    let found = out
        .lines()
        .filter_map(|l| {
            let fields: Vec<&str> = l.split_whitespace().collect();
            let local = fields.get(3)?;
            let (address, port) = split_colon(local)?;
            let address = address.split('%').next().unwrap_or_default().to_string();
            let users = l.find("users:((").map(|i| &l[i + 8..]);
            let process = users
                .and_then(|u| u.strip_prefix('"'))
                .and_then(|u| u.split('"').next())
                .map(String::from);
            let pid = users
                .and_then(|u| u.split("pid=").nth(1))
                .and_then(|p| p.split([',', ')']).next())
                .and_then(|p| p.parse().ok());
            Some(Listener {
                port,
                addresses: vec![address],
                pid,
                process,
            })
        })
        .collect();
    merge(found)
}

/// One entry per port and process; a port seen without a process is dropped when another
/// listing knows its process. Sorted by port.
pub fn merge(all: Vec<Listener>) -> Vec<Listener> {
    let mut out: Vec<Listener> = Vec::new();
    for l in all {
        let same = out
            .iter_mut()
            .find(|o| o.port == l.port && (o.pid == l.pid || o.pid.is_none() || l.pid.is_none()));
        match same {
            Some(o) => {
                if o.pid.is_none() && l.pid.is_some() {
                    o.pid = l.pid;
                    o.process = l.process.clone();
                }
                for a in l.addresses {
                    if !o.addresses.contains(&a) {
                        o.addresses.push(a);
                    }
                }
            }
            None => out.push(l),
        }
    }
    out.sort_by_key(|l| (l.port, l.pid));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsof_output() {
        let out = "p476\ncrapportd\nf11\nn*:58194\nf14\nn*:58194\np12901\nckimo\nf11\nn*:7777\np29378\ncnode\nf20\nn[::1]:5199\nf21\nn127.0.0.1:5199\n";
        let l = parse_lsof(out);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].port, 5199);
        assert_eq!(l[0].process.as_deref(), Some("node"));
        assert_eq!(l[0].addresses, vec!["::1", "127.0.0.1"]);
        assert_eq!((l[1].port, l[1].pid), (7777, Some(12901)));
        assert_eq!(l[2].addresses, vec!["*"]);
    }

    #[test]
    fn netstat_and_merge() {
        let out = "Active Internet connections (including servers)\nProto Recv-Q Send-Q  Local Address          Foreign Address        (state)\ntcp4       0      0  127.0.0.1.61634        *.*                    LISTEN\ntcp46      0      0  *.7777                 *.*                    LISTEN\ntcp6       0      0  ::1.5199               *.*                    LISTEN\ntcp4       0      0  192.168.1.5.50000      1.2.3.4.443            ESTABLISHED\n";
        let n = parse_netstat(out);
        assert_eq!(
            n.iter().map(|l| l.port).collect::<Vec<_>>(),
            vec![5199, 7777, 61634]
        );
        let mut all = n;
        all.extend(parse_lsof("p12901\nckimo\nn*:7777\n"));
        let merged = merge(all);
        assert_eq!(merged.len(), 3);
        let kimo = merged.iter().find(|l| l.port == 7777).unwrap();
        assert_eq!(kimo.process.as_deref(), Some("kimo"));
        assert_eq!(merged.iter().filter(|l| l.pid.is_none()).count(), 2);
    }

    #[test]
    fn ss_output() {
        let out = "LISTEN 0      4096   127.0.0.53%lo:53        0.0.0.0:*\nLISTEN 0      511    0.0.0.0:7777       0.0.0.0:*    users:((\"kimo\",pid=812,fd=9))\nLISTEN 0      128    [::]:22            [::]:*\n";
        let l = parse_ss(out);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].addresses, vec!["::"]);
        assert_eq!(l[1].addresses, vec!["127.0.0.53"]);
        assert_eq!(
            (l[2].port, l[2].pid, l[2].process.as_deref()),
            (7777, Some(812), Some("kimo"))
        );
    }
}
