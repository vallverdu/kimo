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
            fleet: None,
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
        .header(header::HOST, "localhost:7777");
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
            Some("http://localhost:7777"),
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

    // Folder pickers create folders, to start a project from nothing.
    let body = format!(r#"{{"parent":"{}","name":"brand new"}}"#, dev.display());
    let (status, made) = call("POST", "/api/fs".into(), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    assert_eq!(made["path"], dev.join("brand new").display().to_string());
    assert!(dev.join("brand new").is_dir());
    let (status, _) = call("POST", "/api/fs".into(), Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "already exists");
    let bad = format!(r#"{{"parent":"{}","name":"../escape"}}"#, dev.display());
    let (status, _) = call("POST", "/api/fs".into(), Some(bad)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Forgetting a project never deletes its files.
    let id = invoices["id"].as_str().unwrap();
    let (status, _) = call("DELETE", format!("/api/projects/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(dev.join("acme-invoices/.git").is_dir());
}

#[tokio::test]
async fn project_settings_and_worktrees() {
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
    // A repository with one commit.
    let repo = tmp.path().join("home/dev/app");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let ok = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "init.defaultBranch=main",
            ])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "git {args:?}");
    };
    git(&["init", "-q"]);
    std::fs::write(repo.join("README.md"), "app").unwrap();
    git(&["add", "."]);
    git(&["commit", "-qm", "first"]);
    let body = format!(r#"{{"mode":"existing","path":"{}"}}"#, repo.display());
    let (status, made) = call("POST", "/api/projects".into(), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{made}");
    let id = made["project"]["id"].as_str().unwrap().to_string();
    assert_eq!(made["reopened"], false);

    // Adding it again opens it instead of failing.
    let (status, again) = call("POST", "/api/projects".into(), Some(body)).await;
    assert_eq!(
        (status, &again["reopened"]),
        (StatusCode::OK, &serde_json::json!(true))
    );

    // Rename and colour; bad colours are refused.
    let (status, p) = call(
        "PATCH",
        format!("/api/projects/{id}"),
        Some(r##"{"name":"The App","color":"#12939A"}"##.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{p}");
    assert_eq!(
        (p["name"].as_str(), p["color"].as_str()),
        (Some("The App"), Some("#12939a"))
    );
    let (status, _) = call(
        "PATCH",
        format!("/api/projects/{id}"),
        Some(r#"{"color":"red"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A worktree with a shell in it.
    let (status, wt) = call(
        "POST",
        format!("/api/projects/{id}/worktrees"),
        Some(r#"{"name":"Fix Login","agent":"shell"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{wt}");
    let path = wt["worktree"]["path"].as_str().unwrap().to_string();
    assert!(path.ends_with("worktrees/app/fix-login"), "{path}");
    assert_eq!(wt["worktree"]["branch"], "fix-login");
    assert_eq!(wt["session"]["cwd"], path);
    assert!(std::path::Path::new(&path).join("README.md").is_file());
    let (_, list) = call("GET", "/api/projects".into(), None).await;
    let trees = list[0]["worktrees"].as_array().unwrap();
    assert_eq!(trees.len(), 2);
    assert_eq!(
        (trees[0]["primary"].as_bool(), trees[0]["branch"].as_str()),
        (Some(true), Some("main"))
    );
    assert_eq!(
        (trees[1]["path"].as_str(), trees[1]["branch"].as_str()),
        (Some(path.as_str()), Some("fix-login"))
    );

    // Not while an agent works in it; then it goes, with its stopped sessions.
    let uri = format!("/api/projects/{id}/worktrees?path={path}");
    let (status, refused) = call("DELETE", uri.clone(), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("stop the agents"));
    let sid = wt["session"]["id"].as_str().unwrap();
    let (status, _) = call(
        "POST",
        format!("/api/sessions/{sid}/action"),
        Some(r#"{"action":"stop"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, removed) = call("DELETE", uri, None).await;
    assert_eq!(status, StatusCode::OK, "{removed}");
    assert_eq!(removed["sessions_removed"], 1);
    assert!(!std::path::Path::new(&path).exists());
    let (_, list) = call("GET", "/api/projects".into(), None).await;
    assert_eq!(list[0]["worktrees"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn photos_for_an_agent_are_saved_where_it_can_read_them() {
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
    let body = format!(
        r#"{{"name":"cam","kind":"shell","cwd":"{}"}}"#,
        tmp.path().display()
    );
    let (status, s) = call("POST", "/api/sessions".into(), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    let id = s["id"].as_str().unwrap();
    // "hello" in base64.
    let (status, saved) = call(
        "POST",
        format!("/api/sessions/{id}/files"),
        Some(r#"{"name":"IMG 1.jpg","data":"aGVsbG8="}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let path = saved["path"].as_str().unwrap();
    assert!(
        path.contains(&format!("uploads/{id}/")) && path.ends_with("-IMG-1.jpg"),
        "{path}"
    );
    assert_eq!(std::fs::read(path).unwrap(), b"hello");
    let (status, _) = call(
        "POST",
        format!("/api/sessions/{id}/files"),
        Some(r#"{"name":"x.jpg","data":"not base64!"}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(
        "POST",
        "/api/sessions/nobody/files".into(),
        Some(r#"{"name":"x.jpg","data":"aGVsbG8="}"#.into()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call("DELETE", format!("/api/sessions/{id}"), None).await;
    assert_eq!(status, StatusCode::OK);
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

#[tokio::test]
async fn daily_usage_reads_the_agents_logs() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    // A Claude conversation of today in a folder that is not a kimo project.
    let folder = tmp.path().join("home/oss/lib");
    std::fs::create_dir_all(folder.join(".git")).unwrap();
    std::fs::create_dir_all(folder.join("src")).unwrap();
    let logs = tmp.path().join("home/.claude/projects/-oss-lib");
    std::fs::create_dir_all(&logs).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (days, rem) = (now / 86_400, now % 86_400);
    let ts = {
        // Civil date from days since 1970 (UTC).
        let z = days as i64 + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    };
    let cwd = folder.join("src").display().to_string();
    std::fs::write(
        logs.join("c1.jsonl"),
        format!(
            "{{\"type\":\"user\",\"timestamp\":\"{ts}\",\"cwd\":\"{cwd}\",\"uuid\":\"u1\",\"message\":{{\"content\":\"Fix the parser\"}}}}\n\
             {{\"type\":\"assistant\",\"timestamp\":\"{ts}\",\"cwd\":\"{cwd}\",\"message\":{{\"id\":\"m1\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":90,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":500}}}}}}\n"
        ),
    )
    .unwrap();

    let res = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/usage/daily?days=3",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let r = json(res).await;
    assert_eq!(r["days"].as_array().unwrap().len(), 3);
    assert_eq!(
        (r["total"]["claude"].as_u64(), r["prompts"].as_u64()),
        (Some(100), Some(1))
    );
    // Grouped at the repository root, not the subfolder it ran in; not run through kimo.
    assert_eq!(r["places"][0]["name"], "lib");
    assert!(r["places"][0]["project"].is_null());
    assert_eq!(r["outside_kimo"]["claude"].as_u64(), Some(100));

    // Apps with a read key may see usage too.
    let key = json(
        app.clone()
            .oneshot(local(req(
                "POST",
                "/api/clients",
                None,
                None,
                Some(&format!(
                    r#"{{"name":"pm","origins":["{APP}"],"scope":"read"}}"#
                )),
            )))
            .await
            .unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let res = app
        .oneshot(remote(req(
            "GET",
            "/api/usage/daily",
            Some(APP),
            Some(&key),
            None,
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

/// The real binary starts and answers (startup code the in-process router never runs, such as
/// listing this machine's addresses: it once crashed on macOS only).
#[test]
fn kimo_serve_starts_and_answers() {
    let tmp = tempfile::tempdir().unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_kimo"))
        .args(["serve", "--port", &port.to_string()])
        .env("KIMO_DATA_DIR", tmp.path().join("kimo"))
        .env("KIMO_BIND", "localhost")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let answered = loop {
        if let Ok(Some(status)) = child.try_wait() {
            let mut err = String::new();
            std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut err).unwrap();
            panic!("kimo serve exited ({status}): {err}");
        }
        if let Ok(mut s) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            use std::io::{Read, Write};
            s.set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let _ = write!(
                s,
                "GET /api/status HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            );
            let mut response = String::new();
            let _ = s.read_to_string(&mut response);
            if response.starts_with("HTTP/1.1 200") {
                break true;
            }
        }
        if std::time::Instant::now() > deadline {
            break false;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    let _ = child.kill();
    let _ = child.wait();
    assert!(answered, "kimo serve did not answer /api/status");
}

#[tokio::test]
async fn the_active_subscription_is_where_new_agents_start() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let call = |method: &'static str, uri: &'static str, body: Option<String>| {
        let app = app.clone();
        async move {
            app.oneshot(local(req(method, uri, None, None, body.as_deref())))
                .await
                .unwrap()
        }
    };
    let work = json(
        call(
            "POST",
            "/api/accounts",
            Some(r#"{"name":"Work","provider":"claude"}"#.into()),
        )
        .await,
    )
    .await;
    let id = work["id"].as_str().unwrap().to_string();
    let set = call(
        "POST",
        "/api/accounts/active",
        Some(format!(r#"{{"provider":"claude","account":"{id}"}}"#)),
    )
    .await;
    assert_eq!(set.status(), StatusCode::OK);
    let accounts = json(call("GET", "/api/accounts", None).await).await;
    assert_eq!(accounts[0]["active"], true, "{accounts}");
    assert_eq!(accounts[0]["system"], false);

    // No account named: the active one; "system": this computer's own login.
    let cwd = tmp.path().join("home").display().to_string();
    let new = |name: &str, account: &str| {
        format!(r#"{{"name":"{name}","kind":"claude","cwd":"{cwd}","start":false{account}}}"#)
    };
    let a = json(call("POST", "/api/sessions", Some(new("a", ""))).await).await;
    assert_eq!(a["account"], id.as_str());
    let b = json(
        call(
            "POST",
            "/api/sessions",
            Some(new("b", r#","account":"system""#)),
        )
        .await,
    )
    .await;
    assert!(b["account"].is_null(), "{b}");

    // A Claude account is not a Codex subscription.
    let wrong = call(
        "POST",
        "/api/accounts/active",
        Some(format!(r#"{{"provider":"codex","account":"{id}"}}"#)),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);

    // Back to this computer's login, moving the agents: both are on it now.
    let moved = json(
        call(
            "POST",
            "/api/accounts/active",
            Some(r#"{"provider":"claude","account":null,"move":true}"#.into()),
        )
        .await,
    )
    .await;
    assert_eq!(moved["moved"], 1);
    let sessions = json(call("GET", "/api/sessions", None).await).await;
    assert!(sessions
        .as_array()
        .unwrap()
        .iter()
        .all(|s| s["account"].is_null()));
}

#[tokio::test]
async fn typed_pairing_codes_and_other_kimos_are_for_the_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), Some("admin-secret"));
    let pairing = json(
        app.clone()
            .oneshot(local(req("POST", "/api/pairing", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    // Typed from the screen: small letters and a space instead of the dash still work.
    let code = pairing["code"].as_str().unwrap();
    assert_eq!(code.len(), 9, "{code}");
    let typed = code.to_lowercase().replace('-', " ");
    let body = format!(r#"{{"code":"{typed}","name":"iPhone"}}"#);
    let paired = app
        .clone()
        .oneshot(remote(req("POST", "/api/pair", None, None, Some(&body))))
        .await
        .unwrap();
    assert_eq!(paired.status(), StatusCode::OK);

    // Paired kimos reached through this one: owner only, unknown ones are not found.
    let missing = app
        .clone()
        .oneshot(local(req("GET", "/api/hosts/nuc/status", None, None, None)))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let key = json(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/clients",
                None,
                Some("admin-secret"),
                Some(r#"{"name":"tool","scope":"control"}"#),
            ))
            .await
            .unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let denied = app
        .clone()
        .oneshot(remote(req(
            "GET",
            "/api/hosts/nuc/status",
            None,
            Some(&key),
            None,
        )))
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn ports_and_keeping_awake_are_for_the_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), Some("admin-secret"));
    let ports = app
        .clone()
        .oneshot(local(req("GET", "/api/ports", None, None, None)))
        .await
        .unwrap();
    assert_eq!(ports.status(), StatusCode::OK);
    assert!(json(ports).await.is_array());

    // While agents work: nothing works here, so nothing is held awake.
    let set = json(
        app.clone()
            .oneshot(local(req(
                "POST",
                "/api/awake",
                None,
                None,
                Some(r#"{"mode":"agents"}"#),
            )))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (set["mode"].as_str(), set["active"].as_bool()),
        (Some("agents"), Some(false))
    );
    let status = json(
        app.clone()
            .oneshot(local(req("GET", "/api/status", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status["awake"]["mode"], "agents");
    let settings = json(
        app.clone()
            .oneshot(local(req("GET", "/api/settings", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(settings["keep_awake"], "agents");

    let key = json(
        app.clone()
            .oneshot(req(
                "POST",
                "/api/clients",
                None,
                Some("admin-secret"),
                Some(r#"{"name":"tool","scope":"control"}"#),
            ))
            .await
            .unwrap(),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    for (method, path, body) in [
        ("GET", "/api/ports", None),
        ("POST", "/api/awake", Some(r#"{"mode":"on"}"#)),
    ] {
        let denied = app
            .clone()
            .oneshot(remote(req(method, path, None, Some(&key), body)))
            .await
            .unwrap();
        assert_eq!(denied.status(), StatusCode::FORBIDDEN, "{path}");
    }
}

#[tokio::test]
async fn terminals_belong_to_projects_and_go_with_them() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let site = tmp.path().join("home/site");
    std::fs::create_dir_all(site.join("web")).unwrap();
    let new_session = |name: &str, cwd: &std::path::Path| {
        format!(
            r#"{{"name":"{name}","kind":"shell","cwd":"{}","start":false}}"#,
            cwd.display()
        )
    };
    let call = |method: &str, path: &str, body: Option<String>| {
        local(req(method, path, None, None, body.as_deref()))
    };
    // A terminal in a folder outside every project makes it a project; a subfolder joins it.
    for (name, cwd) in [("one", site.clone()), ("two", site.join("web"))] {
        let r = app
            .clone()
            .oneshot(call("POST", "/api/sessions", Some(new_session(name, &cwd))))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
    // The home folder itself does not become a project (it would hold all the others).
    let home = app
        .clone()
        .oneshot(call(
            "POST",
            "/api/sessions",
            Some(new_session("home", &tmp.path().join("home"))),
        ))
        .await
        .unwrap();
    assert_eq!(home.status(), StatusCode::OK);
    let projects = json(
        app.clone()
            .oneshot(call("GET", "/api/projects", None))
            .await
            .unwrap(),
    )
    .await;
    let projects = projects.as_array().unwrap();
    assert_eq!(projects.len(), 1, "{projects:?}");
    assert_eq!(projects[0]["name"], "site");
    assert_eq!(projects[0]["sessions"], 2);

    // Removing the project ends its terminals; the folder stays.
    let id = projects[0]["id"].as_str().unwrap().to_string();
    let removed = json(
        app.clone()
            .oneshot(call("DELETE", &format!("/api/projects/{id}"), None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(removed["sessions_removed"], 2);
    let sessions = json(
        app.clone()
            .oneshot(call("GET", "/api/sessions", None))
            .await
            .unwrap(),
    )
    .await;
    let names: Vec<&str> = sessions
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(names, vec!["home"]);
    assert!(site.join("web").is_dir());
}

/// A request from this machine with `Host`/`Origin` of `host` (no key).
fn from_this_machine(path: &str, host: &str, origin: bool) -> Request<Body> {
    let mut r = Request::builder().uri(path).header(header::HOST, host);
    if origin {
        r = r.header(header::ORIGIN, format!("http://{host}"));
    }
    local(r.body(Body::empty()).unwrap())
}

#[tokio::test]
async fn only_this_machine_s_own_names_are_the_owner() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    // A name pointed at this machine by someone else (DNS rebinding) is not the owner, with
    // or without a matching Origin.
    for (host, origin) in [
        ("attacker.example:7777", true),
        ("attacker.example:7777", false),
        ("localhost.attacker.example:7777", true),
    ] {
        let r = app
            .clone()
            .oneshot(from_this_machine("/api/settings", host, origin))
            .await
            .unwrap();
        assert_eq!(
            r.status(),
            StatusCode::UNAUTHORIZED,
            "{host} origin={origin}"
        );
    }
    // This machine's own names are.
    for host in [
        "localhost:7777",
        "127.0.0.1:7777",
        "[::1]:7777",
        "localhost",
        "test:7777",
        "TEST.local:7777",
    ] {
        let r = app
            .clone()
            .oneshot(from_this_machine("/api/settings", host, true))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{host}");
    }
}

#[tokio::test]
async fn apps_on_their_own_origin_can_rename_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), Some("admin-secret"));
    let board = "https://board.example.com";
    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/clients",
            None,
            Some("admin-secret"),
            Some(&format!(
                r#"{{"name":"board","origins":["{board}"],"scope":"control"}}"#
            )),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let mut pre = req("OPTIONS", "/api/projects/demo", Some(board), None, None);
    pre.headers_mut()
        .insert("access-control-request-method", "PATCH".parse().unwrap());
    let res = app.oneshot(remote(pre)).await.unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let methods = res.headers()["access-control-allow-methods"]
        .to_str()
        .unwrap();
    for m in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        assert!(methods.contains(m), "{methods}");
    }
}

#[tokio::test]
async fn agents_start_with_their_own_model_and_effort() {
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let work = tmp.path().join("home/work");
    std::fs::create_dir_all(&work).unwrap();
    let create = |body: String| local(req("POST", "/api/sessions", None, None, Some(&body)));
    let body = |model: &str, effort: &str| {
        format!(
            r#"{{"name":"a","kind":"claude","cwd":"{}","start":false,"model":"{model}","effort":"{effort}"}}"#,
            work.display()
        )
    };
    let made = json(
        app.clone()
            .oneshot(create(body("opus", "High")))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        (made["model"].as_str(), made["effort"].as_str()),
        (Some("opus"), Some("high")),
        "{made}"
    );
    for (model, effort) in [("opus", "minimal"), ("--yolo", "high"), ("two words", "")] {
        let r = app
            .clone()
            .oneshot(create(body(model, effort)))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{model} {effort}");
    }
    // Clients learn what to offer.
    let agents = json(
        app.oneshot(local(req("GET", "/api/agents", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    let claude = agents
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == "claude")
        .unwrap();
    assert!(claude["efforts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e == "max"));
    assert!(claude["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m == "opus"));
}

#[tokio::test]
async fn project_files_and_git_through_the_api() {
    // CI machines have no git identity: commits here are made as "kimo test".
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "kimo test"),
        ("GIT_AUTHOR_EMAIL", "test@kimo.invalid"),
        ("GIT_COMMITTER_NAME", "kimo test"),
        ("GIT_COMMITTER_EMAIL", "test@kimo.invalid"),
    ] {
        std::env::set_var(k, v);
    }
    let tmp = tempfile::tempdir().unwrap();
    let app = router(tmp.path(), None);
    let site = tmp.path().join("home/site");
    std::fs::create_dir_all(site.join("src")).unwrap();
    std::fs::write(site.join("README.md"), "hello\n").unwrap();
    std::fs::write(site.join("src/app.js"), "let answer = 42;\n").unwrap();
    std::fs::write(tmp.path().join("secret.txt"), "outside").unwrap();
    std::os::unix::fs::symlink(tmp.path().join("secret.txt"), site.join("link.txt")).unwrap();
    for args in [vec!["init", "-q"], vec!["add", "-A"]] {
        assert!(std::process::Command::new("git")
            .args(&args)
            .current_dir(&site)
            .status()
            .unwrap()
            .success());
    }
    let call = |method: &str, path: &str, body: Option<String>| {
        local(req(method, path, None, None, body.as_deref()))
    };
    let get = |path: String| {
        let app = app.clone();
        async move { json(app.oneshot(call("GET", &path, None)).await.unwrap()).await }
    };
    let created = json(
        app.clone()
            .oneshot(call(
                "POST",
                "/api/projects",
                Some(format!(
                    r#"{{"mode":"existing","path":"{}","resume":false}}"#,
                    site.display()
                )),
            ))
            .await
            .unwrap(),
    )
    .await;
    let id = created["project"]["id"].as_str().unwrap().to_string();

    // Folders first, then files; .git hidden.
    let list = get(format!("/api/projects/{id}/files")).await;
    let names: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(names, ["src", "link.txt", "README.md"], "{list}");
    let found = get(format!("/api/projects/{id}/find?q=app")).await;
    assert_eq!(found[0]["path"], "src/app.js");
    let hits = get(format!("/api/projects/{id}/find?q=ANSWER&contents=1")).await;
    assert_eq!(
        (hits[0]["path"].as_str(), hits[0]["line"].as_u64()),
        (Some("src/app.js"), Some(1))
    );

    // Read, save, and refuse to overwrite a newer copy.
    let file = get(format!("/api/projects/{id}/file?path=src/app.js")).await;
    assert_eq!(file["text"], "let answer = 42;\n");
    let modified = file["modified"].as_u64().unwrap();
    let save = |text: &str, modified: u64| {
        call(
            "PUT",
            &format!("/api/projects/{id}/file"),
            Some(
                serde_json::json!({ "path": "src/app.js", "text": text, "modified": modified })
                    .to_string(),
            ),
        )
    };
    let saved = app
        .clone()
        .oneshot(save("let answer = 43;\n", modified))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let saved = json(saved).await;
    assert_eq!(
        std::fs::read_to_string(site.join("src/app.js")).unwrap(),
        "let answer = 43;\n"
    );
    let stale = app
        .clone()
        .oneshot(save("old view", modified - 5000))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert!(saved["modified"].as_u64().unwrap() >= modified);

    // Nothing outside the project, by path or through a symlink.
    for path in ["../secret.txt", "/etc/hosts", "link.txt"] {
        let r = app
            .clone()
            .oneshot(call(
                "GET",
                &format!("/api/projects/{id}/file?path={path}"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{path}");
    }
    let escape = app
        .clone()
        .oneshot(call(
            "PUT",
            &format!("/api/projects/{id}/file"),
            Some(serde_json::json!({ "path": "../evil.txt", "text": "x" }).to_string()),
        ))
        .await
        .unwrap();
    assert_eq!(escape.status(), StatusCode::BAD_REQUEST);
    assert!(!tmp.path().join("home/evil.txt").exists());

    // git: changes, a diff, commit, log.
    let state = get(format!("/api/projects/{id}/git")).await;
    assert_eq!(state["repo"], true);
    let app_js = state["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["path"] == "src/app.js")
        .unwrap()
        .clone();
    assert_eq!(
        (
            app_js["letter"].as_str(),
            app_js["staged"].as_bool(),
            app_js["unstaged"].as_bool()
        ),
        (Some("A"), Some(true), Some(true))
    );
    let diff = get(format!("/api/projects/{id}/git/diff?path=src/app.js")).await;
    assert!(
        diff["diff"].as_str().unwrap().contains("+let answer = 43;"),
        "{diff}"
    );
    let staged = app
        .clone()
        .oneshot(call(
            "POST",
            &format!("/api/projects/{id}/git/stage"),
            Some("{}".into()),
        ))
        .await
        .unwrap();
    assert_eq!(staged.status(), StatusCode::OK);
    let commit = app
        .clone()
        .oneshot(call(
            "POST",
            &format!("/api/projects/{id}/git/commit"),
            Some(r#"{"message":"first"}"#.into()),
        ))
        .await
        .unwrap();
    let commit_status = commit.status();
    let commit = json(commit).await;
    assert_eq!(commit_status, StatusCode::OK, "{commit}");
    assert_eq!(commit["subject"], "first");
    let log = get(format!("/api/projects/{id}/git/log")).await;
    assert_eq!(log[0]["subject"], "first");
    let clean = get(format!("/api/projects/{id}/git")).await;
    assert_eq!(clean["changes"].as_array().unwrap().len(), 0, "{clean}");
}

/// The router and its manager (to stand in for the machine's own commands).
fn router_and_manager(tmp: &std::path::Path) -> (axum::Router, std::sync::Arc<Manager>) {
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
    let app = kimo::api::router(
        manager.clone(),
        kimo::api::Options {
            token: None,
            admin_origins: vec![],
            local_owner: true,
            bound: "0.0.0.0".parse().unwrap(),
            fleet: None,
        },
    );
    (app, manager)
}

#[tokio::test]
async fn a_fleet_coordinator_pairs_one_way_and_reaches_only_facts_jobs_and_power() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    let call = |r: Request<Body>| {
        let app = app.clone();
        async move { app.oneshot(r).await.unwrap() }
    };
    let code = json(call(local(req("POST", "/api/pairing", None, None, None))).await).await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let paired = call(remote(req(
        "POST",
        "/api/pair",
        None,
        None,
        Some(&format!(
            r#"{{"code":"{code}","name":"pi4 (fleet)","role":"fleet"}}"#
        )),
    )))
    .await;
    assert_eq!(paired.status(), StatusCode::OK);
    let fleet = json(paired).await;
    let key = fleet["token"].as_str().unwrap().to_string();
    assert!(key.starts_with("kimo_fleet_"));
    // Its key is listed with the devices, as a coordinator; it is not an owner key.
    let devices = json(call(local(req("GET", "/api/devices", None, None, None))).await).await;
    assert_eq!(devices[0]["role"], "fleet");
    let state = std::fs::read_to_string(tmp.path().join("kimo/state.json")).unwrap();
    let state: serde_json::Value = serde_json::from_str(&state).unwrap();
    assert!(state["devices"].as_array().unwrap().is_empty());
    assert_eq!(state["fleet"].as_array().unwrap().len(), 1);

    let with_key = |m: &str, p: &str, body: Option<&str>| remote(req(m, p, None, Some(&key), body));
    // The machine's facts and power: yes.
    let node = call(with_key("GET", "/api/node", None)).await;
    assert_eq!(node.status(), StatusCode::OK);
    let node = json(node).await;
    assert_eq!(node["host"], "test");
    assert!(node["cpus"].as_u64().unwrap() >= 1);
    assert!(node["labels"].as_array().unwrap().len() >= 2);
    assert!(node["sessions"]["total"].is_number());
    assert_eq!(
        call(with_key("GET", "/api/status", None)).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        call(with_key("GET", "/api/power", None)).await.status(),
        StatusCode::OK
    );
    // The owner's sessions, keys, files and other kimos: no.
    for (m, p) in [
        ("GET", "/api/sessions"),
        ("POST", "/api/sessions"),
        ("GET", "/api/events"),
        ("GET", "/api/devices"),
        ("POST", "/api/pairing"),
        ("GET", "/api/peers"),
        ("GET", "/api/accounts"),
        ("GET", "/api/settings"),
        ("GET", "/api/fs"),
        ("GET", "/api/hosts/nuc/sessions"),
    ] {
        assert_eq!(
            call(with_key(m, p, None)).await.status(),
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
    }
    // A coordinator is never a web page.
    let from_page = call(remote(req("GET", "/api/node", Some(APP), Some(&key), None))).await;
    assert_eq!(from_page.status(), StatusCode::FORBIDDEN);

    // Forgetting it on the Devices page revokes the key at once.
    let id = devices[0]["id"].as_str().unwrap().to_string();
    let gone = call(local(req(
        "DELETE",
        &format!("/api/devices/{id}"),
        None,
        None,
        None,
    )))
    .await;
    assert_eq!(gone.status(), StatusCode::OK);
    assert_eq!(
        call(with_key("GET", "/api/node", None)).await.status(),
        StatusCode::UNAUTHORIZED
    );
    drop(manager);
}

#[tokio::test]
async fn the_machine_switches_off_only_when_nothing_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    let marker = tmp.path().join("switched-off");
    manager.set_shutdown_command(&format!("touch '{}'", marker.display()));
    // People logged in to the test machine do not count here (that check has its own test).
    manager.set_shutdown_ignores_logins(true);
    let call = |r: Request<Body>| {
        let app = app.clone();
        async move { app.oneshot(r).await.unwrap() }
    };
    let code = json(call(local(req("POST", "/api/pairing", None, None, None))).await).await["code"]
        .as_str()
        .unwrap()
        .to_string();
    let key = json(
        call(remote(req(
            "POST",
            "/api/pair",
            None,
            None,
            Some(&format!(
                r#"{{"code":"{code}","name":"pi4","role":"fleet"}}"#
            )),
        )))
        .await,
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let off = |force: bool| {
        remote(req(
            "POST",
            "/api/power/shutdown",
            None,
            Some(&key),
            Some(&format!(r#"{{"force":{force}}}"#)),
        ))
    };

    // A running terminal keeps the machine on: the coordinator hears what.
    let project = tmp.path().join("home/dev/p");
    std::fs::create_dir_all(&project).unwrap();
    let body = serde_json::json!({ "kind": "shell", "name": "build", "cwd": project, "command": ["sleep", "30"] });
    let made = call(local(req(
        "POST",
        "/api/sessions",
        None,
        None,
        Some(&body.to_string()),
    )))
    .await;
    assert_eq!(made.status(), StatusCode::OK);
    let id = json(made).await["id"].as_str().unwrap().to_string();
    let power = json(call(remote(req("GET", "/api/power", None, Some(&key), None))).await).await;
    assert_eq!(power["can_shutdown"], false);
    assert_eq!(power["blockers"][0]["kind"], "session");
    let refused = call(off(false)).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let refused = json(refused).await;
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("build (running)"));
    // Only the owner may force it.
    assert_eq!(call(off(true)).await.status(), StatusCode::FORBIDDEN);

    // Once the terminal is stopped, it switches off (here: the stand-in command runs).
    let stop = call(local(req(
        "POST",
        &format!("/api/sessions/{id}/action"),
        None,
        None,
        Some(r#"{"action":"stop"}"#),
    )))
    .await;
    assert_eq!(stop.status(), StatusCode::OK);
    for _ in 0..50 {
        if manager.sessions()[0].status != kimo::core::models::SessionStatus::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let accepted = call(off(false)).await;
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    assert_eq!(json(accepted).await["shutting_down"], true);
    for _ in 0..50 {
        if marker.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(marker.exists(), "the shutdown command ran");
}

#[test]
fn people_logged_in_keep_the_machine_on() {
    let b = kimo::core::power::blockers(
        &[],
        &[],
        &kimo::core::node::parse_who("jordi    tty2   2026-10-28 10:00 (tty2)\n"),
    );
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].kind, "login");
}

/// Wait until the job has ended; returns it.
async fn job_ended(app: &axum::Router, id: &str) -> serde_json::Value {
    for _ in 0..150 {
        let job = json(
            app.clone()
                .oneshot(local(req(
                    "GET",
                    &format!("/api/jobs/{id}"),
                    None,
                    None,
                    None,
                )))
                .await
                .unwrap(),
        )
        .await;
        if !["starting", "running"].contains(&job["state"].as_str().unwrap_or("")) {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("job {id} did not end");
}

async fn new_job(app: &axum::Router, body: serde_json::Value) -> serde_json::Value {
    let res = app
        .clone()
        .oneshot(local(req(
            "POST",
            "/api/jobs",
            None,
            None,
            Some(&body.to_string()),
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    json(res).await
}

#[tokio::test]
async fn jobs_run_to_their_end_with_a_real_exit_code() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));

    let ok = new_job(
        &app,
        serde_json::json!({ "id": "hello", "command": "echo hi; echo oops >&2" }),
    )
    .await;
    assert_eq!(ok["id"], "hello");
    let ok = job_ended(&app, "hello").await;
    assert_eq!(ok["state"], "succeeded");
    assert_eq!(ok["exit_code"], 0);
    let log = json(
        app.clone()
            .oneshot(local(req("GET", "/api/jobs/hello/log", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    assert!(log["text"].as_str().unwrap().contains("hi"));
    assert!(log["text"].as_str().unwrap().contains("oops"));
    assert_eq!(log["done"], true);
    // From where it stopped: nothing new.
    let next = log["next"].as_u64().unwrap();
    let rest = json(
        app.clone()
            .oneshot(local(req(
                "GET",
                &format!("/api/jobs/hello/log?offset={next}"),
                None,
                None,
                None,
            )))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(rest["text"], "");

    let failed = new_job(
        &app,
        serde_json::json!({ "command": ["sh", "-c", "exit 3"] }),
    )
    .await;
    let failed = job_ended(&app, failed["id"].as_str().unwrap()).await;
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["exit_code"], 3);

    let missing = new_job(
        &app,
        serde_json::json!({ "command": ["no-such-program-kimo"] }),
    )
    .await;
    let missing = job_ended(&app, missing["id"].as_str().unwrap()).await;
    assert_eq!(missing["state"], "failed");
    assert_eq!(missing["exit_code"], 127);

    let late = new_job(
        &app,
        serde_json::json!({ "command": "sleep 30", "timeout_s": 1 }),
    )
    .await;
    let late = job_ended(&app, late["id"].as_str().unwrap()).await;
    assert_eq!(late["state"], "timed_out");

    // A running job keeps the machine on, and counts in its facts.
    manager.set_shutdown_ignores_logins(true);
    let long = new_job(
        &app,
        serde_json::json!({ "id": "long", "command": "sleep 30" }),
    )
    .await;
    assert!(["starting", "running"].contains(&long["state"].as_str().unwrap()));
    for _ in 0..50 {
        if manager.job("long").unwrap().state == kimo::core::jobs::JobState::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    let power = json(
        app.clone()
            .oneshot(local(req("GET", "/api/power", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(power["blockers"][0]["what"], "job long");
    let node = json(
        app.clone()
            .oneshot(local(req("GET", "/api/node", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(node["sessions"]["jobs_running"], 1);
    // Still running: it cannot be forgotten, only cancelled.
    let busy = app
        .clone()
        .oneshot(local(req("DELETE", "/api/jobs/long", None, None, None)))
        .await
        .unwrap();
    assert_eq!(busy.status(), StatusCode::BAD_REQUEST);
    let cancel = app
        .clone()
        .oneshot(local(req(
            "POST",
            "/api/jobs/long/cancel",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(cancel.status(), StatusCode::OK);
    let long = job_ended(&app, "long").await;
    assert_eq!(long["state"], "cancelled");
    let gone = app
        .clone()
        .oneshot(local(req("DELETE", "/api/jobs/long", None, None, None)))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::OK);
    let unknown = app
        .clone()
        .oneshot(local(req("GET", "/api/jobs/long", None, None, None)))
        .await
        .unwrap();
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // Bad requests: plain errors.
    for body in [
        serde_json::json!({ "id": "../x", "command": "true" }),
        serde_json::json!({}),
        serde_json::json!({ "command": "true", "cwd": "/no/such/folder" }),
        serde_json::json!({ "id": "hello", "command": "true" }),
    ] {
        let res = app
            .clone()
            .oneshot(local(req(
                "POST",
                "/api/jobs",
                None,
                None,
                Some(&body.to_string()),
            )))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
        assert!(json(res).await["error"].is_string());
    }
}

#[tokio::test]
async fn jobs_outlive_the_kimo_that_started_them() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));
    let marker = tmp.path().join("done");
    new_job(
        &app,
        serde_json::json!({ "id": "outlive", "command": format!("sleep 1; touch '{}'", marker.display()) }),
    )
    .await;
    // kimo goes away (an update, a crash) and another starts on the same data.
    drop(app);
    drop(manager);
    let (app, _manager) = router_and_manager(tmp.path());
    let job = job_ended(&app, "outlive").await;
    assert_eq!(job["state"], "succeeded");
    assert!(marker.exists());
}

#[tokio::test]
async fn headless_agent_jobs_end_with_the_agent_s_answer() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));
    // A stand-in `claude` that answers like `claude -p --output-format json`.
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\nprompt=$(cat)\necho \"args: $*\" >&2\nprintf '{\"type\":\"result\",\"result\":\"did: %s\",\"is_error\":false}' \"$prompt\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let job = new_job(
        &app,
        serde_json::json!({
            "id": "agent",
            "agent": { "kind": "claude", "prompt": "-- write the report", "model": "sonnet", "permissions": "yolo" },
            "env": { "PATH": path },
        }),
    )
    .await;
    assert_eq!(job["agent"], "claude");
    let job = job_ended(&app, "agent").await;
    assert_eq!(job["state"], "succeeded", "{job}");
    assert_eq!(job["result"]["result"], "did: -- write the report");
    let log = json(
        app.clone()
            .oneshot(local(req("GET", "/api/jobs/agent/log", None, None, None)))
            .await
            .unwrap(),
    )
    .await;
    let log = log["text"].as_str().unwrap();
    assert!(
        log.contains("-p --output-format json --dangerously-skip-permissions --model sonnet"),
        "{log}"
    );
}

#[tokio::test]
async fn jobs_need_control_and_fleet_keys_may_run_them() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));
    let read = manager
        .add_client("watcher", vec![], kimo::core::models::Scope::Read)
        .unwrap();
    let res = app
        .clone()
        .oneshot(remote(req(
            "POST",
            "/api/jobs",
            None,
            Some(&read.token),
            Some(r#"{"command":"true"}"#),
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = app
        .clone()
        .oneshot(remote(req(
            "GET",
            "/api/jobs",
            None,
            Some(&read.token),
            None,
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let code = manager.start_pairing().code;
    let fleet = manager
        .pair_as(&code, "pi4", kimo::services::devices::Role::Fleet)
        .unwrap();
    let res = app
        .clone()
        .oneshot(remote(req(
            "POST",
            "/api/jobs",
            None,
            Some(&fleet.token),
            Some(r#"{"id":"from-fleet","command":"true"}"#),
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    assert_eq!(job_ended(&app, "from-fleet").await["state"], "succeeded");
}

#[tokio::test]
async fn staged_jobs_get_their_input_files_first_and_give_their_outputs() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));
    let staged = new_job(
        &app,
        serde_json::json!({ "id": "bench", "command": "wc -c < in/model.onnx > out.txt; mkdir -p results; echo '{\"fps\":30}' > results/bench.json", "start": false }),
    )
    .await;
    assert_eq!(staged["state"], "staged");
    // Its own folder (no cwd given).
    assert!(staged["cwd"].as_str().unwrap().ends_with("jobs/bench/work"));
    let put = |path: &str, body: &'static str| {
        let mut r = Request::builder()
            .method("PUT")
            .uri(format!("/api/jobs/bench/files?path={path}"))
            .header(header::HOST, "localhost:7777")
            .body(Body::from(body))
            .unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 50000))));
        app.clone().oneshot(r)
    };
    let res = put("in/model.onnx", "12345").await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(json(res).await["bytes"], 5);
    for bad in ["../escape", "/etc/passwd", ""] {
        let res = put(bad, "x").await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{bad}");
    }
    let start = app
        .clone()
        .oneshot(local(req(
            "POST",
            "/api/jobs/bench/start",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::OK);
    assert_eq!(job_ended(&app, "bench").await["state"], "succeeded");
    // Too late for inputs now; outputs can be fetched.
    assert_eq!(
        put("in/late", "x").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let out = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/jobs/bench/files?path=results/bench.json",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(out.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(out.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(&bytes[..], b"{\"fps\":30}\n");
    let out = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/jobs/bench/files?path=out.txt",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(out.into_body(), 1 << 20)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&bytes).trim(), "5");
    let missing = app
        .clone()
        .oneshot(local(req(
            "GET",
            "/api/jobs/bench/files?path=nope",
            None,
            None,
            None,
        )))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn jobs_run_in_a_fresh_checkout_of_a_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let (app, manager) = router_and_manager(tmp.path());
    manager.set_job_runner(std::path::Path::new(env!("CARGO_BIN_EXE_kimo")));
    let repo = tmp.path().join("home/dev/seg");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("train.sh"), "echo trained > model.txt\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    let first = git(&["rev-parse", "HEAD"]);
    std::fs::write(repo.join("train.sh"), "echo second > model.txt\n").unwrap();
    git(&["commit", "-q", "-am", "second"]);
    // Uncommitted work in the repository never reaches a job.
    std::fs::write(repo.join("train.sh"), "echo dirty > model.txt\n").unwrap();

    // A worktree at the first commit, on a branch of its own.
    let job = new_job(
        &app,
        serde_json::json!({
            "id": "train",
            "command": "sh train.sh; git rev-parse --abbrev-ref HEAD > branch.txt",
            "git": { "repo": "~/dev/seg", "ref": first, "branch": "kimo/train" },
        }),
    )
    .await;
    assert_eq!(job["git"]["commit"], first.as_str());
    assert_eq!(job["git"]["worktree"], true);
    assert_eq!(job_ended(&app, "train").await["state"], "succeeded");
    let file = |id: &str, path: &str| {
        let app = app.clone();
        let uri = format!("/api/jobs/{id}/files?path={path}");
        async move {
            let res = app
                .oneshot(local(req("GET", &uri, None, None, None)))
                .await
                .unwrap();
            let b = axum::body::to_bytes(res.into_body(), 1 << 20)
                .await
                .unwrap();
            String::from_utf8_lossy(&b).trim().to_string()
        }
    };
    assert_eq!(file("train", "model.txt").await, "trained");
    assert_eq!(file("train", "branch.txt").await, "kimo/train");
    assert!(git(&["worktree", "list"]).contains("jobs/train/work"));
    // Forgetting the job removes its worktree; the branch stays for whoever pushes it.
    let gone = app
        .clone()
        .oneshot(local(req("DELETE", "/api/jobs/train", None, None, None)))
        .await
        .unwrap();
    assert_eq!(gone.status(), StatusCode::OK);
    assert!(!git(&["worktree", "list"]).contains("jobs/train/work"));
    assert!(git(&["branch", "--list", "kimo/train"]).contains("kimo/train"));

    // A clone of a URL, at its HEAD (the second commit).
    let url = format!("file://{}", repo.display());
    new_job(
        &app,
        serde_json::json!({ "id": "cloned", "command": "sh train.sh", "git": { "repo": url } }),
    )
    .await;
    assert_eq!(job_ended(&app, "cloned").await["state"], "succeeded");
    assert_eq!(file("cloned", "model.txt").await, "second");

    // Mistakes: plain errors, and no folder left behind.
    for body in [
        serde_json::json!({ "command": "true", "git": { "repo": "~/dev/none" } }),
        serde_json::json!({ "command": "true", "git": { "repo": "~/dev/seg", "ref": "no-such-ref" } }),
        serde_json::json!({ "command": "true", "git": { "repo": "~/dev/seg", "branch": "a b" } }),
        serde_json::json!({ "command": "true", "cwd": "~", "git": { "repo": "~/dev/seg" } }),
    ] {
        let res = app
            .clone()
            .oneshot(local(req(
                "POST",
                "/api/jobs",
                None,
                None,
                Some(&body.to_string()),
            )))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "{body}");
    }
    assert_eq!(manager.jobs().len(), 1);
}
