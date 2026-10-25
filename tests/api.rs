//! Access control as other PWAs see it: CORS, app keys bound to origins, scopes, events.

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{header, Request, StatusCode};
use futures_util::StreamExt;
use kimo::services::sessions::{Manager, ManagerConfig};
use std::net::SocketAddr;
use tower::ServiceExt;

const APP: &str = "https://pm.tail.ts.net";
const EVIL: &str = "https://evil.example";

fn router(tmp: &std::path::Path, admin_token: Option<&str>) -> axum::Router {
    std::fs::create_dir_all(tmp.join("home")).unwrap();
    let manager = Manager::new(ManagerConfig {
        data_dir: tmp.join("kimo"),
        user_home: tmp.join("home"),
        shell: "/bin/sh".into(),
        scrollback_bytes: 1024,
        projects_dir: tmp.join("home/dev"),
        port: 7777,
        host_name: "test".into(),
    })
    .unwrap();
    kimo::api::router(
        manager,
        kimo::api::Options {
            token: admin_token.map(String::from),
            admin_origins: vec![],
            local_owner: true,
            bound: "0.0.0.0".parse().unwrap(),
        },
    )
}

/// The same request as if it came from the machine itself.
fn local(mut r: Request<Body>) -> Request<Body> {
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
    r
}

/// The same request as if it came from another device on the network.
fn remote(mut r: Request<Body>) -> Request<Body> {
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([203, 0, 113, 9], 50000))));
    r
}

fn req(
    method: &str,
    uri: &str,
    origin: Option<&str>,
    token: Option<&str>,
    body: Option<&str>,
) -> Request<Body> {
    let mut r = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "kimo.local:7777");
    if let Some(o) = origin {
        r = r.header(header::ORIGIN, o);
    }
    if let Some(t) = token {
        r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    match body {
        Some(b) => r
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => r.body(Body::empty()),
    }
    .unwrap()
}

