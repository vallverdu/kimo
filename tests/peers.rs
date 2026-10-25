//! Two kimo servers paired with each other: agents listed across hosts, messages both ways.

use kimo::client::Client;
use kimo::services::sessions::{Manager, ManagerConfig};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// A kimo on a random port that only accepts its owner token or paired keys.
async fn start(name: &str, root: &Path) -> (Arc<Manager>, String, Client) {
    std::fs::create_dir_all(root.join("home")).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let manager = Manager::new(ManagerConfig {
        data_dir: root.join("kimo"),
        user_home: root.join("home"),
        shell: "/bin/sh".into(),
        scrollback_bytes: 64 * 1024,
        projects_dir: root.join("home/dev"),
        port,
        host_name: name.into(),
    })
    .unwrap();
    let admin = format!("{name}-admin");
    let router = kimo::api::router(
        manager.clone(),
        kimo::api::Options {
            token: Some(admin.clone()),
            admin_origins: vec![],
            local_owner: false,
            bound: "127.0.0.1".parse().unwrap(),
        },
    );
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let url = format!("http://127.0.0.1:{port}");
    (manager, url.clone(), Client::new(&url, Some(admin)))
}

/// Run blocking client calls off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

async fn screen_contains(client: &Client, id: &str, needle: &str) {
    for _ in 0..50 {
        let (c, id2) = (client.clone(), id.to_string());
        let v: Value = blocking(move || {
            c.get(&format!(
                "/sessions/{}/screen?lines=50",
                id2.replace('/', "%2F")
            ))
            .unwrap()
        })
        .await;
        if v["text"].as_str().unwrap_or("").contains(needle) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{needle:?} never appeared on {id}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paired_kimos_share_agents_and_messages() {
    let (ta, tb) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_a, a_url, a) = start("mini", ta.path()).await;
    let (_b, b_url, b) = start("lab", tb.path()).await;

    // Pair from mini with a code shown on lab.
    let (b2, a2, b_url2, a_url2) = (b.clone(), a.clone(), b_url.clone(), a_url.clone());
    let peer = blocking(move || {
        let code = b2.post("/pairing", json!({})).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string();
        a2.post(
            "/peers",
            json!({ "url": b_url2, "code": code, "my_url": a_url2 }),
        )
        .unwrap()
    })
    .await;
    assert_eq!(peer["id"], "lab");
    // Both sides know each other.
    let (b2, a_url2) = (b.clone(), a_url.clone());
    let back: Value = blocking(move || b2.get("/peers/keys").unwrap()).await;
    assert_eq!(
        (back[0]["id"].as_str(), back[0]["url"].as_str()),
        (Some("mini"), Some(a_url2.as_str()))
    );
    // A key of another kimo is not an owner key here, and wrong keys are refused.
    let (b_url2,) = (b_url.clone(),);
    assert!(
        blocking(move || Client::new(&b_url2, Some("nope".into())).get::<Value>("/sessions"))
            .await
            .is_err()
    );

    // An agent on each host.
    let shell = |name: &str, cmd: &str| json!({ "name": name, "kind": "shell", "cwd": "~", "command": [cmd] });
    let (a2, b2) = (a.clone(), b.clone());
    let (alice, bob) = blocking(move || {
        let alice = a2.post("/sessions", shell("alice", "cat")).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        let bob = b2.post("/sessions", shell("bob", "cat")).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        (alice, bob)
    })
    .await;

    // mini lists lab's agents as lab/<id>.
    let a2 = a.clone();
    let all: Value = blocking(move || a2.get("/sessions?all=1").unwrap()).await;
    let remote_bob = format!("lab/{bob}");
    let listed = all
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == remote_bob.as_str())
        .expect("lab's agent listed on mini");
    assert_eq!(
        (listed["host"].as_str(), listed["local"].as_bool()),
        (Some("lab"), Some(false))
    );
    assert!(all
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s["id"] == alice.as_str() && s["host"] == "mini"));

    // alice (on mini) → bob (on lab): signed with the address that leads back to her.
    let (a2, rb, al) = (a.clone(), remote_bob.clone(), alice.clone());
    let sent = blocking(move || {
        a2.post(
            &format!("/sessions/{}/message", rb.replace('/', "%2F")),
            json!({ "text": "review the parser", "from": al }),
        )
        .unwrap()
    })
    .await;
    assert_eq!(sent["to"], remote_bob.as_str());
    screen_contains(&b, &bob, &format!("[kimo message from agent mini/{alice} ")).await;
    // And read through mini.
    screen_contains(&a, &remote_bob, "review the parser").await;

    // bob answers with the address in the message: it reaches alice on mini.
    let (b2, al, bo) = (b.clone(), alice.clone(), bob.clone());
    blocking(move || {
        b2.post(
            &format!("/sessions/mini%2F{al}/message"),
            json!({ "text": "parser looks good", "from": bo }),
        )
        .unwrap()
    })
    .await;
    screen_contains(&a, &alice, &format!("[kimo message from agent lab/{bob} ")).await;
    screen_contains(&a, &alice, "parser looks good").await;
}
