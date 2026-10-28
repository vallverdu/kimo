//! Flags with environment fallbacks.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kimo-fleet",
    version,
    about = "Coordinator for machines running kimo: their power, jobs and workflows"
)]
pub struct Config {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Data folder: devices.yaml (the registry), keys.json, fleet.db, logs, artifacts.
    /// Default: ~/.kimo-fleet
    #[arg(long, global = true, env = "KIMO_FLEET_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Address to listen on. The kimo on this machine reaches it (`kimo serve --fleet`);
    /// keep it on this machine unless a token is set.
    #[arg(long, env = "KIMO_FLEET_BIND", default_value = "127.0.0.1")]
    pub bind: String,

    #[arg(long, global = true, env = "KIMO_FLEET_PORT", default_value_t = 7780)]
    pub port: u16,

    /// A token for calls from elsewhere (scripts); this machine itself needs none.
    #[arg(long, global = true, env = "KIMO_FLEET_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// This coordinator's name on the machines (default: the hostname).
    #[arg(long, env = "KIMO_FLEET_NAME")]
    pub name: Option<String>,

    /// The SSH program used to set machines up (default: ssh).
    #[arg(long, env = "KIMO_FLEET_SSH", default_value = "ssh")]
    pub ssh: String,

    /// The Tailscale program (default: found on the PATH or in Tailscale.app).
    #[arg(long, env = "KIMO_FLEET_TAILSCALE")]
    pub tailscale: Option<String>,

    /// The port kimo listens on, on the machines (looked for, and used when setting one up).
    #[arg(long, env = "KIMO_FLEET_KIMO_PORT", default_value_t = 7777)]
    pub kimo_port: u16,

    /// Look for machines on ZeroTier too.
    #[arg(long, env = "KIMO_FLEET_ZEROTIER", default_value_t = true, action = clap::ArgAction::Set)]
    pub zerotier: bool,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Run the coordinator (the default).
    Serve,
    /// The machines, their state and what they run.
    Devices,
    /// Machines on Tailscale and ZeroTier that could join (is kimo there? SSH?).
    Discover,
    /// Add a machine without sitting at it: install kimo over SSH (or use --code), pair one
    /// way, and give it a hardware model and a role.
    Add {
        /// Its address on Tailscale or ZeroTier.
        address: String,
        /// Its id in the fleet (default: from its name).
        #[arg(long)]
        id: Option<String>,
        /// Hardware model (`kimo-fleet models`); default: guessed from the machine.
        #[arg(long)]
        model: Option<String>,
        /// Role (default: the model's).
        #[arg(long)]
        role: Option<String>,
        /// SSH user (default: yours).
        #[arg(long)]
        user: Option<String>,
        /// Where its kimo listens: tailscale, zerotier or an address (default: its network).
        #[arg(long)]
        network: Option<String>,
        /// Join with a pairing code its kimo shows instead of SSH.
        #[arg(long)]
        code: Option<String>,
        /// Install kimo again even if it is there.
        #[arg(long)]
        reinstall: bool,
    },
    /// Remove a machine added with `add` (its entry and its key).
    Remove { device: String },
    /// Hardware models and roles.
    Models,
    /// Pair with a machine of the registry using a code shown there (`kimo pair`).
    Pair { device: String, code: String },
    /// Wake a machine (Wake-on-LAN).
    Wake { device: String },
    /// Switch a machine off (refused while work runs there).
    Shutdown { device: String },
    /// Keep a machine on, never switch it off, take control, or give it back.
    Hold {
        device: String,
        /// Keep it on for this many hours (0 ends it).
        #[arg(long)]
        hours: Option<f64>,
        /// Never switch it off for being idle.
        #[arg(long)]
        never: bool,
        /// Take control: nothing automatic.
        #[arg(long)]
        manual: bool,
        /// Automatic again (ends --never and --manual).
        #[arg(long)]
        auto: bool,
    },
    /// Run a job or a workflow from a YAML file.
    Run { file: PathBuf },
    /// The jobs, newest first.
    Jobs,
    /// A job's log.
    Log { job: String },
    /// Cancel a job or a workflow.
    Cancel { id: String },
    /// Approve a step that waits for you (or --reject it).
    Approve {
        job: String,
        #[arg(long)]
        reject: bool,
    },
    /// Recent events (wakes, shutdowns, jobs, approvals).
    Events {
        #[arg(long)]
        device: Option<String>,
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Read devices.yaml again.
    Reload,
    /// Copy the database to a file (backups).
    Backup { to: PathBuf },
}

impl Config {
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| kimo::config::user_home().join(".kimo-fleet"))
    }

    pub fn host_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(kimo::config::hostname)
    }
}
