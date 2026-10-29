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
        // The stand-in leaves kimo running: it takes work again right away.
        manager.set_shutdown_grace(Duration::ZERO);
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
    // What it asks you to look at comes with it (audit: approval evidence).
    let approval = step(&fleet, "approve-release");
    assert_eq!(approval.evidence.len(), 1);
    assert_eq!(approval.evidence[0].path, "results.txt");
    assert_eq!(approval.evidence[0].step.as_deref(), Some("bench"));
    assert_eq!(approval.evidence[0].job.as_deref(), Some(bench.id.as_str()));
    // You approve: the workflow is done.
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

    // A job no machine can run waits, and says why. (Not `xcode`: a Mac runner reports it.)
    let mac_job = fleet
        .submit(
            serde_yaml_ng::from_str("task: ios\nrun: xcodebuild\nrequires: { labels: [iphone] }\n")
                .unwrap(),
        )
        .unwrap();
    until(&fleet, 10, "the iOS job waiting", |f| {
        f.job(&mac_job.id).unwrap().state == JobState::WaitingDevice
    })
    .await;
    assert_eq!(
        fleet.job(&mac_job.id).unwrap().reason.as_deref(),
        Some("no machine has labels iphone")
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

/// A stand-in program: a shell script in `dir`.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn fleet_with(tmp: &Path, registry: &str, change: impl FnOnce(&mut Config)) -> Arc<Fleet> {
    let dir = tmp.join("fleet");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("devices.yaml"), registry).unwrap();
    let mut cfg = Config::new(dir, "pi4".into());
    cfg.look_every_ms = 300;
    cfg.look_fast_ms = 200;
    cfg.ping = false;
    cfg.zerotier = false;
    change(&mut cfg);
    Fleet::open(cfg).unwrap()
}

