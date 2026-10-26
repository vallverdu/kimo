//! Listening TCP ports on this machine, each with the agent or terminal that opened it (a dev
//! server started by an agent belongs to that agent's session).

use crate::core::ports::{merge, parse_lsof, parse_netstat, parse_ss, Listener};
use crate::services::network::run_for;
use crate::services::sessions::SessionInfo;
use crate::services::stats;
use serde::Serialize;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct Port {
    #[serde(flatten)]
    pub listener: Listener,
    /// The kimo session whose processes listen here, if any.
    pub session: Option<String>,
    pub session_name: Option<String>,
    /// kimo itself.
    pub kimo: bool,
}

fn output(program: &str, args: &[&str]) -> String {
    run_for(Command::new(program).args(args), Duration::from_secs(5))
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn listeners() -> Vec<Listener> {
    if cfg!(target_os = "macos") {
        // lsof names your own processes; netstat adds the ports of the system and others.
        let mut all = parse_lsof(&output("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-Fpcn"]));
        all.extend(parse_netstat(&output("netstat", &["-an", "-p", "tcp"])));
        merge(all)
    } else {
        parse_ss(&output("ss", &["-Hltnp"]))
    }
}

pub fn list(sessions: &[SessionInfo]) -> Vec<Port> {
    let listeners = listeners();
    let roots: Vec<i32> = sessions.iter().filter_map(|s| s.pid).collect();
    let table = stats::read_table();
    let me = std::process::id() as i32;
    let mut ports: Vec<Port> = listeners
        .into_iter()
        .map(|l| {
            let session = l
                .pid
                .and_then(|pid| table.root_among(pid, &roots))
                .and_then(|root| sessions.iter().find(|s| s.pid == Some(root)));
            Port {
                kimo: l.pid == Some(me),
                session: session.map(|s| s.spec.id.clone()),
                session_name: session.map(|s| s.spec.name.clone()),
                listener: l,
            }
        })
        .collect();
    ports.sort_by_key(|p| (p.session.is_none(), p.listener.port));
    ports
}
