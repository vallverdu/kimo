//! Who may call the API from where: origins (CORS) and what each kind of caller may do.

use super::models::Scope;

/// The authenticated caller of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// kimo's own token (or localhost without a token): everything.
    Admin,
    /// An app key.
    Client { scope: Scope, origins: Vec<String> },
    /// A fleet coordinator's key (one-way pairing): the machine's status and facts, jobs and
    /// power, nothing of the owner's sessions, files or keys.
    Fleet,
}

/// Origins compare without a trailing slash and case-insensitively.
fn normalize(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// True when the browser `Origin` is the kimo page itself (served from `host`).
pub fn is_same_origin(origin: &str, host: &str) -> bool {
    let origin = normalize(origin);
    let host = host.to_ascii_lowercase();
    origin == format!("http://{host}") || origin == format!("https://{host}")
}

/// Is `host` (a `Host` header: a name or address, with or without a port) one of this
/// machine's own names? Only then does a request from the machine itself act as its owner: a
/// name someone else points at this machine (DNS rebinding) must bring a key.
pub fn is_own_authority(host: &str, names: &[String], ips: &[std::net::IpAddr]) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let name = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or_default(),
        None => match host.rsplit_once(':') {
            Some((name, port))
                if !name.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) =>
            {
                name
            }
            _ => host.as_str(),
        },
    };
    let name = name.trim_end_matches('.');
    if name.is_empty() {
        return false;
    }
    if let Ok(ip) = name.parse::<std::net::IpAddr>() {
        return ip.is_loopback() || ips.contains(&ip);
    }
    name == "localhost" || names.iter().any(|n| n.eq_ignore_ascii_case(name))
}

