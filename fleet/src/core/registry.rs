//! The device registry: `devices.yaml`, written by hand. It says what each machine is for
//! and how its power is handled; the facts a machine reports say whether it can do a job now.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    /// ntfy topic URL for notifications (`https://ntfy.sh/my-fleet`), optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ntfy: Option<String>,
    #[serde(default)]
    pub devices: BTreeMap<String, Device>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    /// Its kimo: `http://10.147.17.21:7777`.
    pub kimo: String,
    /// Network card address for Wake-on-LAN (`1c:69:7a:aa:bb:cc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    /// Where the magic packet goes: the LAN's broadcast address (default 255.255.255.255).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broadcast: Option<String>,
    #[serde(default)]
    pub power: PowerPolicy,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub resources: Resources,
    /// Watts when idle: cheaper machines are chosen first.
    #[serde(default)]
    pub cost: f64,
    /// Its hardware model (`nuc-11-extreme`, `rpi5`… see `kimo-fleet models`): the map draws it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// What it does in the fleet (`gpu-worker`, `agent-host`…); also one of its labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
}

impl Device {
    /// Its labels, with its role.
    pub fn all_labels(&self) -> Vec<String> {
        let mut l = self.labels.clone();
        if let Some(r) = &self.role {
            if !l.contains(r) {
                l.push(r.clone());
            }
        }
        l
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Wake {
    /// Wake-on-LAN from the coordinator's network.
    Wol,
    /// Never woken: used when it is on (a laptop, a board left on).
    #[default]
    Never,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sleep {
    /// Switched off (S5) after being idle.
    Shutdown,
    /// Left as it is.
    #[default]
    Never,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerPolicy {
    #[serde(default)]
    pub wake: Wake,
    #[serde(default)]
    pub sleep: Sleep,
    /// Idle this long (no jobs, no agent working) before it is switched off.
    #[serde(default = "idle_minutes")]
    pub idle_minutes: f64,
    /// Given this long to boot and answer after a wake.
    #[serde(default = "boot_deadline_s")]
    pub boot_deadline_s: u64,
    /// Automatic cold boots per day, at most.
    #[serde(default = "boots_per_day")]
    pub boots_per_day: u32,
}

fn idle_minutes() -> f64 {
    20.0
}
fn boot_deadline_s() -> u64 {
    180
}
fn boots_per_day() -> u32 {
    10
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self {
            wake: Wake::Never,
            sleep: Sleep::Never,
            idle_minutes: idle_minutes(),
            boot_deadline_s: boot_deadline_s(),
            boots_per_day: boots_per_day(),
        }
    }
}

/// What a machine offers to jobs; a job reserves part of it while it runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    #[serde(default)]
    pub gpu: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_gb: Option<f64>,
    /// Jobs at once (default 2).
    #[serde(default = "cpu_slots")]
    pub cpu_slots: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<f64>,
}

fn cpu_slots() -> u32 {
    2
}

impl Default for Resources {
    fn default() -> Self {
        Self {
            gpu: 0,
            vram_gb: None,
            cpu_slots: cpu_slots(),
            memory_gb: None,
        }
    }
}

/// `1c:69:7a:aa:bb:cc` or `1C-69-7A-AA-BB-CC` → its six bytes.
pub fn parse_mac(mac: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = mac.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut out = [0u8; 6];
    for (o, p) in out.iter_mut().zip(parts) {
        if p.len() != 2 {
            return None;
        }
        *o = u8::from_str_radix(p, 16).ok()?;
    }
    Some(out)
}

/// Device ids name things in URLs and job records: letters, digits, `-` and `_`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The host part of a kimo URL (what is pinged).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1)?;
    let authority = rest.split('/').next()?;
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_string())
}

