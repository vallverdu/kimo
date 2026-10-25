//! End-to-end session lifecycle with real PTYs and a fake `claude` on PATH.

use kimo::core::lifecycle::Action;
use kimo::core::models::{Provider, SessionKind, SessionStatus};
use kimo::services::sessions::{Manager, ManagerConfig, NewSession};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn manager(root: &Path) -> Arc<Manager> {
    Manager::new(ManagerConfig {
        data_dir: root.join("kimo"),
        user_home: root.join("home"),
        shell: "/bin/sh".into(),
        scrollback_bytes: 64 * 1024,
        projects_dir: root.join("home/dev"),
        port: 7777,
        host_name: "test".into(),
    })
    .unwrap()
}

/// Wait until the session's output (scrollback + live) contains `needle`.
async fn wait_for(m: &Manager, id: &str, needle: &str) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let text = String::from_utf8_lossy(&m.attach(id).unwrap().replay).into_owned();
        if text.contains(needle) {
            return text;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {needle:?} in:\n{text}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Process state letter (`S` sleeping, `T` stopped): /proc on Linux, `ps` elsewhere.
fn proc_state(pid: i32) -> Option<char> {
    if cfg!(target_os = "linux") {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        return stat[stat.rfind(')')? + 2..].chars().next();
    }
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().chars().next()
}

#[tokio::test]
async fn shell_session_echoes_input() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    let m = manager(tmp.path());
    let s = m
        .create(
            NewSession {
                name: "sh".into(),
                kind: SessionKind::Shell,
                cwd: "~".into(),
                account: None,
                args: vec![],
                command: Some(vec!["/bin/sh".into()]),
            },
            true,
        )
        .unwrap();
    assert_eq!(s.status, SessionStatus::Running);
    m.write(&s.spec.id, b"echo hi-$((1+2))\n".to_vec());
    wait_for(&m, &s.spec.id, "hi-3").await;
    use kimo::core::activity::Activity;
    assert_eq!(
        m.session(&s.spec.id).unwrap().activity,
        Some(Activity::Working),
        "fresh output"
    );
    m.write(
        &s.spec.id,
        b"printf 'Overwrite? (y/n) '; read answer\n".to_vec(),
    );
    wait_for(&m, &s.spec.id, "Overwrite?").await;
    tokio::time::sleep(Duration::from_millis(
        kimo::core::activity::WORKING_MS + 300,
    ))
    .await;
    assert_eq!(
        m.session(&s.spec.id).unwrap().activity,
        Some(Activity::Waiting),
        "asks a question"
    );
    m.write(&s.spec.id, b"y\n".to_vec());
    m.write(&s.spec.id, b"exit 7\n".to_vec());
    wait_for(&m, &s.spec.id, "exited (7)").await;
    assert_eq!(m.session(&s.spec.id).unwrap().status, SessionStatus::Exited);
    assert!(m
        .create(
            NewSession {
                name: "x".into(),
                kind: SessionKind::Shell,
                cwd: "/no/such/dir".into(),
                account: None,
                args: vec![],
                command: None
            },
            false
        )
        .is_err());
}