async fn adopted(fleet: &Arc<Fleet>, id: &str) -> kimo_fleet::services::fleet::adopt::Adoption {
    use kimo_fleet::services::fleet::adopt::State;
    for _ in 0..200 {
        let a = fleet.adoption(id).unwrap();
        if a.state != State::Running {
            return a;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("adding {id} did not end");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_machine_joins_the_fleet_without_anyone_at_it() {
    let tmp = tempfile::tempdir().unwrap();
    let mut newbox = Machine::new(tmp.path(), "newbox");
    newbox.on().await;
    // A stand-in `ssh`: keeps the script it is given and answers as the script would on the
    // machine (the code comes from the machine's kimo, as the script gets it there).
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let code_file = tmp.path().join("code");
    let ssh = script(
        &bin,
        "ssh",
        &format!(
            "echo \"$@\" > '{args}'\ncat > '{got}'\necho 'kimo-step: installing kimo'\necho 'kimo-step: starting kimo as a service'\necho 'kimo-note: linger is off'\necho \"KIMO_PAIRING_CODE=$(cat '{code}')\"\n",
            args = tmp.path().join("ssh-args").display(),
            got = tmp.path().join("ssh-script").display(),
            code = code_file.display(),
        ),
    );
    let port = newbox.port;
    let fleet = fleet_with(tmp.path(), "devices: {}\n", |c| {
        c.ssh = ssh.display().to_string();
        c.kimo_port = port;
        c.kimo_version = Some("v9.9.9".into());
    });
    std::fs::write(&code_file, newbox.pairing_code()).unwrap();
    let a = fleet
        .adopt(
            serde_json::from_value(json!({
                "address": "127.0.0.1", "id": "newbox", "model": "nuc-11-extreme", "ssh_user": "jordi",
            }))
            .unwrap(),
        )
        .unwrap();
    assert_eq!(a.state, kimo_fleet::services::fleet::adopt::State::Running);
    let a = adopted(&fleet, "newbox").await;
    assert_eq!(a.error, None, "{a:#?}");
    assert!(a.steps.iter().any(|s| s == "installing kimo"), "{a:#?}");
    assert!(a.notes.iter().any(|n| n == "linger is off"));
    assert!(
        a.notes.iter().any(|n| n.contains("BIOS")),
        "the model's notes"
    );
    // SSH without prompts, as the user given; the script installs this coordinator's version
    // and runs kimo bound to the address it was found on.
    let args = std::fs::read_to_string(tmp.path().join("ssh-args")).unwrap();
    assert!(
        args.contains("BatchMode=yes") && args.contains("jordi@127.0.0.1 sh -s"),
        "{args}"
    );
    let sent = std::fs::read_to_string(tmp.path().join("ssh-script")).unwrap();
    assert!(sent.contains("KIMO_VERSION='v9.9.9'"));
    assert!(sent.contains("Environment=KIMO_BIND=127.0.0.1"));
    assert!(sent.contains("Environment=KIMO_NAME=newbox"));
    // Its entry, in a file of its own, from the model and its facts.
    let file = tmp.path().join("fleet/devices.d/newbox.yaml");
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# newbox: Intel NUC 11 Extreme"), "{text}");
    let d = fleet.device("newbox").unwrap();
    assert_eq!(d.registry.model.as_deref(), Some("nuc-11-extreme"));
    assert_eq!(d.registry.role.as_deref(), Some("gpu-worker"));
    assert!(d.labels.contains(&"gpu-worker".to_string()));
    assert!(d.paired);
    // The machine lists the coordinator as a fleet key, nothing more.
    let keys = newbox.manager.devices();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].role, kimo::services::devices::Role::Fleet);
    // The hand-written registry was not touched.
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("fleet/devices.yaml")).unwrap(),
        "devices: {}\n"
    );
    until(&fleet, 10, "the new machine up", |f| {
        f.device("newbox").is_ok_and(|d| d.facts.is_some())
    })
    .await;
    // Twice: no.
    assert!(fleet
        .adopt(serde_json::from_value(json!({ "address": "127.0.0.1", "id": "newbox" })).unwrap())
        .is_err());

    // Removed: its entry and its key go, on the machine too.
    let f = fleet.clone();
    blocking(move || f.remove("newbox")).await.unwrap();
    assert!(!file.exists());
    assert!(fleet.device("newbox").is_err());
    assert!(
        newbox.manager.devices().is_empty(),
        "the machine forgot the key"
    );
    // Only machines added this way are removed this way.
    assert!(fleet.remove("nothing").is_err());

    // With a code its kimo shows (no SSH), guessing the model from its facts.
    let code = newbox.pairing_code();
    fleet
        .adopt(
            serde_json::from_value(json!({ "address": "127.0.0.1", "id": "again", "code": code }))
                .unwrap(),
        )
        .unwrap();
    let a = adopted(&fleet, "again").await;
    assert_eq!(a.error, None, "{a:#?}");
    assert!(a.model.is_some() && a.role.is_some());
    newbox.off().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn adding_says_what_to_fix() {
    let tmp = tempfile::tempdir().unwrap();
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    // SSH refuses: the coordinator's key is not on the machine.
    let ssh = script(
        &bin,
        "ssh",
        "cat >/dev/null\necho 'jordi@100.64.0.9: Permission denied (publickey).' >&2\nexit 255\n",
    );
    let fleet = fleet_with(tmp.path(), "devices: {}\n", |c| {
        c.ssh = ssh.display().to_string()
    });
    fleet
        .adopt(serde_json::from_value(json!({ "address": "100.64.0.9", "id": "nuc" })).unwrap())
        .unwrap();
    let a = adopted(&fleet, "nuc").await;
    let e = a.error.unwrap();
    assert!(
        e.contains("Permission denied")
            && e.contains("ssh-copy-id")
            && e.contains("tailscale set --ssh"),
        "{e}"
    );
    assert!(!tmp.path().join("fleet/devices.d/nuc.yaml").exists());
    // Bad input never reaches a shell.
    for bad in [
        json!({ "address": "1.2.3.4; rm -rf ~" }),
        json!({ "address": "1.2.3.4", "ssh_user": "a b" }),
        json!({ "address": "1.2.3.4", "model": "toaster" }),
    ] {
        assert!(
            fleet
                .adopt(serde_json::from_value(bad.clone()).unwrap())
                .is_err(),
            "{bad}"
        );
    }

    // A kimo too old for fleet keys hands out an owner key: it is revoked, and pairing stops.
    let revoked = Arc::new(std::sync::Mutex::new(vec![]));
    let seen = revoked.clone();
    let old = axum::Router::new()
        .route(
            "/api/pair",
            axum::routing::post(|| async {
                axum::Json(json!({ "id": "pi4-fleet", "token": "kimo_dev_old" }))
            }),
        )
        .route(
            "/api/devices/{id}",
            axum::routing::delete(
                move |axum::extract::Path(id): axum::extract::Path<String>| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(id);
                        axum::Json(json!({ "ok": true }))
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, old).await.unwrap() });
    let fleet2 = fleet_with(&tmp.path().join("2"), "devices: {}\n", |c| {
        c.kimo_port = port
    });
    fleet2
        .adopt(
            serde_json::from_value(
                json!({ "address": "127.0.0.1", "id": "oldbox", "code": "AAAA-BBBB" }),
            )
            .unwrap(),
        )
        .unwrap();
    let a = adopted(&fleet2, "oldbox").await;
    assert!(a.error.unwrap().contains("too old"));
    assert_eq!(*revoked.lock().unwrap(), ["pi4-fleet"]);
    assert!(fleet2.device("oldbox").is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn machines_on_tailscale_are_found_with_what_answers_there() {
    let tmp = tempfile::tempdir().unwrap();
    let mut nuc = Machine::new(tmp.path(), "nuc");
    nuc.on().await;
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let status = r#"{
      "Self": {"HostName": "pi4", "DNSName": "pi4.tail.ts.net.", "OS": "linux", "TailscaleIPs": ["100.64.0.1"]},
      "Peer": {
        "a": {"HostName": "nuc-extreme", "DNSName": "nuc-extreme.tail.ts.net.", "OS": "linux", "TailscaleIPs": ["127.0.0.1"], "Online": true},
        "b": {"HostName": "iPhone", "DNSName": "iphone.tail.ts.net.", "OS": "iOS", "TailscaleIPs": ["100.64.0.3"], "Online": true},
        "c": {"HostName": "jetson", "DNSName": "jetson.tail.ts.net.", "OS": "linux", "TailscaleIPs": ["100.64.0.4"], "Online": false}
      }}"#;
    let tailscale = script(
        &bin,
        "tailscale",
        &format!(
            "[ \"$1\" = version ] && {{ echo 1.90.0; exit 0; }}\ncat <<'JSON'\n{status}\nJSON\n"
        ),
    );
    let port = nuc.port;
    let fleet = fleet_with(tmp.path(), "devices: {}\n", |c| {
        c.tailscale = Some(tailscale.display().to_string());
        c.kimo_port = port;
    });
    let f = fleet.clone();
    let d = blocking(move || f.discover()).await;
    assert!(d.tailscale && !d.zerotier);
    let by = |name: &str| d.machines.iter().find(|m| m.seen.name == name).unwrap();
    let nuc_found = by("nuc-extreme");
    assert_eq!(nuc_found.kimo, "answers");
    assert_eq!(nuc_found.model, "nuc-11-extreme");
    assert_eq!(nuc_found.role, "gpu-worker");
    assert_eq!(nuc_found.id, "nuc-extreme");
    assert!(!by("iphone").seen.can_run_kimo);
    assert_eq!(by("jetson").model, "jetson-orin-nano");
    assert_eq!(by("jetson").kimo, "unknown");
    assert!(by("pi4").seen.this);
    nuc.off().await;
}

