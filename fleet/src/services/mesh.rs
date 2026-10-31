//! The SSH mesh, carried out: log in to every machine, have it try all the others, and report;
//! on request, set up shortcuts and keys so the machines match the registry's rule.

use super::{now_ms, Fleet};
use crate::core::mesh::{self, Link, Node};
use anyhow::{anyhow, bail, Context, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The last check.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<u64>,
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// Machines this coordinator could not log in to: `(id, why)`.
    pub untested: Vec<Untested>,
    /// Links a restricted machine should not have: the one thing that must not happen.
    pub violations: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Untested {
    pub id: String,
    pub why: String,
}

/// Run a script on a machine over SSH. Bounded: a machine that hangs loses, nothing else does.
fn run_on(
    ssh: &str,
    target: &str,
    env: &[(&str, String)],
    script: &str,
    args: &[String],
    secs: u64,
) -> Result<String> {
    let mut remote = String::new();
    for (k, v) in env {
        remote.push_str(&format!("{k}='{v}'; export {k}\n"));
    }
    remote.push_str(script);
    let mut command = vec!["sh".to_string(), "-s".to_string(), "--".to_string()];
    command.extend(args.iter().map(|a| shell_quote(a)));
    let mut child = Command::new(ssh)
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=6",
            "-o",
            "StrictHostKeyChecking=accept-new",
            "-o",
            "LogLevel=ERROR",
            target,
        ])
        .args(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("running {ssh}"))?;
    wait_output(&mut child, remote.as_bytes(), secs, target)
}

fn run_here(env: &[(&str, String)], script: &str, args: &[String], secs: u64) -> Result<String> {
    let mut child = Command::new("sh")
        .args(["-s", "--"])
        .args(args)
        .envs(env.iter().map(|(k, v)| (*k, v.as_str())))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running sh")?;
    wait_output(&mut child, script.as_bytes(), secs, "this machine")
}