/// This machine's own names for [`is_own_authority`]: its host names (with and without
/// `.local`) and the hosts of the configured admin origins.
pub fn own_names(host_names: &[String], admin_origins: &[String]) -> Vec<String> {
    let mut names = Vec::new();
    for n in host_names {
        let n = n.trim().trim_end_matches('.').to_ascii_lowercase();
        if n.is_empty() {
            continue;
        }
        let bare = n.strip_suffix(".local").unwrap_or(&n).to_string();
        names.push(format!("{bare}.local"));
        names.push(bare);
    }
    for o in admin_origins {
        let o = normalize(o);
        let host = o.split("://").nth(1).unwrap_or(&o);
        let host = host.split('/').next().unwrap_or_default();
        let host = match host.rsplit_once(':') {
            Some((h, p)) if p.bytes().all(|b| b.is_ascii_digit()) => h,
            _ => host,
        };
        if !host.is_empty() && host != "*" {
            names.push(host.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

pub fn origin_listed(origin: &str, list: &[String]) -> bool {
    let origin = normalize(origin);
    list.iter().any(|o| o == "*" || normalize(o) == origin)
}

/// May `caller` use kimo from the cross-site page `origin`? `admin_origins` come from config.
pub fn cross_origin_allowed(caller: &Caller, origin: &str, admin_origins: &[String]) -> bool {
    match caller {
        Caller::Admin => origin_listed(origin, admin_origins),
        Caller::Client { origins, .. } => origin_listed(origin, origins),
        // A coordinator is a program, never a web page.
        Caller::Fleet => false,
    }
}

/// What a fleet coordinator may call: the machine's status and facts, jobs, power.
fn fleet_permits(method: &str, path: &str) -> bool {
    let under = |p: &str| path == p || path.starts_with(&format!("{p}/"));
    (method == "GET" && (path == "/status" || path == "/node"))
        || (method == "DELETE" && path == "/fleet-key")
        || under("/jobs")
        || under("/power")
}

/// May `caller` call `method path` (path relative to `/api`)?
pub fn permits(caller: &Caller, method: &str, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    let scope = match caller {
        Caller::Admin => return true,
        Caller::Fleet => return fleet_permits(method, path),
        Caller::Client { scope, .. } => scope,
    };
    // Managing logins and app keys stays with the owner.
    let admin_only = [
        "/clients", "/network", "/pairing", "/devices", "/qr", "/settings", "/peers", "/update",
        // Paired kimos reached through this one get this kimo's own key there.
        "/hosts",
        // What else runs on the machine, its sleep and power.
        "/ports", "/awake", "/power",
        // The fleet coordinator (wakes and shuts down machines).
        "/fleet",
    ]
        .iter()
        .any(|p| path.starts_with(p))
        // A project's files and repository: the owner's (reading code and keys in it,
        // committing, pushing).
        || (path.starts_with("/projects/")
            && path
                .split('/')
                .nth(3)
                .is_some_and(|part| ["files", "find", "file", "git"].contains(&part)))
        // Installing or removing skills changes what every agent runs.
        || (path.starts_with("/skills") && method != "GET")
        || (path == "/accounts" && method != "GET")
        || (path.starts_with("/accounts/") && (method == "DELETE" || path.ends_with("/login")));
    if admin_only {
        return false;
    }
    match scope {
        Scope::Control => true,
        Scope::Read => method == "GET" && !path.ends_with("/term") && !path.starts_with("/fs"),
    }
}

/// Letters of pairing codes: no O, I, L or 1 to misread when typed from a screen.
const CODE_ALPHABET: &[u8; 32] = b"023456789ABCDEFGHJKMNPQRSTUVWXYZ";

/// A pairing code people can type: `K7M2-9QXP` (40 random bits) from a random number.
pub fn pairing_code(random: u128) -> String {
    let chars: String = (0..8)
        .map(|i| CODE_ALPHABET[((random >> (i * 5)) & 31) as usize] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

/// A typed or scanned pairing code as stored: capitals, no spaces or dashes.
pub fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn files_and_git_are_the_owner_s() {
        let app = Caller::Client {
            scope: Scope::Control,
            origins: vec![],
        };
        for path in [
            "/projects/p/files",
            "/projects/p/file",
            "/projects/p/find",
            "/projects/p/git/commit",
            "/projects/p/git",
        ] {
            assert!(!permits(&app, "GET", path), "{path}");
            assert!(permits(&Caller::Admin, "GET", path), "{path}");
        }
        // A project whose name starts like a route is still the app's to rename.
        assert!(permits(&app, "PATCH", "/projects/github-tools"));
        assert!(permits(&app, "GET", "/projects"));
    }

    #[test]
    fn fleet_keys_reach_only_status_facts_jobs_and_power() {
        let fleet = Caller::Fleet;
        for (m, p) in [
            ("GET", "/status"),
            ("GET", "/node"),
            ("GET", "/power"),
            ("POST", "/power/shutdown"),
            ("GET", "/jobs"),
            ("POST", "/jobs"),
            ("GET", "/jobs/j1/log"),
            ("POST", "/jobs/j1/cancel"),
            ("DELETE", "/fleet-key"),
        ] {
            assert!(permits(&fleet, m, p), "{m} {p}");
        }
        for (m, p) in [
            ("GET", "/sessions"),
            ("POST", "/sessions"),
            ("GET", "/sessions/a/term"),
            ("GET", "/events"),
            ("GET", "/accounts"),
            ("GET", "/devices"),
            ("POST", "/pairing"),
            ("GET", "/hosts/nuc/sessions"),
            ("GET", "/peers"),
            ("GET", "/settings"),
            ("GET", "/projects/p/files"),
            ("GET", "/fs"),
            ("POST", "/status"),
            ("GET", "/jobsx"),
            ("GET", "/powerful"),
            ("GET", "/fleet/devices"),
        ] {
            assert!(!permits(&fleet, m, p), "{m} {p}");
        }
        assert!(!cross_origin_allowed(
            &fleet,
            "https://pi.local",
            &["*".into()]
        ));
        // App keys never control the machine's power or the fleet.
        let app = Caller::Client {
            scope: Scope::Control,
            origins: vec![],
        };
        assert!(!permits(&app, "POST", "/power/shutdown"));
        assert!(!permits(&app, "POST", "/fleet/devices/nuc/wake"));
        assert!(permits(&app, "GET", "/node"));
    }

    #[test]
    fn own_names_only() {
        let ips: Vec<std::net::IpAddr> = vec!["192.168.1.150".parse().unwrap()];
        let names = own_names(
            &["Jordis-MacBook-Air.local".into(), "macbook".into()],
            &["https://kimo.example.com:8443".into(), "*".into()],
        );
        for ok in [
            "localhost",
            "localhost:7777",
            "LOCALHOST.:7777",
            "127.0.0.1:7777",
            "[::1]:7777",
            "::1",
            "192.168.1.150:7777",
            "jordis-macbook-air.local:7777",
            "Jordis-MacBook-Air",
            "macbook.local",
            "kimo.example.com",
        ] {
            assert!(is_own_authority(ok, &names, &ips), "{ok}");
        }
        for bad in [
            "",
            "attacker.example:7777",
            "localhost.attacker.example",
            "10.0.0.9:7777",
            "jordis-macbook-air.attacker.example",
            "[2001:db8::1]:7777",
            "localhost:77:77",
        ] {
            assert!(!is_own_authority(bad, &names, &ips), "{bad}");
        }
    }

    use super::*;

    fn client(scope: Scope) -> Caller {
        Caller::Client {
            scope,
            origins: vec!["https://pm.tail.ts.net".into()],
        }
    }

    #[test]
    fn same_origin_and_listed_origins() {
        assert!(is_same_origin("http://127.0.0.1:7777", "127.0.0.1:7777"));
        assert!(is_same_origin(
            "https://kimo.tail.ts.net",
            "kimo.tail.ts.net"
        ));
        assert!(!is_same_origin("https://evil.example", "127.0.0.1:7777"));
        assert!(origin_listed(
            "https://PM.tail.ts.net/",
            &["https://pm.tail.ts.net".into()]
        ));
        assert!(origin_listed("http://anything", &["*".into()]));
    }

    #[test]
    fn cross_origin_needs_a_listed_origin_for_the_caller() {
        let c = client(Scope::Control);
        assert!(cross_origin_allowed(&c, "https://pm.tail.ts.net", &[]));
        assert!(!cross_origin_allowed(
            &c,
            "https://evil.example",
            &["https://evil.example".into()]
        ));
        assert!(!cross_origin_allowed(
            &Caller::Admin,
            "https://evil.example",
            &[]
        ));
        assert!(cross_origin_allowed(
            &Caller::Admin,
            "https://ops.example",
            &["https://ops.example".into()]
        ));
    }

    #[test]
    fn scopes() {
        let (read, control) = (client(Scope::Read), client(Scope::Control));
        assert!(permits(&read, "GET", "/sessions"));
        assert!(permits(&read, "GET", "/events"));
        assert!(!permits(&read, "GET", "/sessions/a/term"));
        assert!(!permits(&read, "POST", "/sessions/a/action"));
        assert!(permits(&control, "POST", "/sessions/a/action"));
        assert!(permits(&control, "GET", "/sessions/a/term"));
        assert!(permits(&control, "POST", "/accounts/work/move"));
        assert!(permits(&control, "GET", "/accounts"));
        for (m, p) in [
            ("POST", "/accounts"),
            ("DELETE", "/accounts/w"),
            ("POST", "/accounts/w/login"),
            ("GET", "/clients"),
            ("POST", "/clients"),
        ] {
            assert!(!permits(&control, m, p), "{m} {p}");
        }
        for (m, p) in [
            ("GET", "/network"),
            ("GET", "/network/tailnet"),
            ("POST", "/pairing"),
            ("GET", "/devices"),
            ("GET", "/qr"),
        ] {
            assert!(!permits(&control, m, p), "{m} {p}");
        }
        assert!(!permits(&control, "GET", "/settings"));
        assert!(!permits(&control, "POST", "/skills") && !permits(&control, "DELETE", "/skills"));
        assert!(!permits(&control, "POST", "/skills/kimo"));
        assert!(permits(&read, "GET", "/skills") && permits(&read, "GET", "/agents"));
        assert!(permits(&read, "GET", "/projects"));
        assert!(!permits(&read, "GET", "/fs"));
        assert!(permits(&control, "POST", "/projects"));
        assert!(permits(&Caller::Admin, "POST", "/clients"));
    }

    #[test]
    fn pairing_codes() {
        let c = pairing_code(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        assert_eq!(c.len(), 9);
        assert_eq!(&c[4..5], "-");
        assert!(normalize_code(&c)
            .chars()
            .all(|c| CODE_ALPHABET.contains(&(c as u8))));
        assert_eq!(normalize_code(" k7m2-9qxp "), "K7M29QXP");
    }
}
