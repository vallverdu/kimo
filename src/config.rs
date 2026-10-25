//! Settings: defaults in code, overridable by flags or environment variables (see .env.example).

use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "kimo",
    version,
    about = "Mobile-first agent development environment"
)]
pub struct Config {
    /// Where to listen: localhost, lan, tailscale, zerotier or an IP address.
    #[arg(long, env = "KIMO_BIND", default_value = "localhost")]
    pub bind: String,

    #[arg(long, env = "KIMO_PORT", default_value_t = 7777)]
    pub port: u16,

    /// Access token. Required when not on localhost; generated and printed when missing.
    #[arg(long, env = "KIMO_TOKEN", hide_env_values = true)]
    pub token: Option<String>,

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

impl Config {
    pub fn data_dir(&self) -> PathBuf {
        self.data_dir
            .clone()
            .unwrap_or_else(|| user_home().join(".kimo"))
    }

    pub fn shell() -> String {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}
