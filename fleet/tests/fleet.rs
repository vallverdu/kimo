//! The coordinator with real kimos as its machines (in this process, on local ports): pairing
//! one way, waking a machine for a job (the magic packet caught here), running the job there,
//! switching it off when idle, and a workflow whose steps pass files and wait for approval.

use kimo::services::sessions::{Manager, ManagerConfig};
use kimo_fleet::services::fleet::{Config, Fleet};
use serde_json::{json, Value};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The kimo binary runs the jobs (`kimo job-run`): the workspace's, built if needed.
fn kimo_bin() -> PathBuf {
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../target"));
    let bin = target.join("debug/kimo");
    if !bin.exists() {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let ok = std::process::Command::new(cargo)
            .args([
                "build",
                "-p",
                "kimo",
                "--bin",
                "kimo",
                "--no-default-features",
            ])
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .status()
            .unwrap()
            .success();
        assert!(ok, "building kimo");
    }
    bin
}

/// A machine: a kimo with its own data, served on a port of its own (switched on and off).
struct Machine {
    manager: Arc<Manager>,
    port: u16,
    server: Option<tokio::task::JoinHandle<()>>,
    off_marker: PathBuf,
}

impl Machine {
    fn new(tmp: &Path, name: &str) -> Self {
        let home = tmp.join(name).join("home");
        std::fs::create_dir_all(&home).unwrap();
        let manager = Manager::new(ManagerConfig {
            data_dir: tmp.join(name).join("kimo"),
            user_home: home.clone(),
            shell: "/bin/sh".into(),
            scrollback_bytes: 1024,
            projects_dir: home.join("dev"),
            port: 0,
            host_name: name.into(),
        })
        .unwrap();
        manager.set_job_runner(&kimo_bin());
        manager.set_shutdown_ignores_logins(true);
        let off_marker = tmp.join(name).join("switched-off");
        manager.set_shutdown_command(&format!("touch '{}'", off_marker.display()));
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        Self {
            manager,
            port,
            server: None,
            off_marker,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    async fn on(&mut self) {
        let app = kimo::api::router(
            self.manager.clone(),
            kimo::api::Options {
                token: None,
                admin_origins: vec![],
                // Its owner is not this test: everything goes through keys.
                local_owner: false,
                bound: "127.0.0.1".parse().unwrap(),
                fleet: None,
            },
        );
        let listener = loop {
            match tokio::net::TcpListener::bind(("127.0.0.1", self.port)).await {
                Ok(l) => break l,
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        self.server = Some(tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        }));
    }

    async fn off(&mut self) {
        if let Some(s) = self.server.take() {
            s.abort();
            let _ = s.await;
        }
    }

    fn pairing_code(&self) -> String {
        self.manager.start_pairing().code
    }
}

fn fleet(tmp: &Path, registry: &str) -> Arc<Fleet> {
    let dir = tmp.join("fleet");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("devices.yaml"), registry).unwrap();
    let mut cfg = Config::new(dir, "pi4".into());
    cfg.look_every_ms = 300;
    cfg.look_fast_ms = 200;
    cfg.ping = false;
    Fleet::open(cfg).unwrap()
}

/// Turn the coordinator's loop until `done` (or fail after `secs`).
async fn until(fleet: &Arc<Fleet>, secs: u64, what: &str, done: impl Fn(&Fleet) -> bool) {
    let start = Instant::now();
    loop {
        let f = fleet.clone();
        tokio::task::spawn_blocking(move || f.tick()).await.unwrap();
        if done(fleet) {
            return;
        }
        if start.elapsed() > Duration::from_secs(secs) {
            let devices: Vec<Value> = fleet.devices().iter().map(|d| json!(d)).collect();
            panic!(
                "timed out waiting for {what}\njobs: {:#?}\ndevices: {devices:#?}\nevents: {:#?}",
                fleet.jobs(),
                fleet.events(None, None, 40)
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn state(fleet: &Fleet, id: &str) -> String {
    fleet.device(id).unwrap().state.to_string()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    tokio::task::spawn_blocking(f).await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_is_woken_for_a_job_and_switched_off_when_idle() {
    let tmp = tempfile::tempdir().unwrap();
    let mut nuc = Machine::new(tmp.path(), "nuc");
    let mut pi5 = Machine::new(tmp.path(), "pi5");
    let wol = UdpSocket::bind("127.0.0.1:0").unwrap();
    wol.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let registry = format!(
        "devices:
  nuc:
    kimo: {nuc}
    mac: \"1c:69:7a:aa:bb:cc\"
    broadcast: {wol}
    power: {{ wake: wol, sleep: shutdown, idle_minutes: 0.05, boot_deadline_s: 20 }}
    labels: [x86_64, linux, cuda]
    resources: {{ cpu_slots: 2 }}
    cost: 40
  pi5:
    kimo: {pi5}
    labels: [arm64, rpi]
    cost: 3
",
        nuc = nuc.url(),
        pi5 = pi5.url(),
        wol = wol.local_addr().unwrap(),
    );
    let fleet = fleet(tmp.path(), &registry);
    nuc.on().await;
    pi5.on().await;

    // Unpaired: its kimo answers but lets the coordinator see nothing.
    until(&fleet, 10, "unpaired machines seen", |f| {
        f.device("pi5")
            .unwrap()
            .error
            .is_some_and(|e| e.contains("not paired"))
    })
    .await;
    // One-way pairing with the codes the machines show.
    for m in [&nuc, &pi5] {
        let code = m.pairing_code();
        let f = fleet.clone();
        let id = m.manager.host_name().to_string();
        blocking(move || f.pair(&id, &code)).await.unwrap();
    }
    // The machines list the coordinator as a fleet key, and it is not an owner there.
    let listed = nuc.manager.devices();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].role, kimo::services::devices::Role::Fleet);
    assert_eq!(listed[0].name, "pi4 (fleet)");
    // Kept on until the job part (its idle time is 3 s here).
    let f = fleet.clone();
    blocking(move || {
        f.hold(
            "nuc",
            serde_json::from_value(json!({ "never": true })).unwrap(),
        )
    })
    .await
    .unwrap();
    until(&fleet, 10, "both machines ready", |f| {
        state(f, "pi5") == "ready" && state(f, "nuc") == "ready"
    })
    .await;
    let facts = fleet.device("nuc").unwrap().facts.unwrap();
    assert_eq!(facts["host"], "nuc");
    assert!(fleet
        .device("nuc")
        .unwrap()
        .labels
        .contains(&"cuda".to_string()));

    // The NUC goes off (switched off by hand): seen as off.
    nuc.off().await;
    until(&fleet, 10, "the NUC off", |f| state(f, "nuc") == "off").await;
    let f = fleet.clone();
    blocking(move || {
        f.hold(
            "nuc",
            serde_json::from_value(json!({ "never": false })).unwrap(),
        )
    })
    .await
    .unwrap();

    // A CUDA job: the NUC is the only machine for it, so it is woken.
    let job = fleet
        .submit(
            serde_yaml_ng::from_str(
                "task: build\nrun: echo built on $(hostname) > out.txt; echo done\noutputs: [out.txt]\nrequires: { labels: [cuda] }\n",
            )
            .unwrap(),
        )
        .unwrap();
    until(&fleet, 10, "the job waking the NUC", |f| {
        f.job(&job.id).unwrap().state == kimo_fleet::core::jobs::JobState::Waking
    })
    .await;
    let (n, buf) = blocking(move || {
        let mut buf = [0u8; 200];
        let (n, _) = wol.recv_from(&mut buf).unwrap();
        (n, buf)
    })
    .await;
    assert_eq!(n, 102, "a magic packet");
    assert_eq!(&buf[6..12], &[0x1c, 0x69, 0x7a, 0xaa, 0xbb, 0xcc]);
    assert_eq!(state(&fleet, "nuc"), "waking");

    // It boots: the job runs there and its output is kept by the coordinator.
    nuc.on().await;
    until(&fleet, 30, "the job done", |f| {
        f.job(&job.id).unwrap().state.finished()
    })
    .await;
    let done = fleet.job(&job.id).unwrap();
    assert_eq!(
        done.state,
        kimo_fleet::core::jobs::JobState::Succeeded,
        "{done:#?}"
    );
    assert_eq!(done.device.as_deref(), Some("nuc"));
    let art = &done.artifacts["out.txt"];
    let kept = std::fs::read_to_string(fleet.artifact_path(&art.sha256).unwrap()).unwrap();
    assert!(kept.starts_with("built on"));
    let f = fleet.clone();
    let id = job.id.clone();
    let log = blocking(move || f.job_log(&id, 0)).await.unwrap();
    assert!(log["text"].as_str().unwrap().contains("done"), "{log}");
    assert_eq!(log["done"], true);

    // Idle for its idle time (3 s here): switched off through its kimo, which checks first.
    until(&fleet, 20, "the NUC switching off", |f| {
        state(f, "nuc") == "draining"
    })
    .await;
    for _ in 0..50 {
        if nuc.off_marker.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(nuc.off_marker.exists(), "its kimo ran the shutdown command");
    nuc.off().await;
    until(&fleet, 10, "the NUC off again", |f| {
        state(f, "nuc") == "off"
    })
    .await;
    // The always-on board was never touched.
    assert!(!pi5.off_marker.exists());
    assert_eq!(state(&fleet, "pi5"), "ready");
    let events: Vec<String> = fleet
        .events(Some("nuc"), None, 100)
        .into_iter()
        .map(|e| e.detail)
        .collect();
    assert!(
        events.iter().any(|e| e.starts_with("magic packet sent")),
        "{events:?}"
    );
    assert!(
        events.iter().any(|e| e == "switching off (idle)"),
        "{events:?}"
    );
    assert!(events.iter().any(|e| e == "switched off"), "{events:?}");

    // While you hold it, it is not switched off; manual control means nothing automatic.
    nuc.on().await;
    until(&fleet, 10, "the NUC back", |f| state(f, "nuc") == "idle").await;
    let f = fleet.clone();
    let held = blocking(move || {
        f.hold(
            "nuc",
            serde_json::from_value(json!({ "manual": true })).unwrap(),
        )
    })
    .await
    .unwrap();
    assert_eq!(held.state, "manual");
    tokio::time::sleep(Duration::from_secs(4)).await;
    until(&fleet, 2, "still manual", |f| state(f, "nuc") == "manual").await;
    // Work of its own over there keeps it on once it is automatic again.
    let busy = nuc
        .manager
        .create_job(
            serde_json::from_value(json!({ "command": "sleep 20", "id": "local" })).unwrap(),
        )
        .unwrap();
    let f = fleet.clone();
    blocking(move || {
        f.hold(
            "nuc",
            serde_json::from_value(json!({ "manual": false })).unwrap(),
        )
    })
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_secs(4)).await;
    until(&fleet, 2, "kept on by its own job", |f| {
        state(f, "nuc") == "idle"
    })
    .await;
    // And a shutdown by hand is refused while work runs there.
    let f = fleet.clone();
    let refused = blocking(move || f.shutdown("nuc")).await.unwrap_err();
    assert!(format!("{refused:#}").contains("job local"), "{refused:#}");
    nuc.manager.cancel_job(&busy.id).unwrap();
    nuc.off().await;
    pi5.off().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_workflow_passes_files_between_machines_and_waits_for_approval() {
    let tmp = tempfile::tempdir().unwrap();
    let mut trainer = Machine::new(tmp.path(), "trainer");
    let mut jetson = Machine::new(tmp.path(), "jetson");
    let mut board = Machine::new(tmp.path(), "board");
    let registry = format!(
        "devices:
  trainer:
    kimo: {}
    labels: [cuda]
    resources: {{ gpu: 0, vram_gb: 12 }}
  jetson:
    kimo: {}
    labels: [jetson]
  board:
    kimo: {}
    labels: [rpi]
",
        trainer.url(),
        jetson.url(),
        board.url()
    );
    let fleet = fleet(tmp.path(), &registry);
    for m in [&mut trainer, &mut jetson, &mut board] {
        m.on().await;
        let code = m.pairing_code();
        let f = fleet.clone();
        let id = m.manager.host_name().to_string();
        blocking(move || f.pair(&id, &code)).await.unwrap();
    }
    until(&fleet, 10, "machines ready", |f| {
        ["trainer", "jetson", "board"]
            .iter()
            .all(|d| state(f, d) == "ready")
    })
    .await;
    // A stand-in `claude` on the board answers like `claude -p --output-format json`.
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(
        bin.join("claude"),
        "#!/bin/sh\nprompt=$(cat)\nprintf '{\"type\":\"result\",\"result\":\"recommend: ship (%s)\"}' \"$prompt\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let flaky = tmp.path().join("flaky-once");
    let workflow = format!(
        r#"
workflow: model-to-release
steps:
  - task: train
    requires: {{ labels: [cuda], vram_gb: 10 }}
    run: mkdir -p out && echo weights > out/model.onnx
    outputs: [out/model.onnx]
  - task: bench
    needs: [train]
    requires: {{ labels: [jetson] }}
    inputs: {{ model: train/out/model.onnx }}
    run: (cat {{{{inputs.model}}}}; echo "fps=30 from $KIMO_INPUT_MODEL") > results.txt
    outputs: [results.txt]
  - task: retried
    needs: [train]
    requires: {{ labels: [jetson] }}
    run: test -f '{flaky}' || {{ touch '{flaky}'; exit 1; }}
    retries: 1
  - task: optional-bench
    needs: [train]
    requires: {{ labels: [rpi] }}
    run: exit 3
    optional: true
  - task: analyse
    runner: agent
    needs: [bench, optional-bench, retried]
    requires: {{ labels: [rpi] }}
    prompt: compare the results
    env: {{ PATH: "{bin}:/usr/bin:/bin" }}
  - task: approve-release
    runner: approval
    needs: [analyse]
    show: [results.txt]
"#,
        flaky = flaky.display(),
        bin = bin.display()
    );
    let run = fleet
        .submit_workflow(serde_yaml_ng::from_str(&workflow).unwrap())
        .unwrap();
    let step = |f: &Fleet, task: &str| {
        let (_, jobs) = f.workflow(&run.id).unwrap();
        jobs.into_iter().find(|j| j.spec.task == task).unwrap()
    };
    until(&fleet, 60, "the approval step waiting", |f| {
        step(f, "approve-release").state == kimo_fleet::core::jobs::JobState::AwaitingApproval
    })
    .await;
    use kimo_fleet::core::jobs::JobState;
    assert_eq!(step(&fleet, "train").device.as_deref(), Some("trainer"));
    let bench = step(&fleet, "bench");
    assert_eq!(bench.device.as_deref(), Some("jetson"));
    let results = std::fs::read_to_string(
        fleet
            .artifact_path(&bench.artifacts["results.txt"].sha256)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(results, "weights\nfps=30 from out/model.onnx\n");
    // Failed once, retried on the same machine, then fine.
    let retried = step(&fleet, "retried");
    assert_eq!(retried.state, JobState::Succeeded);
    assert_eq!(retried.attempt, 2);
    // Optional: its failure does not stop the workflow.
    assert_eq!(step(&fleet, "optional-bench").state, JobState::Failed);
    let analyse = step(&fleet, "analyse");
    assert_eq!(analyse.state, JobState::Succeeded, "{analyse:#?}");
    assert_eq!(analyse.device.as_deref(), Some("board"));
    assert_eq!(
        analyse.result.as_ref().unwrap()["result"],
        "recommend: ship (compare the results)"
    );
    // You approve: the workflow is done.
    let approval = step(&fleet, "approve-release");
    let f = fleet.clone();
    blocking(move || f.approve(&approval.id, true))
        .await
        .unwrap();
    let (run, _) = fleet.workflow(&run.id).unwrap();
    assert_eq!(run.state, kimo_fleet::services::fleet::RunState::Succeeded);

    // A workflow whose first step fails skips what needs it.
    let broken = fleet
        .submit_workflow(
            serde_yaml_ng::from_str(
                "workflow: broken\nsteps:\n  - task: a\n    run: exit 2\n    requires: { labels: [rpi] }\n  - task: b\n    needs: [a]\n    run: echo never\n",
            )
            .unwrap(),
        )
        .unwrap();
    until(&fleet, 30, "the broken workflow ended", |f| {
        f.workflow(&broken.id).unwrap().0.state != kimo_fleet::services::fleet::RunState::Running
    })
    .await;
    let (b, jobs) = fleet.workflow(&broken.id).unwrap();
    assert_eq!(b.state, kimo_fleet::services::fleet::RunState::Failed);
    assert_eq!(jobs[1].state, JobState::Skipped);
    assert_eq!(jobs[0].exit_code, Some(2));

    // A job no machine can run waits, and says why.
    let mac_job = fleet
        .submit(
            serde_yaml_ng::from_str("task: ios\nrun: xcodebuild\nrequires: { labels: [xcode] }\n")
                .unwrap(),
        )
        .unwrap();
    until(&fleet, 10, "the iOS job waiting", |f| {
        f.job(&mac_job.id).unwrap().state == JobState::WaitingDevice
    })
    .await;
    assert_eq!(
        fleet.job(&mac_job.id).unwrap().reason.as_deref(),
        Some("no machine has labels xcode")
    );
    let f = fleet.clone();
    let id = mac_job.id.clone();
    assert_eq!(
        blocking(move || f.cancel(&id)).await.unwrap().state,
        JobState::Cancelled
    );
    for m in [&mut trainer, &mut jetson, &mut board] {
        m.off().await;
    }
}

#[tokio::test]
async fn only_the_owner_reaches_the_coordinator_and_kimo_passes_its_calls_through() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;
    let tmp = tempfile::tempdir().unwrap();
    let fleet = fleet(tmp.path(), "devices: {}\n");
    let app = kimo_fleet::api::router(fleet, Some("script-token".into()));
    let call = |from: [u8; 4], origin: Option<&str>, token: Option<&str>| {
        let mut r = Request::builder()
            .uri("/api/devices")
            .header(header::HOST, "127.0.0.1:7780");
        if let Some(o) = origin {
            r = r.header(header::ORIGIN, o);
        }
        if let Some(t) = token {
            r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        let mut r = r.body(Body::empty()).unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from((from, 50000))));
        app.clone().oneshot(r)
    };
    assert_eq!(
        call([127, 0, 0, 1], None, None).await.unwrap().status(),
        StatusCode::OK
    );
    // A web page, another machine: no.
    assert_eq!(
        call([127, 0, 0, 1], Some("https://evil.example"), None)
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call([203, 0, 113, 9], None, None).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call([203, 0, 113, 9], None, Some("script-token"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_phone_reaches_the_fleet_through_its_kimo() {
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;
    let tmp = tempfile::tempdir().unwrap();
    let fleet = fleet(
        tmp.path(),
        "devices:\n  nuc:\n    kimo: http://127.0.0.1:9\n",
    );
    let api = kimo_fleet::api::router(fleet, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(
            listener,
            api.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap()
    });
    // The Pi's kimo, with the coordinator next to it.
    let pi = Machine::new(tmp.path(), "pi4");
    let kimo_app = kimo::api::router(
        pi.manager.clone(),
        kimo::api::Options {
            token: None,
            admin_origins: vec![],
            local_owner: true,
            bound: "0.0.0.0".parse().unwrap(),
            fleet: Some((format!("http://127.0.0.1:{port}"), None)),
        },
    );
    let code = pi.pairing_code();
    let phone = pi.manager.pair(&code, "phone").unwrap();
    let app = pi
        .manager
        .add_client("dashboard", vec![], kimo::core::models::Scope::Control)
        .unwrap();
    let call = |token: &str| {
        let mut r = Request::builder()
            .uri("/api/fleet/devices")
            .header(header::HOST, "pi4.local:7777")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        r.extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([10, 147, 17, 5], 50000))));
        kimo_app.clone().oneshot(r)
    };
    let res = call(&phone.token).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let devices: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(devices[0]["id"], "nuc");
    // An app key does not reach the machines' power.
    assert_eq!(
        call(&app.token).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}
