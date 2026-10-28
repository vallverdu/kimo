//! kimo-fleet: the coordinator for a fleet of machines that each run kimo. It owns the
//! machines' registry and power (Wake-on-LAN, shutdown when idle), and the jobs and
//! workflows placed on them; kimo on each machine runs the work.

pub mod api;
pub mod config;
pub mod core;
pub mod services;
