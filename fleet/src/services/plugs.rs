//! Smart plugs over the network: the TPAP login and commands (`core::tapo`), reaching a plug
//! on another LAN through an SSH tunnel, and the stored credential.

use super::{now_ms, Fleet};
use crate::core::registry::{Device, Wake};
use crate::core::tapo::{self, Cipher, CutCheck, Login, PlugAction, PlugStatus};
use crate::services::net;
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand_core::RngCore;
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CALL_SECS: u64 = 8;
const CREDENTIAL_FILE: &str = "plug-credential";

fn credential_path(dir: &Path) -> PathBuf {
    dir.join(CREDENTIAL_FILE)
}

/// The stored credential (a hash of the TP-Link password, see `tapo::credential`).
pub fn load_credential(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(credential_path(dir)).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Keep the hash of the password, readable only by this user; the password itself is not kept.
pub fn save_password(dir: &Path, password: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if password.is_empty() {
        bail!("the password is empty");
    }
    let tmp = dir.join(format!(
        "{CREDENTIAL_FILE}.tmp-{}",
        uuid::Uuid::new_v4().simple()
    ));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .with_context(|| format!("writing {}", tmp.display()))?;
    f.write_all(tapo::credential(password).as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, credential_path(dir))?;
    Ok(())
}

pub fn forget_credential(dir: &Path) -> Result<()> {
    match std::fs::remove_file(credential_path(dir)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// `-L` through a machine that can reach the plug: closed when dropped.
struct Tunnel {
    child: Child,
    port: u16,
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn open_tunnel(ssh: &str, hop: &str, address: &str) -> Result<Tunnel> {
    let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
    let mut child = Command::new(ssh)
        .args([
            "-N",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=6",
            "-o",
            "ExitOnForwardFailure=yes",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-L",
            &format!("127.0.0.1:{port}:{address}:80"),
            hop,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("running {ssh}"))?;
    let deadline = Instant::now() + Duration::from_secs(9);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200))
            .is_ok()
        {
            return Ok(Tunnel { child, port });
        }
        if child.try_wait()?.is_some() {
            bail!("cannot reach the plug through {hop}: that machine does not answer over SSH");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    bail!("cannot reach the plug through {hop}: the tunnel did not open")
}

/// A logged-in session with one plug.
pub struct Session {
    agent: ureq::Agent,
    base: String,
    stok: String,
    cipher: Cipher,
    seq: u32,
    _tunnel: Option<Tunnel>,
}

fn post_json(agent: &ureq::Agent, base: &str, params: Value) -> Result<Value> {
    let body = serde_json::to_vec(&json!({ "method": "login", "params": params }))?;
    let mut res = agent
        .post(format!("{base}/"))
        .header("Content-Type", "application/json")
        .send(&body[..])
        .map_err(|e| anyhow!("the plug does not answer: {e}"))?;
    let status = res.status().as_u16();
    let bytes = res.body_mut().read_to_vec()?;
    let v: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("the plug answered {status} with something that is not JSON"))?;
    match v["error_code"].as_i64().unwrap_or(0) {
        0 => Ok(v["result"].clone()),
        code if tapo::refuses_password(code) => Err(tapo::PasswordRefused.into()),
        code => bail!("{}", tapo::error_text(code)),
    }
}

fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    rand_core::OsRng.fill_bytes(&mut b);
    b
}

impl Session {
    /// Log in to a plug at `address`, directly or through `hop` (`user@host`).
    pub fn open(ssh: &str, address: &str, hop: Option<&str>, credential: &str) -> Result<Self> {
        let tunnel = hop.map(|h| open_tunnel(ssh, h, address)).transpose()?;
        let base = match &tunnel {
            Some(t) => format!("http://127.0.0.1:{}", t.port),
            None => format!("http://{address}"),
        };
        Self::at(base, credential, tunnel)
    }

    /// Log in at a base URL.
    pub fn connect(base: &str, credential: &str) -> Result<Self> {
        Self::at(base.to_string(), credential, None)
    }

    fn at(base: String, credential: &str, tunnel: Option<Tunnel>) -> Result<Self> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(CALL_SECS)))
            .build()
            .into();
        // Only one way of hashing can be right, and only a rare login can tell them apart.
        let (cipher, stok, seq) = match Self::login(&agent, &base, credential, false)? {
            Attempt::Done(c, s, q) => (c, s, q),
            Attempt::Refused(error) => match Self::login(&agent, &base, credential, true)? {
                Attempt::Done(c, s, q) => (c, s, q),
                Attempt::Refused(_) => return Err(error),
            },
        };
        Ok(Self {
            agent,
            base,
            stok,
            cipher,
            seq,
            _tunnel: tunnel,
        })
    }

    fn login(agent: &ureq::Agent, base: &str, credential: &str, full: bool) -> Result<Attempt> {
        let user_random = random(32);
        let reg = post_json(
            agent,
            base,
            json!({
                "sub_method": "pake_register",
                "username": tapo::USERNAME,
                "user_random": B64.encode(&user_random),
                "cipher_suites": [1],
                "encryption": ["aes_128_ccm"],
                "passcode_type": "userpw",
                "stok": null,
            }),
        )?;
        let id = reg["extra_crypt"]["params"]["passwd_id"]
            .as_i64()
            .unwrap_or(0);
        if id != 2 {
            bail!(
                "this plug asks for a kind of password kimo does not know (passwd_id {id}): its firmware may have changed"
            );
        }
        let field = |k: &str| -> Result<Vec<u8>> {
            let text = reg[k]
                .as_str()
                .ok_or_else(|| anyhow!("the plug's answer has no {k}"))?;
            B64.decode(text)
                .map_err(|_| anyhow!("the plug's {k} is not base64"))
        };
        let iterations = reg["iterations"]
            .as_u64()
            .ok_or_else(|| anyhow!("the plug's answer has no iterations"))?
            as u32;
        let login = Login::new(
            credential,
            &field("dev_salt")?,
            iterations,
            &user_random,
            &field("dev_random")?,
        )
        .map_err(|e| anyhow!(e))?;
        let agreed = login
            .finish(&field("dev_share")?, full)
            .map_err(|e| anyhow!(e))?;
        let share = post_json(
            agent,
            base,
            json!({
                "sub_method": "pake_share",
                "user_share": B64.encode(&login.share),
                "user_confirm": B64.encode(agreed.user_confirm),
            }),
        );
        let share = match share {
            Ok(s) => s,
            // The other hash is only worth trying when the two differ.
            Err(e) if login.ambiguous() && !full => return Ok(Attempt::Refused(e)),
            Err(e) => return Err(e),
        };
        let confirm = share["dev_confirm"]
            .as_str()
            .and_then(|c| B64.decode(c).ok())
            .ok_or_else(|| anyhow!("the plug did not confirm the login"))?;
        if confirm != agreed.dev_confirm {
            bail!("the plug did not prove it knows the password: refusing to talk to it");
        }
        let stok = share["stok"]
            .as_str()
            .or_else(|| share["sessionId"].as_str())
            .ok_or_else(|| anyhow!("the plug gave no session"))?
            .to_string();
        let seq = share["start_seq"].as_u64().unwrap_or(0) as u32;
        Ok(Attempt::Done(agreed.cipher, stok, seq))
    }

    /// One command; its `result`.
    pub fn call(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        let sealed = self
            .cipher
            .seal(seq, &tapo::command(method, params))
            .map_err(|e| anyhow!(e))?;
        let mut res = self
            .agent
            .post(format!("{}/stok={}/ds", self.base, self.stok))
            .header("Content-Type", "application/octet-stream")
            .send(&sealed[..])
            .map_err(|e| anyhow!("the plug does not answer: {e}"))?;
        let status = res.status().as_u16();
        let bytes = res.body_mut().read_to_vec()?;
        if status != 200 {
            bail!("the plug answered {status} to {method}");
        }
        let plain = self.cipher.open(&bytes).map_err(|e| anyhow!(e))?;
        let v: Value = serde_json::from_slice(&plain)?;
        match v["error_code"].as_i64().unwrap_or(0) {
            0 => Ok(v["result"].clone()),
            code => bail!("{}", tapo::error_text(code)),
        }
    }

    pub fn status(&mut self) -> Result<PlugStatus> {
        let info = self.call("get_device_info", None)?;
        // A plug that does not meter still switches.
        let energy = self.call("get_energy_usage", None).ok();
        Ok(tapo::plug_status(&info, energy.as_ref()))
    }

    pub fn set_on(&mut self, on: bool) -> Result<()> {
        self.call("set_device_info", Some(json!({ "device_on": on })))?;
        Ok(())
    }

    /// Have the plug itself switch on after `seconds`, and check that it took: when the
    /// connection to it goes through the machine it powers, this is what brings it back.
    pub fn arm_on_after(&mut self, seconds: u64) -> Result<()> {
        let _ = self.call(
            "remove_countdown_rules",
            Some(json!({ "remove_all": true })),
        );
        self.call(
            "add_countdown_rule",
            Some(json!({ "enable": true, "delay": seconds, "desired_states": { "on": true } })),
        )
        .context("the plug would not take a timer")?;
        let rules = self.call("get_countdown_rules", None)?;
        let armed = rules["rule_list"].as_array().is_some_and(|l| {
            l.iter()
                .any(|r| r["enable"] == true && r["desired_states"]["on"] == true)
        });
        if !armed {
            bail!("the plug did not keep the timer: not cutting the power");
        }
        Ok(())
    }
}

