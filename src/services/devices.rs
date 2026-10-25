//! Pairing phones and other devices: a one-time code (shown as a QR) becomes a device key.

use crate::core::models::Device;
use crate::services::sessions::{slug, Manager};
use anyhow::{anyhow, bail, Result};
use serde::Serialize;
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

impl Manager {
    /// A one-time code valid for ten minutes.
    pub fn start_pairing(&self) -> Pairing {
        let code = random_hex();
        let mut codes = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        codes.retain(|_, expires| *expires > now);
        codes.insert(code.clone(), now + PAIRING_TTL);
        Pairing {
            code,
            expires_in: PAIRING_TTL.as_secs(),
        }
    }

    /// Trade a pairing code (once) for a device key.
    pub fn pair(&self, code: &str, name: &str) -> Result<Device> {
        let valid = {
            let mut codes = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
            codes
                .remove(code)
                .is_some_and(|expires| expires > Instant::now())
        };
        if !valid {
            bail!("this pairing code is used or expired: show a new QR on kimo");
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
        self.save(&inner)
    }
}