#[tokio::test]
async fn pause_switch_account_stop_and_continue() {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    fake_agents();

    let m = manager(tmp.path());
    let a = m.add_account("Work Max", Provider::Claude).unwrap();
    let b = m.add_account("Personal", Provider::Claude).unwrap();
    assert!(m.add_account("Codex", Provider::Codex).is_ok());
    let s = m
        .create(
            NewSession {
                name: "api".into(),
                kind: SessionKind::Claude,
                cwd: work.display().to_string(),
                account: Some(a.id.clone()),
                args: vec![],
                command: None,
            },
            true,
        )
        .unwrap();
    let conv = s
        .spec
        .conversation
        .clone()
        .expect("claude sessions own a conversation id");
    wait_for(
        &m,
        &s.spec.id,
        &format!("accounts/{} ARGS=--session-id {conv}\r\n", a.id),
    )
    .await;
    // Let the fake agent write its transcript (as a real one has long done before you switch).
    tokio::time::sleep(Duration::from_millis(1300)).await;

    // Pause freezes the process tree; resume thaws it.
    let pid = m.session(&s.spec.id).unwrap().pid.unwrap();
    m.act(&s.spec.id, Action::Pause).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(proc_state(pid), Some('T'));
    assert!(m.act(&s.spec.id, Action::Pause).await.is_err());
    m.act(&s.spec.id, Action::Resume).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(proc_state(pid), Some('S'));

    // Subscription limit: move every session of A to B; it continues the conversation.
    assert_eq!(
        m.move_sessions(Some(&a.id), Some(b.id.clone()))
            .await
            .unwrap(),
        1
    );
    wait_for(
        &m,
        &s.spec.id,
        &format!("accounts/{} ARGS=--resume {conv}", b.id),
    )
    .await;
    assert_eq!(
        m.session(&s.spec.id).unwrap().spec.account.as_deref(),
        Some(b.id.as_str())
    );
    assert!(proc_state(pid).is_none(), "old process must be gone");
    assert!(m
        .accounts()
        .iter()
        .any(|acc| acc.account.id == b.id && acc.sessions == 1));

    // Usage per subscription: the message before the move counts for A, the one after for B
    // (the fake agent writes it a second after starting).
    tokio::time::sleep(Duration::from_millis(1400)).await;
    let usage = m.usage();
    let of = |id: &str| {
        usage
            .iter()
            .find(|u| u.account.as_deref() == Some(id))
            .unwrap()
            .clone()
    };
    assert_eq!((of(&a.id).messages_7d, of(&a.id).tokens_7d), (1, 15));
    assert_eq!((of(&b.id).messages_7d, of(&b.id).tokens_5h), (1, 15));
    assert_eq!(of(&b.id).detail_7d.cache_read, 100);

    // Stop frees the processes; start continues.
    m.sample();
    let new_pid = m.session(&s.spec.id).unwrap().pid.unwrap();
    assert!(m.session(&s.spec.id).unwrap().usage.rss_bytes > 0);
    m.act(&s.spec.id, Action::Stop).await.unwrap();
    assert_eq!(
        m.session(&s.spec.id).unwrap().status,
        SessionStatus::Stopped
    );
    assert!(proc_state(new_pid).is_none());
    assert!(
        m.delete_account(&b.id).is_err(),
        "account in use cannot be deleted"
    );

    // State survives a restart of kimo; sessions come back stopped.
    drop(m);
    let m = manager(tmp.path());
    let again = m.session(&s.spec.id).unwrap();
    assert_eq!(
        (again.status, again.spec.started_once),
        (SessionStatus::Stopped, true)
    );
    m.act(&s.spec.id, Action::Start).await.unwrap();
    wait_for(&m, &s.spec.id, &format!("ARGS=--resume {conv}")).await;
    m.delete(&s.spec.id).await.unwrap();
    assert!(m.session(&s.spec.id).is_none());
    m.delete_account(&a.id).unwrap();
}

