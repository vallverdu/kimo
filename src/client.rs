//! Blocking Rust client for kimo's API: JSON calls, the live event stream and terminals.
//! The desktop app uses it; any Rust tool can too.

use anyhow::{anyhow, bail, Result};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone)]
pub struct Client {
    base: String,
    token: Option<String>,
    agent: ureq::Agent,
}

/// Bytes and control messages to a terminal.
pub enum TermOut {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

/// What a terminal connection delivers.
pub enum TermIn {
    /// A new connection: the replayed scrollback follows, so start from a clean screen.
    Reset,
    Data(Vec<u8>),
}

/// A terminal connection kept alive (reconnecting) until dropped.
pub struct TerminalLink {
    pub tx: Sender<TermOut>,
    pub rx: Receiver<TermIn>,
    alive: Arc<AtomicBool>,
}

impl Drop for TerminalLink {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

impl Client {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(120)))
            .build()
            .into();
        Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            agent,
        }
    }

    /// The kimo on this machine (requests from the machine itself are the owner).
    pub fn local(port: u16) -> Self {
        Self::new(&format!("http://127.0.0.1:{port}"), None)
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api{path}", self.base)
    }

    fn finish(mut res: ureq::http::Response<ureq::Body>) -> Result<Value> {
        let status = res.status();
        let text = res.body_mut().read_to_string().unwrap_or_default();
        let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if !status.is_success() {
            bail!(
                "{}",
                value["error"]
                    .as_str()
                    .unwrap_or(status.canonical_reason().unwrap_or("request failed"))
            );
        }
        Ok(value)
    }

    /// A call that gives up after `secs` seconds (unreachable hosts).
    pub fn call_timeout(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        secs: u64,
    ) -> Result<Value> {
        let quick = Self {
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .timeout_global(Some(Duration::from_secs(secs)))
                .build()
                .into(),
            ..self.clone()
        };
        quick.call(method, path, body)
    }

    pub fn call(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value> {
        let url = self.url(path);
        let auth = self.token.as_ref().map(|t| format!("Bearer {t}"));
        macro_rules! with_auth {
            ($req:expr) => {{
                let req = $req;
                match &auth {
                    Some(a) => req.header("Authorization", a),
                    None => req,
                }
            }};
        }
        let res = match (method, body) {
            ("GET", _) => with_auth!(self.agent.get(&url)).call(),
            ("DELETE", _) => with_auth!(self.agent.delete(&url)).call(),
            ("POST", Some(b)) => with_auth!(self.agent.post(&url)).send_json(b),
            ("POST", None) => with_auth!(self.agent.post(&url)).send_empty(),
            ("PUT", Some(b)) => with_auth!(self.agent.put(&url)).send_json(b),
            ("PATCH", Some(b)) => with_auth!(self.agent.patch(&url)).send_json(b),
            _ => bail!("unsupported call {method} {path}"),
        }
        .map_err(|e| anyhow!("cannot reach kimo at {}: {e}", self.base))?;
        Self::finish(res)
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        Ok(serde_json::from_value(self.call("GET", path, None)?)?)
    }

    pub fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.call("POST", path, Some(&body))
    }

    pub fn put(&self, path: &str, body: Value) -> Result<Value> {
        self.call("PUT", path, Some(&body))
    }

    pub fn patch(&self, path: &str, body: Value) -> Result<Value> {
        self.call("PATCH", path, Some(&body))
    }

    pub fn delete(&self, path: &str) -> Result<Value> {
        self.call("DELETE", path, None)
    }

    fn token_query(&self) -> String {
        self.token
            .as_ref()
            .map(|t| format!("?token={t}"))
            .unwrap_or_default()
    }

    /// Call `on_update({sessions, accounts})` on every change, reconnecting forever, in a thread.
    pub fn subscribe(&self, on_update: impl Fn(Value) + Send + 'static) {
        let url = format!("{}{}", self.url("/events"), self.token_query());
        std::thread::Builder::new()
            .name("kimo-events".into())
            .spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .http_status_as_error(false)
                    .build()
                    .into();
                loop {
                    if let Ok(res) = agent.get(&url).call() {
                        if res.status().is_success() {
                            let reader = std::io::BufReader::new(res.into_body().into_reader());
                            let mut event = String::new();
                            for line in reader.lines() {
                                let Ok(line) = line else { break };
                                if let Some(name) = line.strip_prefix("event: ") {
                                    event = name.to_string();
                                } else if let Some(data) = line.strip_prefix("data: ") {
                                    if event == "update" {
                                        if let Ok(v) = serde_json::from_str(data) {
                                            on_update(v);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            })
            .expect("events thread");
    }

    /// Open a session's terminal; `notify` is called when output arrives.
    pub fn terminal(&self, session_id: &str, notify: Notify) -> TerminalLink {
        let url = format!(
            "{}{}",
            self.url(&format!("/sessions/{session_id}/term"))
                .replacen("http", "ws", 1),
            self.token_query()
        );
        let (out_tx, out_rx) = mpsc::channel::<TermOut>();
        let (in_tx, in_rx) = mpsc::channel::<TermIn>();
        let alive = Arc::new(AtomicBool::new(true));
        let still = alive.clone();
        std::thread::Builder::new()
            .name(format!("kimo-term-{session_id}"))
            .stack_size(256 * 1024)
            .spawn(move || {
                use tungstenite::{stream::MaybeTlsStream, Message};
                while still.load(Ordering::Relaxed) {
                    let Ok((mut ws, _)) = tungstenite::connect(&url) else {
                        std::thread::sleep(Duration::from_millis(1500));
                        continue;
                    };
                    if let MaybeTlsStream::Plain(s) = ws.get_mut() {
                        let _ = s.set_read_timeout(Some(Duration::from_millis(25)));
                    }
                    let _ = in_tx.send(TermIn::Reset);
                    'conn: while still.load(Ordering::Relaxed) {
                        loop {
                            match ws.read() {
                                Ok(Message::Binary(b)) => {
                                    let _ = in_tx.send(TermIn::Data(b.to_vec()));
                                    notify();
                                }
                                Ok(Message::Text(t)) => {
                                    let _ = in_tx.send(TermIn::Data(t.as_bytes().to_vec()));
                                    notify();
                                }
                                Ok(Message::Close(_)) => break 'conn,
                                Ok(_) => {}
                                Err(tungstenite::Error::Io(e))
                                    if matches!(
                                        e.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::TimedOut
                                    ) =>
                                {
                                    break
                                }
                                Err(_) => break 'conn,
                            }
                        }
                        while let Ok(out) = out_rx.try_recv() {
                            let msg = match out {
                                TermOut::Bytes(b) => Message::Binary(b.into()),
                                TermOut::Resize { cols, rows } => Message::Text(
                                    format!(r#"{{"type":"resize","cols":{cols},"rows":{rows}}}"#)
                                        .into(),
                                ),
                            };
                            if ws.send(msg).is_err() {
                                break 'conn;
                            }
                        }
                    }
                    let _ = ws.close(None);
                    if still.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(1500));
                    }
                }
            })
            .expect("terminal thread");
        TerminalLink {
            tx: out_tx,
            rx: in_rx,
            alive,
        }
    }
}