/// Read `devices.yaml`, with the problems a person can fix in plain words.
pub fn parse(text: &str) -> Result<Registry, String> {
    let r: Registry = if text.trim().is_empty() {
        Registry::default()
    } else {
        serde_yaml_ng::from_str(text).map_err(|e| format!("devices.yaml: {e}"))?
    };
    for (id, d) in &r.devices {
        if !valid_id(id) {
            return Err(format!(
                "devices.yaml: '{id}' is not a device id (letters, digits, - and _)"
            ));
        }
        if !(d.kimo.starts_with("http://") || d.kimo.starts_with("https://"))
            || host_of(&d.kimo).is_none()
        {
            return Err(format!(
                "devices.yaml: {id}: kimo must be its address, like http://10.147.17.21:7777"
            ));
        }
        if d.power.wake == Wake::Wol {
            match &d.mac {
                Some(m) if parse_mac(m).is_some() => {}
                Some(m) => return Err(format!("devices.yaml: {id}: '{m}' is not a MAC address")),
                None => {
                    return Err(format!(
                        "devices.yaml: {id}: wake: wol needs the mac of its network card"
                    ))
                }
            }
        }
        if let Some(b) = &d.broadcast {
            if b.parse::<std::net::IpAddr>().is_err() && b.parse::<std::net::SocketAddr>().is_err()
            {
                return Err(format!(
                    "devices.yaml: {id}: broadcast must be an address like 192.168.1.255"
                ));
            }
        }
    }
    Ok(r)
}

/// `devices.yaml` with the machines added from the Fleet tab (`devices.d/<id>.yaml`, one
/// each, so the hand-written file is never rewritten). A machine in both is a mistake.
pub fn merge(main: &str, added: &[(String, String)]) -> Result<Registry, String> {
    let mut r = parse(main)?;
    for (file, text) in added {
        let extra = parse(text).map_err(|e| e.replace("devices.yaml", file))?;
        for (id, d) in extra.devices {
            if r.devices.contains_key(&id) {
                return Err(format!("{file}: {id} is already in devices.yaml"));
            }
            r.devices.insert(id, d);
        }
    }
    Ok(r)
}

/// The file for a machine added from the Fleet tab.
pub fn render_added(id: &str, d: &Device, note: &str) -> String {
    let mut one = Registry::default();
    one.devices.insert(id.to_string(), d.clone());
    let body = serde_yaml_ng::to_string(&one).unwrap_or_default();
    format!("# {note}\n# Edit freely; `kimo-fleet reload` reads it again. Removing the machine deletes this file.\n{body}")
}