/// Audit F8: notifications go out over HTTPS (TLS is built in) and a failed one is recorded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notifications_arrive_and_failures_are_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    // A stand-in ntfy over HTTP.
    let got = Arc::new(std::sync::Mutex::new(vec![]));
    let seen = got.clone();
    let ntfy = axum::Router::new().route(
        "/fleet",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
            let seen = seen.clone();
            async move {
                let title = headers
                    .get("title")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("")
                    .to_string();
                seen.lock().unwrap().push((title, body));
                "ok"
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, ntfy).await.unwrap() });
    let fleet = fleet(
        tmp.path(),
        &format!("ntfy: http://127.0.0.1:{port}/fleet\ndevices: {{}}\n"),
    );
    fleet
        .submit(serde_yaml_ng::from_str("task: release\nrunner: approval\n").unwrap())
        .unwrap();
    for _ in 0..50 {
        if !got.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let got = got.lock().unwrap().clone();
    assert_eq!(got.len(), 1);
    assert!(
        got[0].0.starts_with("kimo fleet") && got[0].1.contains("release waits for your approval")
    );

    // HTTPS to something that is not TLS: the handshake fails (TLS is there), and it is recorded.
    let plain = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = plain.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in plain.incoming().flatten() {
            use std::io::Write;
            let mut s = s;
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\n\r\nnot tls");
        }
    });
    let tmp2 = tempfile::tempdir().unwrap();
    let fleet = fleet_with(
        tmp2.path(),
        &format!("ntfy: https://127.0.0.1:{port}/fleet\ndevices: {{}}\n"),
        |_| {},
    );
    fleet
        .submit(serde_yaml_ng::from_str("task: release\nrunner: approval\n").unwrap())
        .unwrap();
    let mut failure = None;
    for _ in 0..100 {
        failure = fleet
            .events(None, None, 20)
            .into_iter()
            .find(|e| e.detail.starts_with("notification not delivered"));
        if failure.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let failure = failure.expect("the failed notification is recorded").detail;
    assert!(!failure.contains("TLS required"), "{failure}");
}

