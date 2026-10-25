//! Settings: defaults in code, overridable by flags or environment variables (see .env.example).

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kimo",
    version,
    about = "Mobile-first agent development environment"
)]
pub struct Config {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Where to listen: all (every network), localhost, tailscale, zerotier or an IP address.
    /// Only this machine gets in without pairing; other devices pair with a QR code.
    #[arg(long, env = "KIMO_BIND", default_value = "all")]
    pub bind: String,

    #[arg(long, global = true, env = "KIMO_PORT", default_value_t = 7777)]
    pub port: u16,

    /// Optional owner token for scripts. Devices normally pair with a QR code instead.
    #[arg(long, env = "KIMO_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

    /// Treat requests from this machine itself as the owner (no pairing needed on the server).
    /// Turn off on machines shared with other users.
    #[arg(long, env = "KIMO_LOCAL_OWNER", default_value_t = true, action = clap::ArgAction::Set)]
    pub local_owner: bool,

    /// Folder where new projects are created. Default: ~/dev
    #[arg(long, env = "KIMO_PROJECTS_DIR")]
    pub projects_dir: Option<PathBuf>,

    /// Extra origins (comma separated) where the admin token may be used from another page.
    /// Apps normally get their own key instead (Apps tab or POST /api/clients).
    #[arg(long, env = "KIMO_ORIGINS", value_delimiter = ',')]
    pub origins: Vec<String>,

    /// Data folder (state.json and account homes). Default: ~/.kimo
    #[arg(long, env = "KIMO_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Bytes of output kept per terminal for reconnecting clients.
    #[arg(long, env = "KIMO_SCROLLBACK", default_value_t = 256 * 1024)]
    pub scrollback_bytes: usize,
}

pub fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Open the kimo window (the default when a display is available).
    App,
    /// Run only the server, without a window (servers, systemd).
    Serve,
    /// Show a QR code in this terminal to pair a phone with the kimo running on this machine.
    Pair,
    /// List the agents (id, name, state, resume id, folder).
    Agents {
        #[arg(long)]
        json: bool,
    },
    /// Send a message to an agent (signed with your session when run inside kimo).
    Send {
        /// Agent id or name (`kimo agents`).
        to: String,
        /// The message.
        #[arg(required = true, num_args = 1..)]
        message: Vec<String>,
    },
    /// Print an agent's last reply (or its recent screen with --screen).
    Read {
        agent: String,
        #[arg(long)]
        screen: bool,
        #[arg(long, default_value_t = 40)]
        lines: usize,
    },
    /// Wait until an agent is not working any more.
    Wait {
        agent: String,
        /// Give up after this many seconds.
        #[arg(long, default_value_t = 900)]
        timeout: u64,
    },
    /// Start a new agent (claude, codex or shell), optionally with a first message.
    New {
        kind: String,
        #[arg(long)]
        name: Option<String>,
        /// Folder (default: the current one).
        #[arg(long)]
        cwd: Option<String>,
        #[arg(long)]
        account: Option<String>,
        /// First message, sent once the agent is ready.
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Which kimo session am I?
    Whoami,
}

impl Config {
    pub fn projects_dir(&self) -> PathBuf {
        self.projects_dir
            .clone()
            .unwrap_or_else(|| user_home().join("dev"))
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| user_home().join(".kimo"))
    }

    pub fn shell() -> String {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}
