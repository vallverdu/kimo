//! Hardware models: what a kind of machine is for and how its power is handled, so adding a
//! machine to the fleet fills in its registry entry (labels, role, power, resources, idle
//! watts) and the map draws it. The model is guessed from what the network says (name, OS)
//! and then from what its kimo reports (`/api/node`); you can always pick another.

use super::registry::{Device, PowerPolicy, Resources, Sleep, Wake};
use serde::Serialize;

/// What a machine does in the fleet (also one of its labels, so jobs can ask for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Role {
    pub id: &'static str,
    pub name: &'static str,
    pub what: &'static str,
}

pub const ROLES: &[Role] = &[
    Role {
        id: "control-plane",
        name: "Control plane",
        what: "Runs the coordinator; no jobs",
    },
    Role {
        id: "agent-host",
        name: "Agent host",
        what: "Always on: planning, review and analysis agents (they think in the cloud)",
    },
    Role {
        id: "gpu-worker",
        name: "GPU worker",
        what: "Training, fine-tuning, local inference; woken for work, off when idle",
    },
    Role {
        id: "build-worker",
        name: "Build worker",
        what: "CPU builds and tests, small GPU jobs",
    },
    Role {
        id: "edge-target",
        name: "Edge target",
        what: "Benchmarks and deployment tests on the device itself",
    },
    Role {
        id: "apple-builder",
        name: "Apple builder",
        what: "Xcode builds, signing, simulators; used when awake",
    },
    Role {
        id: "spare",
        name: "Spare",
        what: "Kept in the fleet, given no work",
    },
];

/// A kind of machine with its defaults.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Model {
    pub id: &'static str,
    pub name: &'static str,
    /// The map's drawing for it.
    pub kind: &'static str,
    pub role: &'static str,
    pub labels: &'static [&'static str],
    pub wake: Wake,
    pub sleep: Sleep,
    pub gpu: u32,
    pub cpu_slots: u32,
    /// Idle watts (an estimate: measure yours with a metering plug).
    pub cost: f64,
    /// What to set up on the machine itself (BIOS, power) for this to work.
    pub notes: &'static str,
}

const NUC_BIOS: &str = "BIOS: Wake-on-LAN from S4/S5 = Power On; Wake on USB from S5 off; After power failure = Stay off. Wake-on-LAN works over the cable.";

