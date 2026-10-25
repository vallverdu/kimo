//! kimo: terminals, accounts and resources for coding agents, driven from a phone.

pub mod api;
pub mod cli;
pub mod client;
pub mod config;
pub mod core;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod services;