#[tokio::test]
async fn agent_settings_reach_the_command_line() {
    let tmp = tempfile::tempdir().unwrap();
    let (bin, work) = (tmp.path().join("bin"), tmp.path().join("work"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    // A fake codex (the other test owns the fake claude on PATH).
    let fake = bin.join("codex");
    std::fs::write(&fake, "#!/bin/sh\necho \"CODEX ARGS=$*\"\nexec sleep 60\n").unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let m = manager(tmp.path());
    let mut settings = m.settings();
    settings.agents.codex.permissions = kimo::core::models::Permissions::Yolo;
    settings.agents.codex.model = "gpt-6".into();
    m.set_settings(settings).unwrap();
    let s = m
        .create(
            NewSession {
                name: "c".into(),
                kind: SessionKind::Codex,
                cwd: work.display().to_string(),
                account: None,
                args: vec![],
                command: Some(vec![fake.display().to_string()]),
            },
            false,
        )
        .unwrap();
    // Custom commands never get the agent preferences.
    m.act(&s.spec.id, Action::Start).await.unwrap();
    wait_for(&m, &s.spec.id, "CODEX ARGS=\r\n").await;
    m.delete(&s.spec.id).await.unwrap();
}

/// Fake `claude` and `codex` on PATH, set up once for every test (tests run in parallel and
/// share the process environment).
fn fake_agents() {
    static ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        let bin = std::env::temp_dir().join(format!("kimo-fake-agents-{}", std::process::id()));
        std::fs::create_dir_all(&bin).unwrap();
        let write = |name: &str, script: &str| {
            let path = bin.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        };
        write(
            "claude",
            concat!(
                "#!/bin/sh\n",
                "echo \"CFG=$CLAUDE_CONFIG_DIR ARGS=$*\"\n",
                // Like Claude: a conversation started with --session-id gets a transcript.
                // Whole-second timestamps (portable `date`), written a second after the start so
                // they never fall before the run kimo recorded.
                "if [ \"$1\" = --session-id ] || [ \"$1\" = --resume ]; then sleep 1; mkdir -p \"$CLAUDE_CONFIG_DIR/projects/w\"; ",
                // One assistant message with usage per start, timestamped like Claude Code does.
                "printf '{\"type\":\"assistant\",\"timestamp\":\"%s\",\"message\":{\"id\":\"m%s\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":100}}}\\n' ",
                "\"$(date -u +%Y-%m-%dT%H:%M:%S.000Z)\" \"$$\" >> \"$CLAUDE_CONFIG_DIR/projects/w/$2.jsonl\"; fi\n",
                "exec sleep 60\n"
            ),
        );
        write(
            "codex",
            concat!(
                "#!/bin/sh\n",
                "echo \"CODEX ARGS=$*\"\n",
                // Like Codex: a new session writes its log, whose first line names the session.
                "if [ \"$1\" != resume ]; then d=\"$CODEX_HOME/sessions/2026/01/01\"; mkdir -p \"$d\"; t=$(date -u +%Y-%m-%dT%H:%M:%S.000Z); ",
                "printf '{\"timestamp\":\"%s\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-%s\",\"cwd\":\"%s\",\"timestamp\":\"%s\"}}\\n' ",
                "\"$t\" \"$$\" \"$(pwd -P)\" \"$t\" > \"$d/rollout-$$.jsonl\"; fi\n",
                "exec sleep 60\n"
            ),
        );
        let path = std::env::var("PATH").unwrap();
        std::env::set_var("PATH", format!("{}:{path}", bin.display()));
    });
}

#[tokio::test]
async fn codex_sessions_learn_their_id_and_resume_it() {
    let tmp = tempfile::tempdir().unwrap();
    // Started through a symlinked folder: agents record the resolved path (as on macOS, where
    // temporary folders live under /var → /private/var).
    let real = tmp.path().join("real-work");
    std::fs::create_dir_all(&real).unwrap();
    let work = tmp.path().join("work");
    std::os::unix::fs::symlink(&real, &work).unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    fake_agents();
    let m = manager(tmp.path());
    // An account keeps the fake away from the real ~/.codex (its sessions link into the test home).
    let acc = m.add_account("Team", Provider::Codex).unwrap();
    let s = m
        .create(
            NewSession {
                name: "cx".into(),
                kind: SessionKind::Codex,
                cwd: work.display().to_string(),
                account: Some(acc.id.clone()),
                args: vec![],
                command: None,
            },
            true,
        )
        .unwrap();
    wait_for(&m, &s.spec.id, "CODEX ARGS=\r\n").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    m.sample();
    let id = m
        .session(&s.spec.id)
        .unwrap()
        .spec
        .conversation
        .expect("codex id learned from its log");
    assert!(id.starts_with("codex-"), "{id}");
    m.act(&s.spec.id, Action::Restart).await.unwrap();
    wait_for(&m, &s.spec.id, &format!("CODEX ARGS=resume {id}")).await;
    m.delete(&s.spec.id).await.unwrap();
}

#[tokio::test]
async fn agents_message_each_other_and_read_replies() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    let m = manager(tmp.path());
    let shell = |name: &str, cmd: &[&str]| NewSession {
        name: name.into(),
        kind: SessionKind::Shell,
        cwd: "~".into(),
        account: None,
        args: vec![],
        command: Some(cmd.iter().map(|s| s.to_string()).collect()),
    };
    let alice = m.create(shell("alice", &["/bin/sh"]), true).unwrap();
    let bob = m.create(shell("bob", &["cat"]), true).unwrap();
    // Names, ids and id prefixes all resolve.
    assert_eq!(m.resolve("bob").unwrap().spec.id, bob.spec.id);
    assert_eq!(m.resolve(&bob.spec.id[..5]).unwrap().spec.id, bob.spec.id);
    assert!(m.resolve("nobody").is_err());
    // A message is typed into bob, signed by alice with how to answer.
    let to = m
        .message(
            "bob",
            "please review the parser",
            Some(&alice.spec.id),
            None,
        )
        .await
        .unwrap();
    assert_eq!(to, bob.spec.id);
    let expected = format!(
        "[kimo message from agent alice (id {}); reply with: kimo send {} ",
        alice.spec.id, alice.spec.id
    );
    wait_for(&m, &bob.spec.id, &expected).await;
    // Reading a shell gives its screen text.
    let reply = m.reply("bob").unwrap();
    assert_eq!(reply.source, "terminal");
    assert!(reply.text.contains("please review the parser"));
    // Stopped agents do not take messages.
    m.act(&bob.spec.id, Action::Stop).await.unwrap();
    assert!(m.message("bob", "hi", None, None).await.is_err());
}