async fn json(res: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn other_sites_cannot_use_localhost_without_a_key() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let own = app
        .clone()
        .oneshot(local(req("GET", "/api/sessions", None, None, None)))
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::OK);
    let same = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/sessions",
            Some("http://kimo.local:7777"),
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(same.status(), StatusCode::OK);
    let evil = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/sessions/x/term",
            Some(EVIL),
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(evil.status(), StatusCode::UNAUTHORIZED);
    let pre = app
        .oneshot(req("OPTIONS", "/api/sessions", Some(EVIL), None, None))
        .await
        .unwrap();
    assert_eq!(pre.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn app_keys_are_bound_to_origins_and_scopes() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), Some("admin-secret"));

    // Only the owner manages keys.
    let anon = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/clients",
            None,
            None,
            Some(r#"{"name":"x"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(anon.status(), StatusCode::UNAUTHORIZED);
    let body = format!(r#"{{"name":"Project manager","origins":["{APP}/"],"scope":"read"}}"#);
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/clients",
            None,
            Some("admin-secret"),
            Some(&body),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let client = json(created).await;
    let key = client["token"].as_str().unwrap().to_string();
    assert_eq!(client["origins"][0], APP);
    let control = json(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/clients",
                None,
                Some("admin-secret"),
                Some(r#"{"name":"Acme ERP","origins":["https://erp.tail.ts.net"]}"#),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(control["scope"], "control");

    // Preflight from a known origin is allowed, with CORS headers.
    let pre = app
        .clone()
        .oneshot(req("OPTIONS", "/api/sessions", Some(APP), None, None))
        .await
        .unwrap();
    assert_eq!(pre.status(), StatusCode::NO_CONTENT);
    assert_eq!(pre.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], APP);

    // The key works from its origin, not from another site, and not with another app's origin.
    let ok = app
        .clone()
        .oneshot(req("GET", "/api/sessions", Some(APP), Some(&key), None))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], APP);
    let evil = app
        .clone()
        .oneshot(req("GET", "/api/sessions", Some(EVIL), Some(&key), None))
        .await
        .unwrap();
    assert_eq!(evil.status(), StatusCode::FORBIDDEN);
    let wrong = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/sessions",
            Some("https://erp.tail.ts.net"),
            Some(&key),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

    // Read scope: no changes, no terminals, no key management.
    let new = r#"{"name":"x","kind":"shell","cwd":"","start":false}"#;
    for (m, uri, b) in [
        ("POST", "/api/sessions", Some(new)),
        ("GET", "/api/sessions/x/term", None),
        ("GET", "/api/clients", None),
    ] {
        let res = app
            .clone()
            .oneshot(req(m, uri, Some(APP), Some(&key), b))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "{m} {uri}");
    }

    // Control scope may create sessions; the key list never shows secrets.
    let made = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/sessions",
            Some("https://erp.tail.ts.net"),
            control["token"].as_str(),
            Some(new),
        ))
        .await
        .unwrap();
    assert_eq!(made.status(), StatusCode::OK);
    let list = json(
        app.clone()
            .oneshot(req("GET", "/api/clients", None, Some("admin-secret"), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert!(list[0].get("token").is_none());

    // Revoked keys stop working.
    let id = client["id"].as_str().unwrap();
    let del = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/clients/{id}"),
            None,
            Some("admin-secret"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    let gone = app
        .oneshot(req("GET", "/api/sessions", Some(APP), Some(&key), None))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn events_stream_starts_with_the_full_state() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let res = app
        .oneshot(local(req("GET", "/api/events", None, None, None)))
        .await
        .unwrap();
    assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
    let first = res
        .into_body()
        .into_data_stream()
        .next()
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&first);
    assert!(text.starts_with("event: update\ndata: {"), "{text}");
    assert!(text.contains("\"sessions\":[]"));
}

#[tokio::test]
async fn sdk_is_loadable_from_any_origin() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), Some("admin-secret"));
    let res = app
        .oneshot(req("GET", "/sdk/kimo.js", Some(APP), None, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
}

#[tokio::test]
async fn the_server_itself_is_the_owner_but_not_through_a_proxy() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let own = app
        .clone()
        .oneshot(local(req("GET", "/api/devices", None, None, None)))
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::OK);
    let other = app
        .clone()
        .oneshot(remote(req("GET", "/api/devices", None, None, None)))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::UNAUTHORIZED);
    // tailscale serve connects from localhost but says who the user is: not the owner.
    let mut proxied = local(req("GET", "/api/devices", None, None, None));
    proxied
        .headers_mut()
        .insert("x-forwarded-for", "100.101.1.2".parse().unwrap());
    assert_eq!(
        app.oneshot(proxied).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn pairing_codes_work_once_and_devices_can_be_revoked() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    // Only the owner shows a pairing QR.
    let denied = app
        .clone()
        .oneshot(remote(req("POST", "/api/pairing", None, None, None)))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let pairing = json(
        app.clone()
            .oneshot(local(req("POST", "/api/pairing", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    let code = pairing["code"].as_str().unwrap();
    assert_eq!(pairing["expires_in"], 600);
    let qr = app
        .clone()
        .oneshot(local(req(
            "GET",
            &format!("/api/qr?text=http://x/%23pair={code}"),
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(qr.headers()[header::CONTENT_TYPE], "image/svg+xml");

    // The phone trades the code for its key.
    let body = format!(r#"{{"code":"{code}","name":"Ana's phone"}}"#);
    let device = json(
        app.clone()
            .oneshot(remote(req("POST", "/api/pair", None, None, Some(&body))))
            .await
            .unwrap(),
    )
    .await;
    let key = device["token"].as_str().unwrap().to_string();
    let again = app
        .clone()
        .oneshot(remote(req("POST", "/api/pair", None, None, Some(&body))))
        .await
        .unwrap();
    assert_eq!(again.status(), StatusCode::FORBIDDEN, "codes work once");
    let wrong = app
        .clone()
        .oneshot(remote(req(
            "POST",
            "/api/pair",
            None,
            None,
            Some(r#"{"code":"nope"}"#),
        )))
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);

    // A paired device is the owner: accounts, keys, devices.
    let accounts = app
        .clone()
        .oneshot(remote(req(
            "POST",
            "/api/accounts",
            None,
            Some(&key),
            Some(r#"{"name":"Work","provider":"claude"}"#),
        )))
        .await
        .unwrap();
    assert_eq!(accounts.status(), StatusCode::OK);
    let devices = json(
        app.clone()
            .oneshot(remote(req("GET", "/api/devices", None, Some(&key), None)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(devices[0]["name"], "Ana's phone");
    assert!(devices[0].get("token").is_none());

    let id = devices[0]["id"].as_str().unwrap();
    let del = app
        .clone()
        .oneshot(local(req(
            "DELETE",
            &format!("/api/devices/{id}"),
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::OK);
    let gone = app
        .oneshot(remote(req("GET", "/api/sessions", None, Some(&key), None)))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn projects_new_existing_clone_and_browse() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let call = |m: &'static str, uri: String, body: Option<String>| {
        let app = app.clone();
        async move {
            let res = app
                .oneshot(local(req(m, &uri, None, None, body.as_deref())))
                .await
                .unwrap();
            (res.status(), json(res).await)
        }
    };
    let dev = tmp.path().join("home/dev");

    // New folder with git in the projects folder.
    let (status, made) = call(
        "POST",
        "/api/projects".into(),
        Some(r#"{"name":"Acme Invoices","mode":"new","git":true}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(
        made["project"]["path"],
        dev.join("acme-invoices").display().to_string()
    );
    assert!(dev.join("acme-invoices/.git").is_dir());
    let (_, again) = call(
        "POST",
        "/api/projects".into(),
        Some(r#"{"name":"acme invoices","mode":"new"}"#.into()),
    )
    .await;
    assert!(again["error"].as_str().unwrap().contains("already"));

    // An existing folder keeps its name.
    std::fs::create_dir_all(dev.join("old-site/src")).unwrap();
    let body = format!(
        r#"{{"mode":"existing","path":"{}"}}"#,
        dev.join("old-site").display()
    );
    let (status, existing) = call("POST", "/api/projects".into(), Some(body)).await;
    assert_eq!(
        (status, existing["project"]["name"].as_str()),
        (StatusCode::OK, Some("old-site"))
    );

    // Clone runs in a terminal session in the new folder.
    let origin = tmp.path().join("origin.git");
    assert!(std::process::Command::new("git")
        .args(["init", "-q", "--bare"])
        .arg(&origin)
        .status()
        .unwrap()
        .success());
    let body = format!(r#"{{"mode":"clone","url":"{}"}}"#, origin.display());
    let (status, cloned) = call("POST", "/api/projects".into(), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{cloned}");
    assert_eq!(cloned["project"]["name"], "origin");
    assert_eq!(cloned["session"]["command"][1], "clone");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !dev.join("origin/.git").is_dir() {
        assert!(std::time::Instant::now() < deadline, "clone did not finish");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // List with branch and session counts; browse shows folders with git marks.
    let (_, list) = call("GET", "/api/projects".into(), None).await;
    let list = list.as_array().unwrap();
    assert_eq!(list.len(), 3);
    let invoices = list.iter().find(|p| p["name"] == "Acme Invoices").unwrap();
    assert!(invoices["branch"].is_string());
    assert_eq!(
        list.iter().find(|p| p["name"] == "origin").unwrap()["sessions"],
        1
    );
    let (_, listing) = call("GET", "/api/fs".into(), None).await;
    assert_eq!(listing["path"], dev.display().to_string());
    let names: Vec<_> = listing["dirs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            (
                d["name"].as_str().unwrap().to_string(),
                d["git"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        names,
        vec![
            ("acme-invoices".into(), true),
            ("old-site".into(), false),
            ("origin".into(), true)
        ]
    );

    // Forgetting a project never deletes its files.
    let id = invoices["id"].as_str().unwrap();
    let (status, _) = call("DELETE", format!("/api/projects/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dev.join("acme-invoices/.git").is_dir());
}

#[tokio::test]
async fn settings_and_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let call = |m: &'static str, uri: String, body: Option<String>| {
        let app = app.clone();
        async move {
            let res = app
                .oneshot(local(req(m, &uri, None, None, body.as_deref())))
                .await
                .unwrap();
            (res.status(), json(res).await)
        }
    };

    // Settings round-trip with defaults filled in; invalid values refused.
    let (_, settings) = call("GET", "/api/settings".into(), None).await;
    assert_eq!(settings["agents"]["claude"]["permissions"], "manual");
    assert_eq!(settings["terminal"]["font_size"], 13.0);
    let body = r#"{"agents":{"claude":{"permissions":"yolo","model":"opus"}},"quick_commands":[{"name":"tests","text":"run the tests"}]}"#;
    let (status, saved) = call("PUT", "/api/settings".into(), Some(body.into())).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["agents"]["codex"]["enabled"], true);
    let (status, _) = call(
        "PUT",
        "/api/settings".into(),
        Some(r#"{"terminal":{"font_size":2}}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A skills source with two skills, one of them nested.
    let src = tmp.path().join("src-skills");
    for (dir, name) in [("pdf", "pdf"), ("group/review-pr", "review-pr")] {
        std::fs::create_dir_all(src.join(dir)).unwrap();
        std::fs::write(
            src.join(dir).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Does {name}\n---\nbody"),
        )
        .unwrap();
    }
    std::fs::write(src.join("pdf/helper.py"), "print(1)").unwrap();
    let home = tmp.path().join("home");
    // A synced (claude.ai) skill is listed but read-only.
    std::fs::create_dir_all(home.join(".claude/skills/synced/abc/xlsx")).unwrap();
    std::fs::write(
        home.join(".claude/skills/synced/abc/xlsx/SKILL.md"),
        "---\nname: xlsx\n---",
    )
    .unwrap();

    let body = format!(
        r#"{{"source":"{}","agents":["claude","codex"]}}"#,
        src.display()
    );
    let (status, installed) = call("POST", "/api/skills".into(), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{installed}");
    assert_eq!(installed.as_array().unwrap().len(), 4);
    assert!(home.join(".claude/skills/pdf/helper.py").is_file());
    assert!(home.join(".codex/skills/review-pr/SKILL.md").is_file());
    let (status, again) = call("POST", "/api/skills".into(), Some(body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(again["error"].as_str().unwrap().contains("already exists"));

    let (_, list) = call("GET", "/api/skills".into(), None).await;
    let list = list.as_array().unwrap();
    let names: Vec<_> = list
        .iter()
        .map(|s| {
            (
                s["name"].as_str().unwrap(),
                s["agent"].as_str().unwrap(),
                s["managed"].as_bool().unwrap(),
            )
        })
        .collect();
    assert!(names.contains(&("xlsx", "claude", true)));
    assert!(names.contains(&("review-pr", "codex", false)));
    assert_eq!(
        list.iter().find(|s| s["name"] == "pdf").unwrap()["description"],
        "Does pdf"
    );

    // Replace keeps a backup; remove moves to the backups folder; synced skills are refused.
    let replace = format!(
        r#"{{"source":"{}","skill":"pdf","agents":["claude"],"replace":true}}"#,
        src.display()
    );
    let (status, _) = call("POST", "/api/skills".into(), Some(replace)).await;
    assert_eq!(status, StatusCode::OK);
    let pdf = home.join(".claude/skills/pdf").display().to_string();
    let (status, removed) = call("DELETE", format!("/api/skills?path={pdf}"), None).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert!(!home.join(".claude/skills/pdf").exists());
    assert!(std::path::Path::new(removed["backup"].as_str().unwrap())
        .join("helper.py")
        .is_file());
    let backups = std::fs::read_dir(tmp.path().join("kimo/skill-backups"))
        .unwrap()
        .count();
    assert_eq!(backups, 2, "one from replace, one from remove");
    let synced = home
        .join(".claude/skills/synced/abc/xlsx")
        .display()
        .to_string();
    let (status, _) = call("DELETE", format!("/api/skills?path={synced}"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call("DELETE", "/api/skills?path=/etc".into(), None).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "only known skills can be removed"
    );

    // kimo's own skill installs for both agents; a second install keeps a backup.
    let (status, own) = call("POST", "/api/skills/kimo".into(), Some("{}".into())).await;
    assert_eq!(
        (status, own.as_array().unwrap().len()),
        (StatusCode::OK, 2),
        "{own}"
    );
    let skill = std::fs::read_to_string(home.join(".claude/skills/kimo-agents/SKILL.md")).unwrap();
    assert!(skill.contains("kimo send <id|name>"));
    assert!(home.join(".codex/skills/kimo-agents/SKILL.md").is_file());
    let (status, _) = call(
        "POST",
        "/api/skills/kimo".into(),
        Some(r#"{"agents":["claude"]}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Installing from a git repository (file:// URL keeps the test offline).
    let repo = tmp.path().join("skills-repo");
    std::fs::create_dir_all(repo.join("skills/lint")).unwrap();
    std::fs::write(
        repo.join("skills/lint/SKILL.md"),
        "---\nname: lint\ndescription: Lint\n---",
    )
    .unwrap();
    let git = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success())
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&[
        "-c",
        "user.email=t@t",
        "-c",
        "user.name=t",
        "commit",
        "-qm",
        "skills",
    ]);
    let bare = tmp.path().join("skills.git");
    assert!(std::process::Command::new("git")
        .args(["clone", "-q", "--bare"])
        .arg(&repo)
        .arg(&bare)
        .status()
        .unwrap()
        .success());
    let body = format!(
        r#"{{"source":"file://{}","agents":["claude"]}}"#,
        bare.display()
    );
    let (status, cloned) = call("POST", "/api/skills".into(), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{cloned}");
    assert!(home.join(".claude/skills/lint/SKILL.md").is_file());
    assert!(
        !tmp.path()
            .join("kimo/tmp")
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "clone cleaned up"
    );
}
