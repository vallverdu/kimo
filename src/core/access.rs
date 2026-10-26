//! Who may call the API from where: origins (CORS) and what each kind of caller may do.

use super::models::Scope;

/// The authenticated caller of a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// kimo's own token (or localhost without a token): everything.
    Admin,
    /// An app key.
    Client { scope: Scope, origins: Vec<String> },
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

pub fn origin_listed(origin: &str, list: &[String]) -> bool {
    let origin = normalize(origin);
    list.iter().any(|o| o == "*" || normalize(o) == origin)
}

/// May `caller` use kimo from the cross-site page `origin`? `admin_origins` come from config.
pub fn cross_origin_allowed(caller: &Caller, origin: &str, admin_origins: &[String]) -> bool {
    match caller {
        Caller::Admin => origin_listed(origin, admin_origins),
        Caller::Client { origins, .. } => origin_listed(origin, origins),
    }
}

/// May `caller` call `method path` (path relative to `/api`)?
pub fn permits(caller: &Caller, method: &str, path: &str) -> bool {
    let Caller::Client { scope, .. } = caller else {
        return true;
    };
    let path = path.trim_end_matches('/');
    // Managing logins and app keys stays with the owner.
    let admin_only = [
        "/clients", "/network", "/pairing", "/devices", "/qr", "/settings", "/peers", "/update",
    ]
        .iter()
        .any(|p| path.starts_with(p))
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

#[cfg(test)]
mod tests {
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
}
