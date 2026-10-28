//! Adding a machine to the fleet without sitting at it. Over SSH (your keys or Tailscale SSH;
//! never a password) the coordinator installs kimo when it is missing, runs it as a service
//! bound to the fleet's network and gets a pairing code made there. Then it pairs one way,
//! reads the machine's facts, picks its hardware model and role, and writes its entry to
//! `devices.d/<id>.yaml`. A machine whose kimo already runs can join with a code instead.

use super::{now_ms, Fleet};
use crate::core::discover::id_from;
use crate::core::hardware::{self, Clues};
use crate::core::registry::{self, valid_id};
use crate::core::setup::{self, Setup};
use crate::services::net;
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long the setup over SSH may take (a download and an install).
const SETUP_LIMIT: Duration = Duration::from_secs(900);

/// What to add.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Adopt {
    /// Its address on the fleet's network (Tailscale or ZeroTier IP, or a name).
    pub address: String,
    /// Its id in the fleet (default: from its name or address).
    #[serde(default)]
    pub id: Option<String>,
    /// Its hardware model (default: guessed from its facts).
    #[serde(default)]
    pub model: Option<String>,
    /// Its role (default: its model's).
    #[serde(default)]
    pub role: Option<String>,
    /// `tailscale`, `zerotier` or an address: where its kimo listens (default: the network
    /// its address is on).
    #[serde(default)]
    pub network: Option<String>,
    /// The SSH user (default: the coordinator's own).
    #[serde(default)]
    pub ssh_user: Option<String>,
    /// Install kimo again even if it is there (to update it).
    #[serde(default)]
    pub reinstall: bool,
    /// A pairing code shown by its kimo: joins without SSH.
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Running,
    Done,
    Failed,
}

/// A machine being added, step by step.
#[derive(Debug, Clone, Serialize)]
pub struct Adoption {
    pub id: String,
    pub address: String,
    pub state: State,
    pub steps: Vec<String>,
    /// What you may want to do on the machine (BIOS settings, linger).
    pub notes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// Is `ip` a Tailscale address (100.64.0.0/10)?
fn tailscale_ip(ip: &str) -> bool {
    ip.parse::<std::net::Ipv4Addr>()
        .is_ok_and(|ip| ip.octets()[0] == 100 && (64..128).contains(&ip.octets()[1]))
}

impl Fleet {
    pub fn adoptions(&self) -> Vec<Adoption> {
        let mut list: Vec<Adoption> = self
            .adoptions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        list.sort_by_key(|a| std::cmp::Reverse(a.started_at));
        list
    }

    pub fn adoption(&self, id: &str) -> Result<Adoption> {
        self.adoptions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown machine being added: {id}"))
    }

    fn adoption_mut(&self, id: &str, f: impl FnOnce(&mut Adoption)) {
        if let Some(a) = self
            .adoptions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(id)
        {
            f(a);
        }
    }

    fn step(&self, id: &str, text: impl Into<String>) {
        let text = text.into();
        tracing::info!("adding {id}: {text}");
        self.adoption_mut(id, |a| a.steps.push(text));
    }

    /// Start adding a machine (in the background); follow it with [`Fleet::adoption`].
    pub fn adopt(self: &Arc<Self>, req: Adopt) -> Result<Adoption> {
        let address = req.address.trim().to_string();
        if address.is_empty() || !setup::safe_word(&address) {
            bail!("the address is an IP address or a name, like 100.64.0.2");
        }
        let id = req
            .id
            .clone()
            .filter(|i| !i.trim().is_empty())
            .unwrap_or_else(|| id_from(&address.replace('.', "-")));
        if !valid_id(&id) {
            bail!("'{id}' is not a machine id (letters, digits, - and _)");
        }
        if self.lock().registry.devices.contains_key(&id) {
            bail!("{id} is in the fleet already");
        }
        if let Some(m) = &req.model {
            hardware::model(m).ok_or_else(|| anyhow!("unknown hardware model {m}"))?;
        }
        if let Some(r) = &req.role {
            hardware::role(r).ok_or_else(|| anyhow!("unknown role {r}"))?;
        }
        if let Some(u) = &req.ssh_user {
            if !setup::safe_word(u) {
                bail!("'{u}' is not a user name");
            }
        }
        let adoption = Adoption {
            id: id.clone(),
            address: address.clone(),
            state: State::Running,
            steps: vec![],
            notes: vec![],
            error: None,
            model: req.model.clone(),
            role: req.role.clone(),
            started_at: now_ms(),
            ended_at: None,
        };
        {
            let mut all = self.adoptions.lock().unwrap_or_else(|e| e.into_inner());
            if all.get(&id).is_some_and(|a| a.state == State::Running) {
                bail!("{id} is being added already");
            }
            all.insert(id.clone(), adoption.clone());
        }
        let me = self.clone();
        std::thread::Builder::new()
            .name("fleet-adopt".into())
            .spawn(move || {
                let result = me.adopt_now(&id, &address, &req);
                let failed = result.as_ref().err().map(|e| format!("{e:#}"));
                me.adoption_mut(&id, |a| {
                    a.state = if failed.is_some() {
                        State::Failed
                    } else {
                        State::Done
                    };
                    a.error = failed.clone();
                    a.ended_at = Some(now_ms());
                });
                match failed {
                    Some(e) => me.event(Some(&id), None, "adopt", &format!("not added: {e}")),
                    None => {
                        let a = me.adoption(&id).ok();
                        let role = a.and_then(|a| a.role).unwrap_or_default();
                        me.notify(Some(&id), None, &format!("joined the fleet as {role}"));
                    }
                }
            })?;
        Ok(adoption)
    }

