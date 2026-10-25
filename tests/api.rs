//! Access control as other PWAs see it: CORS, app keys bound to origins, scopes, events.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use futures_util::StreamExt;
use kimo::services::sessions::{Manager, ManagerConfig};
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
    })
    .unwrap();
    kimo::api::router(manager, admin_token.map(String::from), vec![])
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
        .oneshot(req("GET", "/api/sessions", None, None, None))
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::OK);
    let same = app
        .clone()
        .oneshot(req(
            "GET",
            "/api/sessions",
            Some("http://kimo.local:7777"),
            None,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(same.status(), StatusCode::OK);
    let evil = app
        .clone()
        .oneshot(req("GET", "/api/sessions/x/term", Some(EVIL), None, None))
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
                Some(r#"{"name":"Everpano","origins":["https://everpano.tail.ts.net"]}"#),
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
            Some("https://everpano.tail.ts.net"),
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
            Some("https://everpano.tail.ts.net"),
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
        .oneshot(req("GET", "/api/events", None, None, None))
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