enum Attempt {
    Done(Cipher, String, u32),
    Refused(anyhow::Error),
}

/// How long the power stays off in a restart (a NUC needs a moment to notice).
const OFF_SECONDS: u64 = 8;
/// The plug's own timer when nothing else could switch it on again: long enough to get the
/// "off" through first.
const TIMER_SECONDS: u64 = 25;
const PLUG_EVERY_MS: u64 = 60_000;
const PLUG_VIA_EVERY_MS: u64 = 300_000;

/// A machine's plug, as the fleet shows it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PlugView {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// From the last look at it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<PlugStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Fleet {
    pub fn plug_credential_set(&self) -> bool {
        load_credential(&self.cfg.data_dir).is_some()
    }

    pub fn set_plug_password(&self, password: &str) -> Result<()> {
        save_password(&self.cfg.data_dir, password)?;
        let mut inner = self.lock();
        inner.plug_looked.clear();
        inner.plug_failures.clear();
        inner.plug_refused = false;
        Ok(())
    }

    pub fn forget_plug_password(&self) -> Result<()> {
        forget_credential(&self.cfg.data_dir)
    }

    /// `user@address` of the machine a plug is reached through, unless that is this machine.
    fn hop_of(&self, d: &Device) -> Result<Option<String>> {
        let Some(via) = d.plug.as_ref().and_then(|p| p.via.as_ref()) else {
            return Ok(None);
        };
        let hop = self
            .registry()
            .devices
            .get(via)
            .cloned()
            .ok_or_else(|| anyhow!("{via} is not a machine here"))?;
        let host = hop
            .zerotier
            .clone()
            .or(hop.address())
            .ok_or_else(|| anyhow!("{via} has no address"))?;
        if host
            .parse()
            .is_ok_and(|ip| kimo::services::network::local_ips().contains(&ip))
        {
            return Ok(None);
        }
        Ok(Some(format!("{}@{host}", hop.user.unwrap_or_default())))
    }

    fn open_plug(&self, id: &str, d: &Device) -> Result<Session> {
        let plug = d
            .plug
            .as_ref()
            .ok_or_else(|| anyhow!("{id} has no plug in the registry"))?;
        let credential = load_credential(&self.cfg.data_dir).ok_or_else(|| {
            anyhow!("no plug password yet: set your TP-Link account's with `kimo-fleet plug-login`")
        })?;
        let base = plug.address.as_str();
        // Tests point a plug at a stand-in on a local port.
        if let Some(port) = base.strip_prefix("127.0.0.1:") {
            return Session::connect(&format!("http://127.0.0.1:{port}"), &credential);
        }
        let hop = self.hop_of(d)?;
        Session::open(&self.cfg.ssh, base, hop.as_deref(), &credential)
    }

    /// Read a plug now and keep what it says.
    pub fn plug_status(&self, id: &str) -> Result<PlugView> {
        let (d, _) = self.device_of(id)?;
        let result = self.open_plug(id, &d).and_then(|mut s| s.status());
        let mut view = self.plug_view(id, &d);
        let now = now_ms();
        view.at = Some(now);
        let mut refused_now = false;
        let answer = match result {
            Ok(status) => {
                view.status = Some(status);
                view.error = None;
                let mut inner = self.lock();
                inner.plug_failures.remove(id);
                inner.plug_refused = false;
                Ok(view.clone())
            }
            Err(e) => {
                view.error = Some(format!("{e:#}"));
                let mut inner = self.lock();
                *inner.plug_failures.entry(id.to_string()).or_default() += 1;
                if e.downcast_ref::<tapo::PasswordRefused>().is_some() && !inner.plug_refused {
                    inner.plug_refused = true;
                    refused_now = true;
                }
                Err(e)
            }
        };
        self.lock().plugs.insert(id.to_string(), view);
        if refused_now {
            self.notify(
                Some(id),
                None,
                "the plugs refused the TP-Link password: kimo stopped reading them until you set it again",
            );
        }
        answer
    }

    pub(super) fn plug_view(&self, id: &str, d: &Device) -> PlugView {
        let mut v = self.lock().plugs.get(id).cloned().unwrap_or_default();
        if let Some(p) = &d.plug {
            v.address = p.address.clone();
            v.via = p.via.clone();
        }
        v
    }

    /// Read each plug every so often (watts for the map), on threads of their own.
    pub(super) fn look_at_plugs(self: &Arc<Self>, now: u64) {
        if !self.plug_credential_set() || self.lock().plug_refused {
            return;
        }
        let devices = self.registry().devices;
        for (id, d) in devices {
            let Some(plug) = &d.plug else { continue };
            let every = if plug.via.is_some() {
                PLUG_VIA_EVERY_MS
            } else {
                PLUG_EVERY_MS
            };
            {
                let mut inner = self.lock();
                // A plug that keeps failing is asked less often: 2, 4, 8, then 16 times later.
                let failures = inner.plug_failures.get(&id).copied().unwrap_or(0).min(4);
                let every = every << failures;
                if inner.plug_looked.get(&id).is_some_and(|t| now < t + every) {
                    continue;
                }
                inner.plug_looked.insert(id.clone(), now);
            }
            let me = self.clone();
            let spawned = std::thread::Builder::new()
                .name("fleet-plug".into())
                .spawn(move || {
                    if let Err(e) = me.plug_status(&id) {
                        tracing::debug!("plug of {id}: {e:#}");
                    }
                });
            if spawned.is_err() {
                tracing::warn!("cannot read a plug: no thread");
            }
        }
    }

    /// What should stop someone cutting this machine's power.
    fn cut_check(&self, id: &str, d: &Device, key: Option<&str>) -> CutCheck {
        let plug_via = d.plug.as_ref().and_then(|p| p.via.as_deref());
        let mut check = CutCheck {
            reached_through_itself: plug_via == Some(id),
            hosts_coordinator: self.hosts_coordinator(d),
            ..Default::default()
        };
        let jobs = {
            let inner = self.lock();
            inner
                .jobs
                .values()
                .filter(|j| j.state.on_device() && j.device.as_deref() == Some(id))
                .count()
        };
        if jobs > 0 {
            check.blockers.push(format!("{jobs} job(s) of the fleet"));
        }
        // Its own kimo knows best: agents, terminals, its own jobs, people logged in.
        let answer = key.filter(|_| d.has_kimo()).and_then(|k| {
            net::kimo(&d.kimo, Some(k))
                .call_timeout("GET", "/power", None, net::CALL_SECS)
                .ok()
        });
        match answer {
            Some(power) => {
                for b in power["blockers"].as_array().into_iter().flatten() {
                    check.blockers.push(
                        b["what"]
                            .as_str()
                            .or_else(|| b["kind"].as_str())
                            .unwrap_or("something is running")
                            .to_string(),
                    );
                }
            }
            None => {
                let up = d.address().is_some_and(|a| net::ping(&a));
                check.unknown_load = up;
            }
        }
        check
    }

    fn hosts_coordinator(&self, d: &Device) -> bool {
        let ips = kimo::services::network::local_ips();
        [
            d.zerotier.clone(),
            d.tailscale.clone(),
            crate::core::registry::host_of(&d.kimo),
        ]
        .into_iter()
        .flatten()
        .any(|a| a.parse().is_ok_and(|ip| ips.contains(&ip)))
    }

    /// Switch a machine's plug on or off, or restart it. Cutting power loses unsaved work, so
    /// it is refused while anything runs there unless `force`; and it is refused outright when
    /// nothing could bring the machine back.
    pub fn plug_action(
        self: &Arc<Self>,
        id: &str,
        action: PlugAction,
        force: bool,
    ) -> Result<PlugView> {
        let (d, key) = self.device_of(id)?;
        let check = self.cut_check(id, &d, key.as_deref());
        tapo::may_act(action, &check, force)?;
        let mut session = self.open_plug(id, &d)?;
        match action {
            PlugAction::On => {
                session.set_on(true)?;
                self.event(Some(id), None, "power", "plug on");
            }
            PlugAction::Off => {
                session.set_on(false)?;
                self.event(
                    Some(id),
                    None,
                    "power",
                    if force {
                        "plug off (forced)"
                    } else {
                        "plug off"
                    },
                );
            }
            PlugAction::Cycle if check.reached_through_itself || check.hosts_coordinator => {
                // The way to the plug goes down with the machine: the plug brings itself back.
                session.arm_on_after(TIMER_SECONDS)?;
                session.set_on(false)?;
                self.event(
                    Some(id),
                    None,
                    "power",
                    &format!("plug off; its own timer switches it on in {TIMER_SECONDS} s"),
                );
            }
            PlugAction::Cycle => {
                session.set_on(false)?;
                self.event(Some(id), None, "power", "plug off for a restart");
                std::thread::sleep(std::time::Duration::from_secs(
                    self.cfg.plug_off_seconds.unwrap_or(OFF_SECONDS),
                ));
                self.switch_back_on(id, &d)?;
                self.event(Some(id), None, "power", "plug on again");
            }
        }
        self.lock().looked.remove(id);
        if action == PlugAction::Cycle && (check.reached_through_itself || check.hosts_coordinator)
        {
            // Its answers just stopped, and this connection may have too.
            let mut view = self.plug_view(id, &d);
            view.at = Some(now_ms());
            return Ok(view);
        }
        self.plug_status(id)
    }

    /// Switch a plug on again after cutting it, trying a few times: a session is short-lived on
    /// these plugs, and a machine left without power is worse than a slow restart.
    fn switch_back_on(&self, id: &str, d: &Device) -> Result<()> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.open_plug(id, d).and_then(|mut s| s.set_on(true)) {
                Ok(()) => return Ok(()),
                Err(e) if attempt >= 3 => {
                    self.notify(
                        Some(id),
                        None,
                        "its plug did not switch on again: switch it on by hand",
                    );
                    return Err(e.context("switching it on again"));
                }
                Err(e) => {
                    tracing::warn!("plug of {id}, switching on again (try {attempt}): {e:#}");
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        }
    }

    /// Bring up a machine set to start when power returns. A plug that is on and feeding a
    /// machine that is off has to go off first, but only once nothing says the machine is up:
    /// a machine wrongly seen as off must not lose its power.
    pub(super) fn wake_by_plug(&self, id: &str, d: &Device) -> Result<()> {
        debug_assert_eq!(d.power.wake, Wake::Plug);
        let mut session = self.open_plug(id, d)?;
        if session.status()?.on {
            let key = self.device_of(id).ok().and_then(|(_, key)| key);
            let check = self.cut_check(id, d, key.as_deref());
            tapo::may_act(PlugAction::Cycle, &check, false)
                .context("it may be up already, so its plug was left on")?;
            session.set_on(false)?;
            std::thread::sleep(std::time::Duration::from_secs(
                self.cfg.plug_off_seconds.unwrap_or(OFF_SECONDS),
            ));
            self.switch_back_on(id, d)?;
            self.event(
                Some(id),
                None,
                "power",
                "plug off and on: it starts when power returns",
            );
        } else {
            session.set_on(true)?;
            self.event(
                Some(id),
                None,
                "power",
                "plug on: it starts when power returns",
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tapo::device::Device as FakeDevice;
    use crate::core::tapo::Agreed;
    use std::io::{BufRead, BufReader, Read};
    use std::sync::Mutex;

    /// A plug on a local port: the real login and encryption, a relay that remembers.
    #[derive(Default)]
    struct Plug {
        on: bool,
        calls: Vec<String>,
        countdown: bool,
        refuse_timer: bool,
    }

    const PASSWORD: &str = "correct horse";

    fn stand_in(plug: Arc<Mutex<Plug>>) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let device: Arc<Mutex<Option<FakeDevice>>> = Arc::default();
        let agreed: Arc<Mutex<Option<Agreed>>> = Arc::default();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let (device, agreed, plug) = (device.clone(), agreed.clone(), plug.clone());
                // A connection each: sessions keep theirs open.
                std::thread::spawn(move || {
                    while let Some((path, body)) = read_request(&mut stream) {
                        let reply: Vec<u8> = if path == "/" {
                            let v: Value = serde_json::from_slice(&body).unwrap();
                            let p = &v["params"];
                            match p["sub_method"].as_str() {
                                Some("pake_register") => {
                                    let user_random =
                                        B64.decode(p["user_random"].as_str().unwrap()).unwrap();
                                    let (salt, dev_random) = ([9u8; 16], [8u8; 32]);
                                    let d = FakeDevice::new(
                                        &tapo::credential(PASSWORD),
                                        &salt,
                                        100,
                                        &user_random,
                                        &dev_random,
                                    );
                                    let out = json!({ "error_code": 0, "result": {
                                        "extra_crypt": { "params": { "passwd_id": 2 }, "type": "password_shadow" },
                                        "dev_random": B64.encode(dev_random), "dev_salt": B64.encode(salt),
                                        "dev_share": B64.encode(&d.share), "iterations": 100, "cipher_suites": 1,
                                    }});
                                    *device.lock().unwrap() = Some(d);
                                    serde_json::to_vec(&out).unwrap()
                                }
                                Some("pake_share") => {
                                    let share =
                                        B64.decode(p["user_share"].as_str().unwrap()).unwrap();
                                    let a = device.lock().unwrap().as_ref().unwrap().finish(&share);
                                    let given =
                                        B64.decode(p["user_confirm"].as_str().unwrap()).unwrap();
                                    let out = if given == a.user_confirm {
                                        json!({ "error_code": 0, "result": {
                                            "dev_confirm": B64.encode(a.dev_confirm), "stok": "abc", "start_seq": 5,
                                        }})
                                    } else {
                                        json!({ "error_code": -1501 })
                                    };
                                    *agreed.lock().unwrap() = Some(a);
                                    serde_json::to_vec(&out).unwrap()
                                }
                                _ => serde_json::to_vec(&json!({ "error_code": 1003 })).unwrap(),
                            }
                        } else {
                            assert_eq!(path, "/stok=abc/ds");
                            let held = agreed.lock().unwrap();
                            let cipher = &held.as_ref().unwrap().cipher;
                            let seq = u32::from_be_bytes(body[..4].try_into().unwrap());
                            let plain = cipher.open(&body).unwrap();
                            let v: Value = serde_json::from_slice(&plain).unwrap();
                            let method = v["method"].as_str().unwrap().to_string();
                            let mut plug = plug.lock().unwrap();
                            plug.calls.push(method.clone());
                            let result = match method.as_str() {
                                "get_device_info" => {
                                    json!({ "device_on": plug.on, "model": "P110", "fw_ver": "1.4.3" })
                                }
                                "get_energy_usage" => {
                                    json!({ "current_power": 41000, "today_energy": 12 })
                                }
                                "set_device_info" => {
                                    plug.on = v["params"]["device_on"] == true;
                                    let state = plug.on;
                                    plug.calls.push(format!("relay {state}"));
                                    json!({})
                                }
                                "add_countdown_rule" => {
                                    plug.countdown = !plug.refuse_timer;
                                    json!({})
                                }
                                "get_countdown_rules" => json!({ "rule_list": if plug.countdown {
                                json!([{ "enable": true, "desired_states": { "on": true } }])
                            } else { json!([]) } }),
                                _ => json!({}),
                            };
                            cipher
                                .seal(
                                    seq,
                                    &serde_json::to_vec(
                                        &json!({ "error_code": 0, "result": result }),
                                    )
                                    .unwrap(),
                                )
                                .unwrap()
                        };
                        let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                        reply.len()
                    );
                        if stream.write_all(head.as_bytes()).is_err()
                            || stream.write_all(&reply).is_err()
                        {
                            break;
                        }
                    }
                });
            }
        });
        port
    }

    fn read_request(stream: &mut TcpStream) -> Option<(String, Vec<u8>)> {
        let mut reader = BufReader::new(stream.try_clone().ok()?);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let path = line.split_whitespace().nth(1)?.to_string();
        let mut length = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h).ok()?;
            if h.trim().is_empty() {
                break;
            }
            if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().ok()?;
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).ok()?;
        Some((path, body))
    }

    fn fleet_with(dir: &Path, yaml: &str) -> Arc<Fleet> {
        std::fs::write(dir.join("devices.yaml"), yaml).unwrap();
        let mut cfg = super::super::Config::new(dir.to_path_buf(), "test".into());
        cfg.plug_off_seconds = Some(0);
        cfg.mesh_every_ms = 0;
        Fleet::open(cfg).unwrap()
    }

    #[test]
    fn the_login_and_a_wrong_password() {
        let plug = Arc::new(Mutex::new(Plug::default()));
        let base = format!("http://127.0.0.1:{}", stand_in(plug.clone()));
        let mut s = Session::connect(&base, &tapo::credential(PASSWORD)).unwrap();
        let status = s.status().unwrap();
        assert!(!status.on);
        assert_eq!(status.watts, Some(41.0));
        s.set_on(true).unwrap();
        assert!(s.status().unwrap().on);
        // It speaks only after proving it knows the password; a wrong one gets nowhere.
        let e = Session::connect(&base, &tapo::credential("wrong"))
            .err()
            .unwrap();
        assert!(e.to_string().contains("refused the password"), "{e}");
    }

    #[test]
    fn a_refused_password_stops_the_background_reads() {
        let dir = tempfile::tempdir().unwrap();
        let plug = Arc::new(Mutex::new(Plug::default()));
        let port = stand_in(plug.clone());
        let fleet = fleet_with(
            dir.path(),
            &format!("devices:\n  nuc:\n    zerotier: 192.0.2.1\n    user: me\n    plug: {{ address: \"127.0.0.1:{port}\" }}\n"),
        );
        fleet.set_plug_password("not the one").unwrap();
        let e = fleet.plug_status("nuc").unwrap_err();
        assert!(e.downcast_ref::<tapo::PasswordRefused>().is_some(), "{e:#}");
        assert!(fleet.lock().plug_refused);
        // The minute's reads no longer log in: a lockout is worse than a missing reading.
        fleet.lock().plug_looked.clear();
        fleet.look_at_plugs(now_ms());
        assert!(fleet.lock().plug_looked.is_empty());
        // A new password starts them again.
        fleet.set_plug_password(PASSWORD).unwrap();
        assert!(!fleet.lock().plug_refused);
        assert!(fleet.plug_status("nuc").is_ok());
        assert!(fleet.lock().plug_failures.is_empty());
    }

    #[test]
    fn restarting_guards_and_timers() {
        let dir = tempfile::tempdir().unwrap();
        let plug = Arc::new(Mutex::new(Plug {
            on: true,
            ..Default::default()
        }));
        let port = stand_in(plug.clone());
        // No kimo there, and 127.0.0.1 answers pings: nothing can say what runs on it.
        let fleet = fleet_with(
            dir.path(),
            &format!("devices:\n  nuc:\n    zerotier: 127.0.0.1\n    user: me\n    plug: {{ address: \"127.0.0.1:{port}\" }}\n"),
        );
        let no_password = fleet.plug_status("nuc").unwrap_err();
        assert!(no_password.to_string().contains("plug-login"));
        fleet.set_plug_password(PASSWORD).unwrap();
        assert!(fleet.plug_status("nuc").unwrap().status.unwrap().on);

        // This machine hosts the coordinator here (127.0.0.1): off is out of the question.
        let off = fleet.plug_action("nuc", PlugAction::Off, true).unwrap_err();
        assert!(off.to_string().contains("only be restarted"), "{off}");
        assert!(plug.lock().unwrap().on);

        // A restart goes through the plug's own timer, and only once the plug holds it.
        plug.lock().unwrap().refuse_timer = true;
        let e = fleet
            .plug_action("nuc", PlugAction::Cycle, true)
            .unwrap_err();
        assert!(e.to_string().contains("did not keep the timer"), "{e}");
        assert!(plug.lock().unwrap().on, "the power stayed on");
        plug.lock().unwrap().refuse_timer = false;
        fleet.plug_action("nuc", PlugAction::Cycle, true).unwrap();
        let p = plug.lock().unwrap();
        assert!(!p.on, "off, for the timer to bring back");
        let timer = p
            .calls
            .iter()
            .position(|c| c == "add_countdown_rule")
            .unwrap();
        let cut = p.calls.iter().rposition(|c| c == "relay false").unwrap();
        assert!(timer < cut);
    }

    #[test]
    fn an_ordinary_restart_is_off_then_on() {
        let dir = tempfile::tempdir().unwrap();
        let plug = Arc::new(Mutex::new(Plug {
            on: true,
            ..Default::default()
        }));
        let port = stand_in(plug.clone());
        // An address that answers nothing: off, and not this machine.
        let fleet = fleet_with(
            dir.path(),
            &format!("devices:\n  nuc:\n    zerotier: 192.0.2.1\n    user: me\n    plug: {{ address: \"127.0.0.1:{port}\" }}\n    power: {{ wake: plug }}\n"),
        );
        fleet.set_plug_password(PASSWORD).unwrap();
        let v = fleet.plug_action("nuc", PlugAction::Cycle, false).unwrap();
        assert!(v.status.unwrap().on);
        {
            let p = plug.lock().unwrap();
            let relay: Vec<_> = p.calls.iter().filter(|c| c.starts_with("relay")).collect();
            assert_eq!(relay, ["relay false", "relay true"]);
            assert!(!p.calls.contains(&"add_countdown_rule".to_string()));
        }
        // Waking through the plug when it is already on: off first, or nothing boots.
        plug.lock().unwrap().calls.clear();
        let (d, _) = fleet.device_of("nuc").unwrap();
        fleet.wake_by_plug("nuc", &d).unwrap();
        let p = plug.lock().unwrap();
        let relay: Vec<_> = p.calls.iter().filter(|c| c.starts_with("relay")).collect();
        assert_eq!(relay, ["relay false", "relay true"]);
    }

    #[test]
    fn the_password_is_never_written() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_credential(dir.path()), None);
        save_password(dir.path(), "correct horse").unwrap();
        let text = std::fs::read_to_string(dir.path().join(CREDENTIAL_FILE)).unwrap();
        assert!(!text.contains("correct"));
        assert_eq!(
            load_credential(dir.path()),
            Some(tapo::credential("correct horse"))
        );
        let mode = std::fs::metadata(dir.path().join(CREDENTIAL_FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        forget_credential(dir.path()).unwrap();
        assert_eq!(load_credential(dir.path()), None);
        forget_credential(dir.path()).unwrap();
        assert!(save_password(dir.path(), "").is_err());
    }
}