#[tokio::test]
async fn adding_a_folder_continues_its_earlier_conversations() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let folder = tmp.path().join("app");
    std::fs::create_dir_all(&folder).unwrap();
    fake_agents();
    let path = std::fs::canonicalize(&folder)
        .unwrap()
        .display()
        .to_string();
    // A Codex conversation, then a newer Claude one, made in that folder before kimo knew it.
    let codex_dir = home.join(".codex/sessions/2026/01/01");
    std::fs::create_dir_all(&codex_dir).unwrap();
    std::fs::write(
        codex_dir.join("rollout-old.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::json!({"timestamp":"2026-01-01T00:00:00.000Z","type":"session_meta","payload":{"id":"codex-earlier","cwd":path,"timestamp":"2026-01-01T00:00:00.000Z"}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"UserMessage","content":[{"type":"text","text":"Build the game"}]}}})
        ),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let claude_dir = home
        .join(".claude/projects")
        .join(kimo::core::history::claude_dir_name(&path));
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("claude-earlier.jsonl"),
        format!(
            "{}\n{}\n",
            serde_json::json!({"type":"user","cwd":path,"sessionId":"claude-earlier","message":{"content":"Fix the parser"}}),
            serde_json::json!({"type":"ai-title","aiTitle":"Parser fixes"})
        ),
    )
    .unwrap();
    // A conversation of another folder that happens to share Claude's directory name is ignored.
    std::fs::write(
        claude_dir.join("elsewhere.jsonl"),
        serde_json::json!({"type":"user","cwd":"/somewhere/else","message":{"content":"x"}})
            .to_string(),
    )
    .unwrap();

    let m = manager(tmp.path());
    let found = m.conversations(&path);
    let ids: Vec<(&str, &str)> = found
        .iter()
        .map(|c| (c.id.as_str(), c.title.as_str()))
        .collect();
    assert_eq!(
        ids,
        vec![
            ("claude-earlier", "Parser fixes"),
            ("codex-earlier", "Build the game")
        ]
    );

    // Adding the folder as a project continues both; the newest (Claude) starts right away.
    let made = m
        .create_project(kimo::services::projects::NewProject {
            name: String::new(),
            mode: kimo::services::projects::ProjectMode::Existing,
            path: Some(path.clone()),
            git: false,
            url: None,
            resume: true,
        })
        .unwrap();
    assert_eq!(made.continued.len(), 2);
    let (claude, codex) = (&made.continued[0], &made.continued[1]);
    assert_eq!(
        (claude.spec.kind, claude.spec.conversation.as_deref()),
        (SessionKind::Claude, Some("claude-earlier"))
    );
    wait_for(&m, &claude.spec.id, "ARGS=--resume claude-earlier").await;
    assert_eq!(
        codex.status,
        SessionStatus::Stopped,
        "the other one waits for continue"
    );
    m.act(&codex.spec.id, Action::Start).await.unwrap();
    wait_for(&m, &codex.spec.id, "CODEX ARGS=resume codex-earlier").await;

    // Attached conversations are marked, and continuing again reuses the same session.
    assert!(m.conversations(&path).iter().all(|c| c.session.is_some()));
    let again = m
        .continue_conversation(SessionKind::Claude, &path, "claude-earlier", None, true)
        .unwrap();
    assert_eq!(again.spec.id, claude.spec.id);
    for s in [&claude.spec.id, &codex.spec.id] {
        m.delete(s).await.unwrap();
    }
}
