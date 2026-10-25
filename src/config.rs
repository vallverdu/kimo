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

    #[arg(long, env = "KIMO_PORT", default_value_t = 7777)]
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
    /// Run kimo (the default).
    Serve,
    /// Show a QR code in this terminal to pair a phone with the kimo running on this machine.
    Pair,
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
