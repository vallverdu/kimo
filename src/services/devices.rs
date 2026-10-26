//! Pairing phones and other devices: a one-time code (shown as a QR) becomes a device key.

use crate::core::access::{normalize_code, pairing_code};
use crate::core::models::Device;
use crate::services::sessions::{slug, Manager};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

pub const PAIRING_TTL: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub paired_at: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pairing {
    pub code: String,
    pub expires_in: u64,
}

fn random_hex() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Wrong codes tried since the last good one: past this many, every open code is dropped.
static WRONG: AtomicU32 = AtomicU32::new(0);
const MAX_WRONG: u32 = 20;

impl Manager {
    /// A one-time code valid for ten minutes, short enough to type (`K7M2-9QXP`).
    pub fn start_pairing(&self) -> Pairing {
        let code = pairing_code(uuid::Uuid::new_v4().as_u128());
        let mut codes = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        codes.retain(|_, expires| *expires > now);
        codes.insert(normalize_code(&code), now + PAIRING_TTL);
        Pairing {
            code,
            expires_in: PAIRING_TTL.as_secs(),
        }
    }

    /// Trade a pairing code (once) for a device key.
    pub fn pair(&self, code: &str, name: &str) -> Result<Device> {
        let valid = {
            let mut codes = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
            let valid = codes
                .remove(&normalize_code(code))
                .is_some_and(|expires| expires > Instant::now());
            // Guessing codes: after too many wrong ones, none of the open codes works.
            if valid {
                WRONG.store(0, Ordering::Relaxed);
            } else if WRONG.fetch_add(1, Ordering::Relaxed) + 1 >= MAX_WRONG {
                codes.clear();
                WRONG.store(0, Ordering::Relaxed);
            }
            valid
        };
        if !valid {
            bail!("this pairing code is wrong, used or expired: show a new one on kimo");
        }
        let name = match name.trim() {
            "" => "device",
            n => n,
        };
        let device = Device {
            id: slug(name),
            name: name.chars().take(60).collect(),
            token: format!("kimo_dev_{}{}", random_hex(), random_hex()),
            paired_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
            extra: Default::default(),
        };
        let mut inner = self.lock();
        inner.state.devices.push(device.clone());
        self.save(&inner)?;
        Ok(device)
    }

    /// A device key without a pairing code (for a peer kimo during mutual pairing).
    pub fn create_device(&self, name: &str) -> Result<Device> {
        let code = self.start_pairing().code;
        self.pair(&code, name)
    }

    pub fn devices(&self) -> Vec<DeviceInfo> {
        self.lock()
            .state
            .devices
            .iter()
            .map(|d| DeviceInfo {
                id: d.id.clone(),
                name: d.name.clone(),
                paired_at: d.paired_at,
            })
            .collect()
    }

    pub fn device_by_token(&self, token: &str) -> Option<Device> {
        self.lock()
            .state
            .devices
            .iter()
            .find(|d| d.token == token)
            .cloned()
    }

    pub fn delete_device(&self, id: &str) -> Result<()> {
        let mut inner = self.lock();
        let before = inner.state.devices.len();
        inner.state.devices.retain(|d| d.id != id);
        if inner.state.devices.len() == before {
            return Err(anyhow!("unknown device: {id}"));
        }
        self.save(&inner)?;
        drop(inner);
        self.revoked();
        Ok(())
    }
}