/// A proxy in front of a machine that loses the answer of the first job it passes on (the job
/// is created over there; the coordinator hears 503).
async fn lossy_proxy(target: String) -> u16 {
    let lost = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
        let (target, lost) = (target.clone(), lost.clone());
        async move {
            let method = req.method().to_string();
            let path = req
                .uri()
                .path_and_query()
                .map(|p| p.to_string())
                .unwrap_or_default();
            let token = req
                .headers()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::to_string);
            let ct = req
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            let body = axum::body::to_bytes(req.into_body(), 1 << 20)
                .await
                .unwrap()
                .to_vec();
            let first_job = method == "POST"
                && path == "/api/jobs"
                && !lost.swap(true, std::sync::atomic::Ordering::SeqCst);
            let (status, ct, bytes) = tokio::task::spawn_blocking(move || {
                kimo::client::Client::new(&target, token)
                    .raw(
                        &method,
                        path.trim_start_matches("/api"),
                        body,
                        ct.as_deref(),
                        30,
                    )
                    .unwrap()
            })
            .await
            .unwrap();
            if first_job {
                return (
                    axum::http::StatusCode::SERVICE_UNAVAILABLE,
                    "lost on the way",
                )
                    .into_response();
            }
            let mut res =
                (axum::http::StatusCode::from_u16(status).unwrap(), bytes).into_response();
            if let Some(ct) = ct {
                res.headers_mut()
                    .insert("content-type", ct.parse().unwrap());
            }
            res
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    port
}

use axum::response::IntoResponse;

async fn pair_all(fleet: &Arc<Fleet>, machines: &[(&str, &Machine)]) {
    for (id, m) in machines {
        let code = m.pairing_code();
        let (f, id) = (fleet.clone(), id.to_string());
        blocking(move || f.pair(&id, &code)).await.unwrap();
    }
}