fn wait_output(
    child: &mut std::process::Child,
    input: &[u8],
    secs: u64,
    what: &str,
) -> Result<String> {
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(input).ok();
    }
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut out);
            }
            String::from_utf8_lossy(&out).to_string()
        })
    };
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let deadline = Instant::now() + Duration::from_secs(secs);
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("{what} took too long");
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    if !status.success() {
        let why = err
            .lines()
            .rev()
            .find(|l| !l.contains("manpath"))
            .unwrap_or("it failed");
        bail!("{why}");
    }
    Ok(out)
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Fleet {
    fn is_here(&self, n: &Node) -> bool {
        let ips = kimo::services::network::local_ips();
        [&n.zerotier, &n.tailscale]
            .into_iter()
            .flatten()
            .any(|a| a.parse().is_ok_and(|ip| ips.contains(&ip)))
    }

    /// Run a script on a machine: here when it is this one, else over SSH on either network.
    fn on_node(
        &self,
        n: &Node,
        env: &[(&str, String)],
        script: &str,
        args: &[String],
        secs: u64,
    ) -> Result<String> {
        if self.is_here(n) {
            return run_here(env, script, args, secs);
        }
        run_on(&self.cfg.ssh, &n.target(), env, script, args, secs)
    }

    pub fn mesh_report(&self) -> Report {
        self.mesh.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn mesh_nodes(&self) -> Vec<Node> {
        mesh::nodes(&self.registry())
    }

    /// Start a check in the background (no-op if one is running). Follow it with `mesh_report`.
    pub fn mesh_check(self: &Arc<Self>) {
        {
            let mut r = self.mesh.lock().unwrap_or_else(|e| e.into_inner());
            if r.running {
                return;
            }
            r.running = true;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new()
            .name("fleet-mesh".into())
            .spawn(move || {
                let report = me.run_check();
                let mut problems = report.violations.clone();
                problems.extend(
                    report
                        .links
                        .iter()
                        .filter(|l| !l.as_intended)
                        .map(|l| format!("{} → {}", l.from, l.to)),
                );
                *me.mesh.lock().unwrap_or_else(|e| e.into_inner()) = report;
                tracing::info!("mesh checked: {} links not as intended", problems.len());
            });
        if spawned.is_err() {
            self.mesh.lock().unwrap_or_else(|e| e.into_inner()).running = false;
        }
    }

    fn run_check(self: &Arc<Self>) -> Report {
        let nodes = self.mesh_nodes();
        let handles: Vec<_> = nodes
            .iter()
            .cloned()
            .map(|n| {
                let (me, all) = (self.clone(), nodes.clone());
                std::thread::spawn(move || {
                    let args = mesh::probe_args(&n, &all);
                    let out = me.on_node(&n, &[], mesh::PROBE, &args, 60);
                    (n, out)
                })
            })
            .collect();
        let mut report = Report {
            running: false,
            at: Some(now_ms()),
            nodes: nodes.clone(),
            ..Default::default()
        };
        for h in handles {
            let Ok((n, out)) = h.join() else { continue };
            match out {
                Ok(text) => report.links.extend(mesh::parse_probe(&n, &nodes, &text)),
                Err(e) => report.untested.push(Untested {
                    id: n.id.clone(),
                    why: format!("{e:#}"),
                }),
            }
        }
        report.violations = report
            .links
            .iter()
            .filter(|l| mesh::breaks_rule(l.expect, l.found))
            .map(|l| format!("{} logs in to {} without a password", l.from, l.to))
            .collect();
        report
    }

    fn mesh_keys_path(&self) -> std::path::PathBuf {
        self.cfg.data_dir.join("mesh-keys.json")
    }

    /// Make the machines match the registry: shortcuts everywhere, a key on each machine that
    /// may log in to others, each key accepted where the rule says, and keys the mesh handed
    /// out earlier taken back from where they no longer belong (a machine removed or now
    /// restricted). Keys it did not hand out are left alone. Returns a line per machine.
    pub fn mesh_apply(self: &Arc<Self>) -> Result<Value> {
        let nodes = self.mesh_nodes();
        if nodes.is_empty() {
            bail!("no machine has user and an address in the registry");
        }
        let config = B64.encode(mesh::ssh_config(&nodes));
        let mut keys: BTreeMap<String, String> = BTreeMap::new();
        let mut lines: Vec<String> = Vec::new();
        let mut reached: Vec<&Node> = Vec::new();
        for n in &nodes {
            let mut env = vec![("CONFIG", config.clone())];
            if n.restricted {
                env.push(("NOKEY", "1".to_string()));
            }
            match self.on_node(n, &env, mesh::SETUP, &[], 40) {
                Ok(out) => {
                    if let Some(k) = out.lines().find_map(|l| l.strip_prefix("KEY ")) {
                        keys.insert(n.id.clone(), k.trim().to_string());
                    }
                    reached.push(n);
                }
                Err(e) => lines.push(format!("{}: not reached ({e:#})", n.id)),
            }
        }
        // Everything handed out before, and now: what may need taking back.
        let path = self.mesh_keys_path();
        let mut known: BTreeMap<String, String> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for (id, k) in &keys {
            known.insert(id.clone(), k.clone());
        }
        for n in &reached {
            let (add, remove) = mesh::keys_for(n, &nodes, &keys, &known);
            let lines_of = |list: &[&str]| {
                B64.encode(list.iter().map(|k| format!("{k}\n")).collect::<String>())
            };
            let env = [("ADD", lines_of(&add)), ("REMOVE", lines_of(&remove))];
            match self.on_node(n, &env, mesh::AUTHORIZE, &[], 30) {
                Ok(_) => lines.push(format!("{}: {} keys accepted", n.id, add.len())),
                Err(e) => lines.push(format!("{}: keys not set ({e:#})", n.id)),
            }
        }
        // Machines gone from the mesh stay here only until they are out of every list.
        let current: Vec<&String> = nodes.iter().map(|n| &n.id).collect();
        if reached.len() == nodes.len() {
            known.retain(|id, _| current.contains(&id));
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&known)?)
            .map_err(|e| anyhow!("writing {}: {e}", path.display()))?;
        self.event(
            None,
            None,
            "mesh",
            &format!("shortcuts and keys set on {} machines", reached.len()),
        );
        self.mesh_check();
        Ok(serde_json::json!({ "ok": true, "machines": lines }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_run_here_with_arguments() {
        let out = run_here(
            &[("A", "1".to_string())],
            "echo \"$A $1\"",
            &["two words".to_string()],
            5,
        )
        .unwrap();
        assert_eq!(out.trim(), "1 two words");
    }

    #[test]
    fn a_failing_script_says_why() {
        let e = run_here(&[], "echo nope >&2; exit 3", &[], 5).unwrap_err();
        assert_eq!(e.to_string(), "nope");
    }

    #[test]
    fn a_hanging_script_is_stopped() {
        let t = Instant::now();
        let e = run_here(&[], "sleep 30", &[], 1).unwrap_err();
        assert!(e.to_string().contains("too long"));
        assert!(t.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }
}