    fn adopt_now(&self, id: &str, address: &str, req: &Adopt) -> Result<()> {
        let port = self.cfg.kimo_port;
        let url = format!("http://{address}:{port}");
        let bind = match req.network.as_deref() {
            Some(n) if setup::safe_word(n) => n.to_string(),
            Some(n) => bail!("'{n}' is not a network"),
            None if tailscale_ip(address) => "tailscale".into(),
            None => {
                let zt = kimo::services::node::nics()
                    .into_iter()
                    .filter(|n| n.name.starts_with("zt"))
                    .any(|n| {
                        let (Ok(mine), Ok(theirs)) = (
                            n.ipv4.parse::<std::net::Ipv4Addr>(),
                            address.parse::<std::net::Ipv4Addr>(),
                        ) else {
                            return false;
                        };
                        let mask = u32::MAX << (32 - u32::from(n.prefix.clamp(1, 32)));
                        u32::from(mine) & mask == u32::from(theirs) & mask
                    });
                if zt {
                    "zerotier".into()
                } else {
                    address.to_string()
                }
            }
        };
        let code = match req.code.as_deref().filter(|c| !c.trim().is_empty()) {
            Some(c) => {
                self.step(id, "pairing with the code you gave");
                c.trim().to_string()
            }
            None => self.set_up_over_ssh(id, address, req, &bind)?,
        };
        self.step(id, "pairing one way: the coordinator gets a fleet key");
        let token = self.pair_at(&url, &code)?;
        let client = net::kimo(&url, Some(&token));
        let facts = client
            .call_timeout("GET", "/node", None, 15)
            .context("reading its facts")?;
        let name = facts["host"].as_str().unwrap_or(address).to_string();
        let model = match &req.model {
            Some(m) => hardware::model(m).expect("checked"),
            None => hardware::guess(&Clues {
                name: &name,
                os: facts["os"].as_str().unwrap_or_default(),
                facts: Some(&facts),
            }),
        };
        let role = req.role.clone().unwrap_or_else(|| model.role.to_string());
        self.step(
            id,
            format!(
                "{} ({}) as {role}",
                model.name,
                facts["model"].as_str().unwrap_or("?")
            ),
        );
        let mine: Vec<(std::net::Ipv4Addr, u8)> = kimo::services::node::nics()
            .into_iter()
            .filter(|n| !n.vpn)
            .filter_map(|n| Some((n.ipv4.parse().ok()?, n.prefix)))
            .collect();
        let wol = hardware::wol_card(&facts, &mine);
        if model.wake == registry::Wake::Wol {
            match &wol {
                Some((mac, broadcast)) => self.step(id, format!("Wake-on-LAN through {mac} on {broadcast}")),
                None => self.adoption_mut(id, |a| a.notes.push(
                    "it has no wired card on this coordinator's network, so it is never switched off (it could not be woken)".into(),
                )),
            }
        }
        let device = hardware::entry(model, &role, &url, Some(&facts), wol);
        let file = self.added_dir().join(format!("{id}.yaml"));
        std::fs::create_dir_all(self.added_dir())?;
        let note = format!(
            "{id}: {} added from the Fleet tab ({})",
            model.name,
            facts["model"].as_str().unwrap_or("")
        );
        std::fs::write(&file, registry::render_added(id, &device, &note))?;
        self.keep_key(id, token)?;
        if let Err(e) = self.reload() {
            let _ = std::fs::remove_file(&file);
            let _ = self.reload();
            return Err(e.context("its entry"));
        }
        self.adoption_mut(id, |a| {
            a.model = Some(model.id.to_string());
            a.role = Some(role.clone());
            a.notes.push(model.notes.to_string());
        });
        self.step(id, "in the fleet");
        Ok(())
    }