/// Audit F1: an answer lost after the machine took the job never makes it run twice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_answer_never_runs_a_job_twice() {
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Machine::new(tmp.path(), "a");
    let mut b = Machine::new(tmp.path(), "b");
    a.on().await;
    b.on().await;
    let proxy = lossy_proxy(a.url()).await;
    let fleet = fleet(
        tmp.path(),
        &format!(
            "devices:\n  a:\n    kimo: http://127.0.0.1:{proxy}\n    labels: [x]\n    cost: 1\n  b:\n    kimo: {}\n    labels: [x]\n    cost: 2\n",
            b.url()
        ),
    );
    pair_all(&fleet, &[("a", &a), ("b", &b)]).await;
    until(&fleet, 10, "machines ready", |f| {
        state(f, "a") == "ready" && state(f, "b") == "ready"
    })
    .await;
    let marker = tmp.path().join("ran");
    let job = fleet
        .submit(
            serde_yaml_ng::from_str(&format!(
                "task: once\nrun: echo $(hostname) >> '{}'; sleep 1\nrequires: {{ labels: [x] }}\nretries: 0\n",
                marker.display()
            ))
            .unwrap(),
        )
        .unwrap();
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
    assert_eq!(done.device.as_deref(), Some("a"), "adopted where it runs");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap().lines().count(),
        1,
        "ran once"
    );
    assert!(b.manager.jobs().is_empty(), "never sent to b");
    a.off().await;
    b.off().await;
}

/// Audit F1: a coordinator that restarts with a job on its way checks it with the machine.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restarted_coordinator_settles_jobs_that_were_on_their_way() {
    use kimo_fleet::core::jobs::JobState;
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Machine::new(tmp.path(), "a");
    let mut b = Machine::new(tmp.path(), "b");
    a.on().await;
    b.on().await;
    let registry = format!(
        "devices:\n  a:\n    kimo: {}\n    labels: [x]\n    cost: 1\n  b:\n    kimo: {}\n    labels: [x]\n    cost: 2\n",
        a.url(),
        b.url()
    );
    let marker = tmp.path().join("ran");
    let (arrived, missing) = {
        let fleet = fleet(tmp.path(), &registry);
        pair_all(&fleet, &[("a", &a), ("b", &b)]).await;
        let spec = |n: &str| {
            serde_yaml_ng::from_str(&format!(
                "task: {n}\nrun: echo {n} >> '{}'\nrequires: {{ labels: [x] }}\n",
                marker.display()
            ))
            .unwrap()
        };
        (
            fleet.submit(spec("arrived")).unwrap(),
            fleet.submit(spec("missing")).unwrap(),
        )
    };
    // As the coordinator left them: both written down as sent to a; only one got there.
    let db = kimo_fleet::services::db::Db::open(&tmp.path().join("fleet/fleet.db")).unwrap();
    for j in [&arrived, &missing] {
        let mut j = j.clone();
        j.state = JobState::Dispatched;
        j.device = Some("a".into());
        j.attempt = 1;
        j.worker_job = Some(format!("{}-1", j.id));
        db.put_job(&j.id, j.created_at, "dispatched", &j).unwrap();
    }
    a.manager
        .create_job(
            serde_json::from_value(json!({
                "id": format!("{}-1", arrived.id),
                "command": format!("echo arrived >> '{}'", marker.display()),
            }))
            .unwrap(),
        )
        .unwrap();
    let fleet = fleet(tmp.path(), &registry);
    until(&fleet, 30, "both done", |f| {
        f.job(&arrived.id).unwrap().state.finished() && f.job(&missing.id).unwrap().state.finished()
    })
    .await;
    assert_eq!(fleet.job(&arrived.id).unwrap().state, JobState::Succeeded);
    assert_eq!(fleet.job(&arrived.id).unwrap().device.as_deref(), Some("a"));
    assert_eq!(fleet.job(&missing.id).unwrap().state, JobState::Succeeded);
    let ran = std::fs::read_to_string(&marker).unwrap();
    assert_eq!(ran.lines().filter(|l| *l == "arrived").count(), 1, "{ran}");
    assert_eq!(ran.lines().filter(|l| *l == "missing").count(), 1, "{ran}");
    a.off().await;
    b.off().await;
}

