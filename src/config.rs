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

    /// This kimo's name for the other kimos it is paired with (prefix of its agents' ids there).
    /// Default: the machine's hostname.
    #[arg(long, env = "KIMO_NAME")]
    pub name: Option<String>,

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

    /// The fleet coordinator on this machine (`kimo-fleet`), e.g. http://127.0.0.1:7780: the
    /// owner's apps reach it at /api/fleet (machines' power, jobs, workflows).
    #[arg(long, env = "KIMO_FLEET")]
    pub fleet: Option<String>,

    /// Key for the fleet coordinator, when it asks for one.
    #[arg(long, env = "KIMO_FLEET_TOKEN", hide_env_values = true)]
    pub fleet_token: Option<String>,

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
    /// macOS: put kimo.app in Applications, to open kimo from the Dock, Launchpad or Spotlight.
    InstallApp,
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
    /// Earlier Claude and Codex conversations in a folder (default: the current one).
    History { folder: Option<String> },
    /// Continue one of those conversations as a kimo session.
    Continue {
        /// Conversation id (or its beginning) from `kimo history`.
        id: String,
        #[arg(long)]
        folder: Option<String>,
    },
    /// Daily usage of Claude and Codex on this machine, by project and account.
    Usage {
        /// How many days, today included.
        #[arg(long, default_value_t = 14)]
        days: u32,
        /// The whole report as JSON.
        #[arg(long)]
        json: bool,
    },
    /// List the other kimos paired with this one.
    Peers,
    /// Run a job's folder to its end (kimo starts this itself for `POST /api/jobs`).
    #[command(hide = true)]
    JobRun { dir: PathBuf },
    /// Pair with or forget another kimo (lab box, Raspberry Pi, Mac mini…).
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum PeerAction {
    /// Pair with the kimo at URL using a pairing code shown there (`kimo pair` or Devices).
    Add {
        url: String,
        code: String,
        /// Where that kimo reaches this one (default: picked from this kimo's addresses).
        #[arg(long)]
        my_url: Option<String>,
    },
    /// Forget a paired kimo.
    Remove { id: String },
}

impl Config {
    pub fn host_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(hostname)
    }

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

/// The machine's hostname.
pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `buf.len()` bytes into the buffer.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if ok && end > 0 {
        String::from_utf8_lossy(&buf[..end]).into_owned()
    } else {
        "kimo".into()
    }
}