    /// Install and start kimo over SSH; returns the pairing code made there.
    fn set_up_over_ssh(&self, id: &str, address: &str, req: &Adopt, bind: &str) -> Result<String> {
        let target = match &req.ssh_user {
            Some(u) => format!("{u}@{address}"),
            None => address.to_string(),
        };
        self.step(id, format!("connecting over SSH to {target}"));
        let script = setup::script(&Setup {
            install: req.reinstall,
            version: self.cfg.kimo_version.as_deref(),
            bind,
            name: id,
            port: self.cfg.kimo_port,
            install_url: &self.cfg.install_url,
        });
        let mut child = Command::new(&self.cfg.ssh)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "StrictHostKeyChecking=accept-new",
                &target,
                "sh",
                "-s",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("starting {}", self.cfg.ssh))?;
        child
            .stdin
            .take()
            .expect("piped")
            .write_all(script.as_bytes())?;
        let mut err = child.stderr.take().expect("piped");
        let errors = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        // Its steps as they come.
        let out = child.stdout.take().expect("piped");
        let started = Instant::now();
        let mut all = String::new();
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let said = setup::read_output(&line);
            for s in said.steps {
                self.step(id, s);
            }
            for n in said.notes {
                self.adoption_mut(id, |a| a.notes.push(n.clone()));
            }
            all.push_str(&line);
            all.push('\n');
            if started.elapsed() > SETUP_LIMIT {
                let _ = child.kill();
                break;
            }
        }
        let status = child.wait()?;
        let stderr = errors.join().unwrap_or_default();
        let said = setup::read_output(&all);
        if let Some(e) = said.error {
            bail!(e);
        }
        if !status.success() || said.code.is_none() {
            // 255: SSH itself (no key, no route, host key changed).
            if status.code() == Some(255) {
                let last = stderr
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .trim();
                bail!(
                    "cannot log in to {target} over SSH ({last}). Give the coordinator access once: \
                     ssh-copy-id {target} from this machine, or Tailscale SSH there (tailscale set --ssh)"
                );
            }
            let last = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no pairing code");
            bail!("setting kimo up on {target} failed: {}", last.trim());
        }
        Ok(said.code.expect("checked"))
    }

    /// Remove a machine added from the Fleet tab: its entry and its key (the key also stops
    /// working on the machine).
    pub fn remove(&self, id: &str) -> Result<()> {
        let file = self.added_dir().join(format!("{id}.yaml"));
        if !valid_id(id) || !file.exists() {
            bail!("{id} is in devices.yaml: remove it there");
        }
        let busy = self
            .lock()
            .jobs
            .values()
            .any(|j| j.state.on_device() && j.device.as_deref() == Some(id));
        if busy {
            bail!("jobs run on {id}: cancel them first");
        }
        let (d, key) = self.device_of(id)?;
        if let Some(key) = key {
            // The machine forgets the key too, when it answers.
            let _ = net::kimo(&d.kimo, Some(&key)).call_timeout("DELETE", "/fleet-key", None, 5);
            self.forget_key(id)?;
        }
        std::fs::remove_file(&file)?;
        self.reload()?;
        self.event(Some(id), None, "adopt", "removed from the fleet");
        Ok(())
    }

    /// The coordinator's public SSH key, to give to machines it should reach.
    pub fn ssh_key(&self) -> Option<String> {
        let home = kimo::config::user_home().join(".ssh");
        ["id_ed25519.pub", "id_ecdsa.pub", "id_rsa.pub"]
            .iter()
            .find_map(|f| std::fs::read_to_string(home.join(f)).ok())
            .map(|k| k.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tailscale_addresses() {
        assert!(super::tailscale_ip("100.64.0.2"));
        assert!(super::tailscale_ip("100.127.255.1"));
        assert!(!super::tailscale_ip("100.128.0.1"));
        assert!(!super::tailscale_ip("10.147.17.21"));
    }
}