/// Audit F3: a step never runs without its inputs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_step_never_runs_without_its_inputs() {
    use kimo_fleet::core::jobs::JobState;
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Machine::new(tmp.path(), "a");
    a.on().await;
    let fleet = fleet(
        tmp.path(),
        &format!("devices:\n  a:\n    kimo: {}\n    labels: [x]\n", a.url()),
    );
    pair_all(&fleet, &[("a", &a)]).await;
    // An input that is not an output of its step: refused.
    let wrong = "workflow: w\nsteps:\n  - task: producer\n    run: echo d > data.txt\n    outputs: [data.txt]\n  - task: consumer\n    needs: [producer]\n    inputs: { m: producer/missing.txt }\n    run: cat missing.txt || echo MISSING\n";
    let e = fleet
        .submit_workflow(serde_yaml_ng::from_str(wrong).unwrap())
        .unwrap_err();
    assert!(
        format!("{e:#}").contains("not an output of producer"),
        "{e:#}"
    );
    // An optional producer that fails leaves its consumer without the input: the consumer
    // fails, saying so, and never runs.
    let marker = tmp.path().join("consumer-ran");
    let flow = format!(
        "workflow: w\nsteps:\n  - task: producer\n    run: exit 1\n    outputs: [data.txt]\n    optional: true\n    requires: {{ labels: [x] }}\n  - task: consumer\n    needs: [producer]\n    inputs: {{ m: producer/data.txt }}\n    run: touch '{}'\n    requires: {{ labels: [x] }}\n",
        marker.display()
    );
    let run = fleet
        .submit_workflow(serde_yaml_ng::from_str(&flow).unwrap())
        .unwrap();
    until(&fleet, 30, "the workflow ended", |f| {
        f.workflow(&run.id).unwrap().0.state != kimo_fleet::services::fleet::RunState::Running
    })
    .await;
    let (w, jobs) = fleet.workflow(&run.id).unwrap();
    assert_eq!(w.state, kimo_fleet::services::fleet::RunState::Failed);
    assert_eq!(jobs[1].state, JobState::Failed);
    assert!(jobs[1]
        .reason
        .as_deref()
        .unwrap()
        .contains("producer/data.txt is not available"));
    assert!(!marker.exists(), "the consumer never ran");
    a.off().await;
}

/// Audit F4: cancelling a job whose machine does not answer keeps it (and its lease) until
/// the machine confirms the run ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_waits_for_the_machine_to_confirm() {
    use kimo_fleet::core::jobs::JobState;
    let tmp = tempfile::tempdir().unwrap();
    let mut a = Machine::new(tmp.path(), "a");
    a.on().await;
    let fleet = fleet(
        tmp.path(),
        &format!("devices:\n  a:\n    kimo: {}\n    labels: [x]\n", a.url()),
    );
    pair_all(&fleet, &[("a", &a)]).await;
    let job = fleet
        .submit(
            serde_yaml_ng::from_str("task: long\nrun: sleep 60\nrequires: { labels: [x] }\n")
                .unwrap(),
        )
        .unwrap();
    until(&fleet, 20, "running", |f| {
        f.job(&job.id).unwrap().state == JobState::Running
    })
    .await;
    let wid = fleet.job(&job.id).unwrap().worker_job.unwrap();
    // Its machine's kimo goes away; the job itself keeps running there.
    a.off().await;
    let f = fleet.clone();
    let id = job.id.clone();
    let c = blocking(move || f.cancel(&id)).await.unwrap();
    assert_eq!(c.state, JobState::Running);
    assert!(c.cancel_requested);
    assert!(c.reason.unwrap().contains("does not answer"));
    assert_eq!(
        fleet.device("a").unwrap().jobs,
        vec![job.id.clone()],
        "its lease stays"
    );
    assert_eq!(
        a.manager.job(&wid).unwrap().state,
        kimo::core::jobs::JobState::Running
    );
    // Back: the cancel goes through, and only then is the job cancelled.
    a.on().await;
    until(&fleet, 30, "cancelled", |f| {
        f.job(&job.id).unwrap().state == JobState::Cancelled
    })
    .await;
    assert_eq!(
        a.manager.job(&wid).unwrap().state,
        kimo::core::jobs::JobState::Cancelled
    );
    assert!(fleet.device("a").unwrap().jobs.is_empty());
    a.off().await;
}