/// The registry new installs start from (commented, with the doc's example).
pub const EXAMPLE: &str = r#"# Machines this coordinator manages. Edit by hand, then `kimo-fleet reload` (or restart).
# Pair each one after adding it: on the machine, `kimo pair` shows a code; here,
# `kimo-fleet pair <id> <code>`.
#
# ntfy: https://ntfy.sh/my-fleet-topic     # notifications (optional)
devices: {}
#  nuc-extreme:
#    kimo: http://10.147.17.21:7777          # its kimo (ZeroTier or LAN address)
#    mac: "1c:69:7a:aa:bb:cc"                # for Wake-on-LAN
#    broadcast: 192.168.1.255                # the LAN's broadcast address
#    power: { wake: wol, sleep: shutdown, idle_minutes: 20, boot_deadline_s: 180 }
#    labels: [x86_64, linux, cuda]
#    resources: { gpu: 1, vram_gb: 12, cpu_slots: 4, memory_gb: 32 }
#    cost: 40                                # watts when idle; cheaper machines first
#  macbook-air:
#    kimo: http://10.147.17.30:7777
#    power: { wake: never, sleep: never }
#    labels: [macos, arm64, xcode]
#    resources: { cpu_slots: 2 }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn added_machines_merge_with_the_hand_written_file() {
        let main = "devices:\n  nuc:\n    kimo: http://10.0.0.2:7777\n";
        let d = Device {
            kimo: "http://10.0.0.3:7777".into(),
            mac: None,
            broadcast: None,
            power: PowerPolicy::default(),
            labels: vec!["arm64".into()],
            resources: Resources::default(),
            cost: 3.0,
            model: Some("rpi5".into()),
            role: Some("agent-host".into()),
        };
        let text = render_added("pi5", &d, "Added from the Fleet tab");
        assert!(text.starts_with("# Added from the Fleet tab"));
        let r = merge(main, &[("devices.d/pi5.yaml".into(), text.clone())]).unwrap();
        assert_eq!(r.devices.len(), 2);
        assert_eq!(r.devices["pi5"], d);
        assert_eq!(r.devices["pi5"].all_labels(), ["arm64", "agent-host"]);
        let twice = merge(main, &[("a".into(), text.clone()), ("b".into(), text)]);
        assert!(twice.unwrap_err().contains("already"));
    }

    #[test]
    fn the_example_from_the_design() {
        let text = r#"
devices:
  nuc-extreme:
    kimo: http://10.147.17.21:7777     # ZeroTier address
    mac: "1c:69:7a:aa:bb:cc"
    power: { wake: wol, sleep: shutdown, idle_minutes: 20, boot_deadline_s: 180 }
    labels: [x86_64, linux, cuda]
    resources: { gpu: 1, vram_gb: 12, cpu_slots: 4, memory_gb: 32 }
    cost: 40
  macbook-air:
    kimo: http://10.147.17.30:7777
    power: { wake: never, sleep: never }
    labels: [macos, arm64, xcode]
    resources: { cpu_slots: 2 }
"#;
        let r = parse(text).unwrap();
        let nuc = &r.devices["nuc-extreme"];
        assert_eq!(nuc.power.wake, Wake::Wol);
        assert_eq!(nuc.power.sleep, Sleep::Shutdown);
        assert_eq!(nuc.power.boots_per_day, 10);
        assert_eq!(nuc.resources.gpu, 1);
        assert_eq!(nuc.resources.vram_gb, Some(12.0));
        assert_eq!(nuc.cost, 40.0);
        let mac = &r.devices["macbook-air"];
        assert_eq!(mac.power.wake, Wake::Never);
        assert_eq!(mac.resources.gpu, 0);
        assert!(parse(EXAMPLE).unwrap().devices.is_empty());
        assert!(parse("").unwrap().devices.is_empty());
    }

    #[test]
    fn mistakes_are_explained() {
        let no_mac = "devices:\n  nuc:\n    kimo: http://10.0.0.2:7777\n    power: { wake: wol }\n";
        assert!(parse(no_mac).unwrap_err().contains("needs the mac"));
        let bad_url = "devices:\n  nuc:\n    kimo: 10.0.0.2\n";
        assert!(parse(bad_url).unwrap_err().contains("its address"));
        let typo = "devices:\n  nuc:\n    kimo: http://10.0.0.2:7777\n    lables: [x]\n";
        assert!(parse(typo).unwrap_err().contains("lables"));
        let bad_id = "devices:\n  \"a b\":\n    kimo: http://10.0.0.2:7777\n";
        assert!(parse(bad_id).unwrap_err().contains("not a device id"));
    }

    #[test]
    fn macs_and_hosts() {
        assert_eq!(
            parse_mac("1C-69-7a-aa-bb-cc"),
            Some([0x1c, 0x69, 0x7a, 0xaa, 0xbb, 0xcc])
        );
        assert_eq!(parse_mac("1c:69:7a:aa:bb"), None);
        assert_eq!(parse_mac("1c:69:7a:aa:bb:zz"), None);
        assert_eq!(
            host_of("http://10.147.17.21:7777").as_deref(),
            Some("10.147.17.21")
        );
        assert_eq!(host_of("https://nuc.local/").as_deref(), Some("nuc.local"));
        assert_eq!(host_of("http://[fd00::1]:7777").as_deref(), Some("fd00::1"));
        assert_eq!(host_of("nonsense"), None);
    }
}
