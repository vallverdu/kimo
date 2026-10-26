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
    let (manager, url, client, _server) = start_server(name, root).await;
    (manager, url, client)
}

/// The same, with the server task (aborting it takes this kimo offline).
async fn start_server(
    name: &str,
    root: &Path,
) -> (Arc<Manager>, String, Client, tokio::task::JoinHandle<()>) {
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
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let url = format!("http://127.0.0.1:{port}");
    (manager, url.clone(), Client::new(&url, Some(admin)), server)
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
    screen_contains(
        &b,
        &bob,
        &format!("[kimo message from agent alice (id mini/{alice})"),
    )
    .await;
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
    screen_contains(
        &a,
        &alice,
        &format!("[kimo message from agent bob (id lab/{bob})"),
    )
    .await;
    screen_contains(&a, &alice, "parser looks good").await;
}

/// Pair `from` with `to` (a code shown on `to`); returns the peer as `from` records it.
async fn pair(from: &Client, from_url: &str, to: &Client, to_url: &str) -> Value {
    let (from, to, from_url, to_url) = (
        from.clone(),
        to.clone(),
        from_url.to_string(),
        to_url.to_string(),
    );
    blocking(move || {
        let code = to.post("/pairing", json!({})).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string();
        from.post(
            "/peers",
            json!({ "url": to_url, "code": code, "my_url": from_url }),
        )
        .unwrap()
    })
    .await
}

async fn works(url: &str, key: &str) -> bool {
    let c = Client::new(url, Some(key.to_string()));
    blocking(move || c.get::<Value>("/settings").is_ok()).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgetting_a_kimo_revokes_its_keys_both_ways() {
    let (ta, tb) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_a, a_url, a) = start("mini", ta.path()).await;
    let (_b, b_url, b) = start("lab", tb.path()).await;
    pair(&a, &a_url, &b, &b_url).await;
    // The keys each side holds for the other.
    let (a2, b2) = (a.clone(), b.clone());
    let a_key_on_b: Value = blocking(move || a2.get("/peers/keys").unwrap()).await;
    let b_key_on_a: Value = blocking(move || b2.get("/peers/keys").unwrap()).await;
    let a_key_on_b = a_key_on_b[0]["token"].as_str().unwrap().to_string();
    let b_key_on_a = b_key_on_a[0]["token"].as_str().unwrap().to_string();
    assert!(works(&b_url, &a_key_on_b).await && works(&a_url, &b_key_on_a).await);

    // mini forgets lab: lab is asked to forget mini too; both keys stop working.
    let a2 = a.clone();
    let gone = blocking(move || a2.delete("/peers/lab").unwrap()).await;
    assert_eq!(gone["remote_forgotten"], true, "{gone}");
    assert!(
        !works(&b_url, &a_key_on_b).await,
        "mini's key on lab still works"
    );
    assert!(
        !works(&a_url, &b_key_on_a).await,
        "lab's key on mini still works"
    );
    let (a2, b2) = (a.clone(), b.clone());
    let (pa, pb): (Value, Value) =
        blocking(move || (a2.get("/peers").unwrap(), b2.get("/peers").unwrap())).await;
    assert_eq!(
        (pa.as_array().unwrap().len(), pb.as_array().unwrap().len()),
        (0, 0)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forgetting_an_offline_kimo_revokes_its_key_here_and_says_so() {
    let (ta, tb) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let (_a, a_url, a) = start("mini", ta.path()).await;
    let (_b, b_url, b, lab_server) = start_server("lab", tb.path()).await;
    pair(&a, &a_url, &b, &b_url).await;
    let b2 = b.clone();
    let lab_key_on_mini: Value = blocking(move || b2.get("/peers/keys").unwrap()).await;
    let lab_key_on_mini = lab_key_on_mini[0]["token"].as_str().unwrap().to_string();
    assert!(works(&a_url, &lab_key_on_mini).await);
    // lab goes offline; mini forgets it anyway.
    lab_server.abort();
    let _ = lab_server.await;
    let a2 = a.clone();
    let gone = blocking(move || a2.delete("/peers/lab").unwrap()).await;
    assert_eq!(gone["remote_forgotten"], false, "{gone}");
    assert!(
        gone["note"].as_str().unwrap().contains("forget mini"),
        "{gone}"
    );
    assert!(
        !works(&a_url, &lab_key_on_mini).await,
        "lab's key on mini still works"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_device_loses_its_open_terminal() {
    let t = tempfile::tempdir().unwrap();
    let (_m, url, owner) = start("mini", t.path()).await;
    // A phone pairs; a terminal runs cat.
    let (owner2, url2) = (owner.clone(), url.clone());
    let (phone_key, session) = blocking(move || {
        let code = owner2.post("/pairing", json!({})).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string();
        let paired = Client::new(&url2, None)
            .post("/pair", json!({ "code": code, "name": "phone" }))
            .unwrap();
        let session = owner2
            .post(
                "/sessions",
                json!({ "name": "cat", "kind": "shell", "cwd": "~", "command": ["cat"] }),
            )
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        (paired["token"].as_str().unwrap().to_string(), session)
    })
    .await;
    let ws_url = format!(
        "{}/api/sessions/{session}/term?token={phone_key}",
        url.replace("http://", "ws://")
    );
    let (owner2, session2) = (owner.clone(), session.clone());
    let outcome = blocking(move || {
        use tungstenite::Message;
        let (mut ws, _) = tungstenite::connect(&ws_url).unwrap();
        if let tungstenite::stream::MaybeTlsStream::Plain(s) = ws.get_mut() {
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        }
        ws.send(Message::Binary(b"before-revoke\n".to_vec().into()))
            .unwrap();
        // The phone's input reaches cat.
        let mut seen = String::new();
        while !seen.contains("before-revoke") {
            match ws.read() {
                Ok(Message::Binary(b)) => seen.push_str(&String::from_utf8_lossy(&b)),
                Ok(_) => {}
                Err(e) => panic!("no echo: {e}"),
            }
        }
        // The owner revokes the phone.
        let devices: Value = owner2.get("/devices").unwrap();
        let id = devices
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["name"] == "phone")
            .unwrap()["id"]
            .as_str()
            .unwrap()
            .to_string();
        owner2.delete(&format!("/devices/{id}")).unwrap();
        // Its open terminal closes, and later input never reaches the session.
        let mut closed = false;
        for _ in 0..50 {
            match ws.read() {
                Ok(Message::Close(_)) | Err(_) => {
                    closed = true;
                    break;
                }
                Ok(_) => {}
            }
        }
        let _ = ws.send(Message::Binary(b"after-revoke\n".to_vec().into()));
        std::thread::sleep(Duration::from_millis(300));
        let screen: Value = owner2
            .get(&format!("/sessions/{session2}/screen?lines=50"))
            .unwrap();
        (closed, screen["text"].as_str().unwrap_or("").to_string())
    })
    .await;
    assert!(outcome.0, "the revoked device's terminal stayed open");
    assert!(outcome.1.contains("before-revoke"), "{}", outcome.1);
    assert!(!outcome.1.contains("after-revoke"), "{}", outcome.1);
}
