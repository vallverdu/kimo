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
