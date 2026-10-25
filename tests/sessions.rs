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

fn proc_state(pid: i32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 2..].chars().next()
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
    let (bin, work) = (tmp.path().join("bin"), tmp.path().join("work"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(tmp.path().join("home")).unwrap();
    let fake = bin.join("claude");
    std::fs::write(
        &fake,
        "#!/bin/sh\necho \"CFG=$CLAUDE_CONFIG_DIR ARGS=$*\"\nexec sleep 60\n",
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::env::set_var(
        "PATH",
        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
    );

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
    wait_for(&m, &s.spec.id, &format!("accounts/{} ARGS=\r\n", a.id)).await;

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
        &format!("accounts/{} ARGS=--continue", b.id),
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
    wait_for(&m, &s.spec.id, "ARGS=--continue").await;
    m.delete(&s.spec.id).await.unwrap();
    assert!(m.session(&s.spec.id).is_none());
    m.delete_account(&a.id).unwrap();
}