pub const MODELS: &[Model] = &[
    Model {
        id: "nuc-11-extreme",
        name: "Intel NUC 11 Extreme (NUC11BT) with a GPU",
        kind: "nucExtreme",
        role: "gpu-worker",
        labels: &["x86_64", "linux"],
        wake: Wake::Wol,
        sleep: Sleep::Shutdown,
        gpu: 1,
        cpu_slots: 4,
        cost: 40.0,
        notes: NUC_BIOS,
    },
    Model {
        id: "nuc-11-enthusiast",
        name: "Intel NUC 11 Enthusiast (NUC11PH, RTX 2060)",
        kind: "nucEnthusiast",
        role: "build-worker",
        labels: &["x86_64", "linux"],
        wake: Wake::Wol,
        sleep: Sleep::Shutdown,
        gpu: 1,
        cpu_slots: 4,
        cost: 18.0,
        notes: NUC_BIOS,
    },
    Model {
        id: "jetson-orin-nano",
        name: "NVIDIA Jetson Orin Nano",
        kind: "jetson",
        role: "edge-target",
        labels: &["arm64", "linux"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 1,
        cost: 5.0,
        notes: "No Wake-on-LAN: keep it on (a 7 W nvpmodel mode), or on a smart plug.",
    },
    Model {
        id: "rpi5-hailo",
        name: "Raspberry Pi 5 with AI HAT+ (Hailo)",
        kind: "rpi5hat",
        role: "agent-host",
        labels: &["arm64", "linux"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 2,
        cost: 3.0,
        notes: "Always on (about 3 W idle). Its Hailo is for vision, not LLMs.",
    },
    Model {
        id: "rpi5",
        name: "Raspberry Pi 5",
        kind: "rpi5",
        role: "agent-host",
        labels: &["arm64", "linux"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 2,
        cost: 3.0,
        notes: "Always on (about 3 W idle).",
    },
    Model {
        id: "rpi4",
        name: "Raspberry Pi 4",
        kind: "rpi4",
        role: "control-plane",
        labels: &["arm64", "linux"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 0,
        cost: 3.0,
        notes: "Always on; boot it from a USB SSD. As the control plane it takes no jobs.",
    },
    Model {
        id: "macbook-air",
        name: "MacBook Air (Apple silicon)",
        kind: "macbookAir",
        role: "apple-builder",
        labels: &["macos", "arm64"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 2,
        cost: 3.0,
        notes:
            "Never woken over the network: used when it is open and plugged in. Jobs for it wait.",
    },
    Model {
        id: "mac-mini",
        name: "Mac mini (Apple silicon)",
        kind: "macMini",
        role: "apple-builder",
        labels: &["macos", "arm64"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 4,
        cost: 4.0,
        notes: "Always on; run kimo inside your logged-in session so Xcode can sign.",
    },
    Model {
        id: "linux-gpu-pc",
        name: "Linux PC with an NVIDIA GPU",
        kind: "nucExtreme",
        role: "gpu-worker",
        labels: &["x86_64", "linux"],
        wake: Wake::Wol,
        sleep: Sleep::Shutdown,
        gpu: 1,
        cpu_slots: 4,
        cost: 50.0,
        notes: "Enable Wake-on-LAN in its BIOS and on its wired card (ethtool -s <card> wol g).",
    },
    Model {
        id: "linux-pc",
        name: "Linux PC or server",
        kind: "nucEnthusiast",
        role: "build-worker",
        labels: &["linux"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 2,
        cost: 20.0,
        notes: "Left on. Set power: { wake: wol, sleep: shutdown } once Wake-on-LAN works on it.",
    },
    Model {
        id: "mac",
        name: "Other Mac",
        kind: "macbookAir",
        role: "apple-builder",
        labels: &["macos"],
        wake: Wake::Never,
        sleep: Sleep::Never,
        gpu: 0,
        cpu_slots: 2,
        cost: 5.0,
        notes: "Used when awake.",
    },
];

pub fn model(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id == id)
}

pub fn role(id: &str) -> Option<&'static Role> {
    ROLES.iter().find(|r| r.id == id)
}

/// What is known about a machine before choosing its model.
#[derive(Debug, Clone, Default)]
pub struct Clues<'a> {
    /// Its name on the network (`nuc-extreme`, `jordis-macbook-air`).
    pub name: &'a str,
    /// The network's word for its OS (`linux`, `macOS`), or kimo's (`linux`, `macos`).
    pub os: &'a str,
    /// Its kimo's facts (`/api/node`), once it runs.
    pub facts: Option<&'a serde_json::Value>,
}

/// The most likely model: from its kimo's facts when there are some, else from its name and OS.
pub fn guess(c: &Clues) -> &'static Model {
    let f = c.facts.cloned().unwrap_or_default();
    let product = f["model"].as_str().unwrap_or_default().to_ascii_lowercase();
    let name = c.name.to_ascii_lowercase();
    let text = format!("{product} {name}");
    let os = c.os.to_ascii_lowercase();
    let has = |s: &str| text.contains(s);
    let pick = |id: &str| model(id).expect("known model");
    if f["jetson_l4t"].is_string() || has("jetson") || has("orin") {
        return pick("jetson-orin-nano");
    }
    if has("raspberry pi 5") || has("rpi5") || has("pi5") {
        return if f["hailo"].is_string() || has("hailo") || has("hat") {
            pick("rpi5-hailo")
        } else {
            pick("rpi5")
        };
    }
    if has("raspberry pi 4") || has("rpi4") || has("pi4") {
        return pick("rpi4");
    }
    if has("nuc11bt") || has("extreme") || has("beast") {
        return pick("nuc-11-extreme");
    }
    if has("nuc11ph") || has("enthusiast") || has("phantom") {
        return pick("nuc-11-enthusiast");
    }
    if os.contains("mac") {
        return if has("mini") {
            pick("mac-mini")
        } else if has("macbook") || has("air") {
            pick("macbook-air")
        } else {
            pick("mac")
        };
    }
    let gpus = f["gpus"].as_array().is_some_and(|g| !g.is_empty());
    if gpus || has("gpu") {
        return pick("linux-gpu-pc");
    }
    pick("linux-pc")
}

/// A machine's registry entry from its model, its address and what its kimo reports.
/// `wol`: its wired card's MAC and the broadcast address to wake it through (when it shares a
/// network with the coordinator); without one, a model that would be woken is left on.
pub fn entry(
    m: &Model,
    role_id: &str,
    kimo: &str,
    facts: Option<&serde_json::Value>,
    wol: Option<(String, String)>,
) -> Device {
    let mut labels: Vec<String> = m.labels.iter().map(|l| l.to_string()).collect();
    labels.push(role_id.to_string());
    let vram_gb = facts.and_then(|f| {
        f["gpus"]
            .as_array()?
            .iter()
            .filter_map(|g| g["memory_total_mb"].as_f64())
            .reduce(f64::max)
            .map(|mb| (mb / 1024.0).round())
    });
    let memory_gb = facts
        .and_then(|f| f["mem_total_bytes"].as_f64())
        .map(|b| (b / 1e9).round());
    let gpus_seen = facts
        .and_then(|f| f["gpus"].as_array().map(|g| g.len() as u32))
        .unwrap_or(m.gpu);
    let (wake, sleep, mac, broadcast) = match (m.wake, wol) {
        (Wake::Wol, Some((mac, broadcast))) => (Wake::Wol, m.sleep, Some(mac), Some(broadcast)),
        // No card the coordinator can reach: never switched off, since it could not be woken.
        (Wake::Wol, None) => (Wake::Never, Sleep::Never, None, None),
        (w, _) => (w, m.sleep, None, None),
    };
    Device {
        kimo: kimo.to_string(),
        mac,
        broadcast,
        power: PowerPolicy {
            wake,
            sleep,
            ..PowerPolicy::default()
        },
        labels,
        resources: Resources {
            gpu: if m.gpu > 0 { gpus_seen.max(1) } else { 0 },
            vram_gb,
            cpu_slots: if role_id == "control-plane" || role_id == "spare" {
                0
            } else {
                m.cpu_slots
            },
            memory_gb,
        },
        cost: m.cost,
        model: Some(m.id.to_string()),
        role: Some(role_id.to_string()),
    }
}

/// The card to wake a machine through: a wired, non-VPN card on one of the coordinator's own
/// networks (`(ip, prefix)` of the coordinator's cards) → its MAC and that network's broadcast.
pub fn wol_card(
    facts: &serde_json::Value,
    mine: &[(std::net::Ipv4Addr, u8)],
) -> Option<(String, String)> {
    let same_net = |ip: std::net::Ipv4Addr| {
        mine.iter().any(|(m, p)| {
            let mask = if *p == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from((*p).min(32)))
            };
            u32::from(*m) & mask == u32::from(ip) & mask
        })
    };
    let nics = facts["nics"].as_array()?;
    let ok = |n: &&serde_json::Value| {
        n["vpn"] != true
            && n["ipv4"]
                .as_str()
                .and_then(|i| i.parse().ok())
                .is_some_and(same_net)
    };
    // A cable first; Wi-Fi only if nothing else (some boards wake over it).
    let card = nics
        .iter()
        .filter(ok)
        .find(|n| n["wired"] == true)
        .or_else(|| nics.iter().find(ok))?;
    Some((
        card["mac"].as_str()?.to_string(),
        card["broadcast"].as_str()?.to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_model_and_role_is_known() {
        for m in MODELS {
            assert!(role(m.role).is_some(), "{}", m.id);
            assert!(crate::core::registry::valid_id(m.id));
        }
        let ids: std::collections::BTreeSet<_> = MODELS.iter().map(|m| m.id).collect();
        assert_eq!(ids.len(), MODELS.len());
    }

    #[test]
    fn guesses_from_names_then_facts() {
        let g = |name, os| {
            guess(&Clues {
                name,
                os,
                facts: None,
            })
            .id
        };
        assert_eq!(g("nuc-extreme", "linux"), "nuc-11-extreme");
        assert_eq!(g("jetson", "linux"), "jetson-orin-nano");
        assert_eq!(g("pi5", "linux"), "rpi5");
        assert_eq!(g("jordis-macbook-air", "macOS"), "macbook-air");
        assert_eq!(g("studio-mini", "macOS"), "mac-mini");
        assert_eq!(g("box", "linux"), "linux-pc");
        let f = |facts: serde_json::Value| {
            guess(&Clues {
                name: "ubuntu",
                os: "linux",
                facts: Some(&facts),
            })
            .id
        };
        assert_eq!(
            f(json!({ "model": "Raspberry Pi 5 Model B Rev 1.0", "hailo": "HAILO10H" })),
            "rpi5-hailo"
        );
        assert_eq!(
            f(json!({ "model": "Raspberry Pi 4 Model B Rev 1.5" })),
            "rpi4"
        );
        assert_eq!(
            f(json!({ "model": "NVIDIA Jetson Orin Nano Developer Kit", "jetson_l4t": "36.4.3" })),
            "jetson-orin-nano"
        );
        assert_eq!(f(json!({ "model": "NUC11BTMi9" })), "nuc-11-extreme");
        assert_eq!(f(json!({ "model": "NUC11PHi7" })), "nuc-11-enthusiast");
        assert_eq!(
            f(json!({ "model": "B550", "gpus": [{ "name": "RTX 4090" }] })),
            "linux-gpu-pc"
        );
    }

    #[test]
    fn entries_from_the_model_and_the_facts() {
        let facts = json!({
            "mem_total_bytes": 32e9,
            "gpus": [{ "name": "RTX 3060", "memory_total_mb": 12288 }],
            "nics": [
                { "name": "wlo1", "mac": "aa:aa:aa:aa:aa:aa", "ipv4": "192.168.1.30", "prefix": 24, "broadcast": "192.168.1.255", "wired": false, "vpn": false },
                { "name": "enp3s0", "mac": "1c:69:7a:aa:bb:cc", "ipv4": "192.168.1.21", "prefix": 24, "broadcast": "192.168.1.255", "wired": true, "vpn": false },
                { "name": "ztabc", "mac": "bb:bb:bb:bb:bb:bb", "ipv4": "10.147.17.21", "prefix": 24, "broadcast": "10.147.17.255", "wired": false, "vpn": true }
            ]
        });
        let pi_lan = [("192.168.1.2".parse().unwrap(), 24)];
        let wol = wol_card(&facts, &pi_lan);
        assert_eq!(
            wol,
            Some(("1c:69:7a:aa:bb:cc".into(), "192.168.1.255".into()))
        );
        let m = model("nuc-11-extreme").unwrap();
        let d = entry(
            m,
            "gpu-worker",
            "http://10.147.17.21:7777",
            Some(&facts),
            wol,
        );
        assert_eq!(d.power.wake, Wake::Wol);
        assert_eq!(d.power.sleep, Sleep::Shutdown);
        assert_eq!(d.mac.as_deref(), Some("1c:69:7a:aa:bb:cc"));
        assert_eq!(d.resources.vram_gb, Some(12.0));
        assert_eq!(d.resources.memory_gb, Some(32.0));
        assert_eq!(d.resources.gpu, 1);
        assert_eq!(d.labels, ["x86_64", "linux", "gpu-worker"]);
        assert_eq!(d.model.as_deref(), Some("nuc-11-extreme"));
        // Not on the coordinator's network: it cannot be woken, so it is never switched off.
        let elsewhere = [("10.0.0.2".parse().unwrap(), 24)];
        let d = entry(
            m,
            "gpu-worker",
            "http://x:7777",
            Some(&facts),
            wol_card(&facts, &elsewhere),
        );
        assert_eq!((d.power.wake, d.power.sleep), (Wake::Never, Sleep::Never));
        // A control plane takes no jobs.
        let pi = entry(
            model("rpi4").unwrap(),
            "control-plane",
            "http://p:7777",
            None,
            None,
        );
        assert_eq!(pi.resources.cpu_slots, 0);
    }
}
