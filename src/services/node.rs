//! Facts about this machine for a fleet coordinator (`GET /api/node`): hardware, GPUs and
//! accelerators, toolchains, load, disk, who is logged in and what kimo runs. What a machine
//! has rarely changes and is looked up once every ten minutes; load and GPU memory on each call.

use crate::core::node::{self, Found, Gpu, Login, Nic};
use crate::services::network::run_for;
use crate::services::sessions::Manager;
use serde::Serialize;
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What the machine is: changes with a driver or Xcode update, not from one call to the next.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Hardware {
    pub os: String,
    pub arch: String,
    /// `Raspberry Pi 5 Model B Rev 1.0`, `NUC11PHi7`, `Mac16,12`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nvidia_driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cuda: Option<String>,
    /// Jetson L4T release (`36.4.3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jetson_l4t: Option<String>,
    /// Hailo accelerator (`HAILO10H`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hailo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub xcode: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Disk {
    pub path: String,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

/// What kimo runs on the machine.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionCounts {
    pub total: usize,
    pub running: usize,
    pub paused: usize,
    /// Agents working right now (from their output).
    pub working: usize,
    /// Unix time (ms) an agent or a job was last seen working (since kimo started).
    pub last_working_at: Option<u64>,
    /// Jobs running now (`/api/jobs`).
    pub jobs_running: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    pub host: String,
    pub version: &'static str,
    #[serde(flatten)]
    pub hardware: Hardware,
    pub cpus: usize,
    /// 1, 5 and 15 minute load averages.
    pub load: [f64; 3],
    pub uptime_s: Option<u64>,
    pub mem_total_bytes: u64,
    pub mem_available_bytes: u64,
    pub disk: Option<Disk>,
    pub gpus: Vec<Gpu>,
    /// Network cards with an IPv4 address (MAC and broadcast address for Wake-on-LAN).
    pub nics: Vec<Nic>,
    pub sessions: SessionCounts,
    /// People logged in to the machine (a desktop, a console, SSH).
    pub logins: Vec<Login>,
    /// Something keeps it on (terminals, jobs, people logged in when they count): it would
    /// refuse to switch off now (`GET /api/power` says what).
    pub in_use: bool,
    pub awake: crate::services::awake::Awake,
    /// Unix time (ms) of these facts.
    pub at: u64,
}

const HARDWARE_FOR: Duration = Duration::from_secs(600);
const GPUS_FOR: Duration = Duration::from_secs(5);
const PROBE_LIMIT: Duration = Duration::from_secs(5);

static HARDWARE: Mutex<Option<(Instant, Hardware)>> = Mutex::new(None);
static GPUS: Mutex<Option<(Instant, Vec<Gpu>)>> = Mutex::new(None);

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = run_for(Command::new(program).args(args), PROBE_LIMIT).ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Is `program` installed (on the PATH or in the usual system folders)?
fn installed(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/usr/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn read_trimmed(path: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!text.is_empty()).then(|| text.to_string())
}

fn model() -> Option<String> {
    if cfg!(target_os = "macos") {
        return output("sysctl", &["-n", "hw.model"]).map(|m| m.trim().to_string());
    }
    read_trimmed("/proc/device-tree/model")
        .or_else(|| read_trimmed("/sys/class/dmi/id/product_name"))
}

/// Xcode's version, only when a full Xcode is selected: running the `xcodebuild` stub without
/// the developer tools would ask to install them.
fn xcode() -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let dir = output("xcode-select", &["-p"])?;
    let dir = Path::new(dir.trim());
    if !dir.to_string_lossy().contains(".app/") {
        return None;
    }
    let tool = dir.join("usr/bin/xcodebuild");
    node::parse_xcode(&output(tool.to_str()?, &["-version"])?)
}

fn nvidia_gpus() -> Vec<Gpu> {
    if installed("nvidia-smi").is_none() {
        return vec![];
    }
    let query = format!("--query-gpu={}", node::NVIDIA_QUERY);
    output("nvidia-smi", &[&query, "--format=csv,noheader,nounits"])
        .map(|csv| node::parse_nvidia_smi(&csv))
        .unwrap_or_default()
}

fn look_up_hardware() -> Hardware {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        other => other,
    }
    .to_string();
    let arch = std::env::consts::ARCH.to_string();
    let model = model();
    let (nvidia_driver, cuda) = if installed("nvidia-smi").is_some() {
        output("nvidia-smi", &[])
            .map(|t| node::parse_nvidia_versions(&t))
            .unwrap_or_default()
    } else {
        (None, None)
    };
    let jetson_l4t = std::fs::read_to_string("/etc/nv_tegra_release")
        .ok()
        .and_then(|t| node::parse_tegra_release(&t));
    let hailo = (Path::new("/dev/hailo0").exists() && installed("hailortcli").is_some())
        .then(|| output("hailortcli", &["fw-control", "identify"]))
        .flatten()
        .and_then(|t| node::parse_hailo_architecture(&t));
    let xcode = xcode();
    let found = Found {
        os: os.clone(),
        arch: arch.clone(),
        cuda: cuda.is_some() || !nvidia_gpus().is_empty() || jetson_l4t.is_some(),
        jetson: jetson_l4t.is_some(),
        raspberry_pi: model
            .as_deref()
            .is_some_and(|m| m.starts_with("Raspberry Pi")),
        hailo: hailo.clone(),
        xcode: xcode.is_some(),
    };
    Hardware {
        labels: node::labels(&found),
        os,
        arch,
        model,
        nvidia_driver,
        cuda,
        jetson_l4t,
        hailo,
        xcode,
    }
}

/// What the machine is (looked up again every ten minutes).
pub fn hardware() -> Hardware {
    let mut cached = HARDWARE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, h)) = cached.as_ref() {
        if at.elapsed() < HARDWARE_FOR {
            return h.clone();
        }
    }
    let h = look_up_hardware();
    *cached = Some((Instant::now(), h.clone()));
    h
}

fn gpus() -> Vec<Gpu> {
    let mut cached = GPUS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, g)) = cached.as_ref() {
        if at.elapsed() < GPUS_FOR {
            return g.clone();
        }
    }
    let g = nvidia_gpus();
    *cached = Some((Instant::now(), g.clone()));
    g
}

fn load() -> [f64; 3] {
    let mut l = [0f64; 3];
    // SAFETY: getloadavg writes at most 3 values into the array.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    if n < 3 {
        return [0.0; 3];
    }
    l
}

/// Seconds since the machine started.
pub fn uptime_s() -> Option<u64> {
    if cfg!(target_os = "macos") {
        // `{ sec = 1761634000, usec = 0 } Tue Oct 28 …`
        let text = output("sysctl", &["-n", "kern.boottime"])?;
        let sec: u64 = text
            .split("sec = ")
            .nth(1)?
            .split(',')
            .next()?
            .trim()
            .parse()
            .ok()?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs();
        return now.checked_sub(sec);
    }
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    text.split('.').next()?.trim().parse().ok()
}

pub fn disk(path: &Path) -> Option<Disk> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: statvfs fills the struct for a valid C path.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize as u64;
    Some(Disk {
        path: path.display().to_string(),
        total_bytes: s.f_blocks as u64 * block,
        free_bytes: s.f_bavail as u64 * block,
    })
}

/// Network cards with an address: `ip` and /sys on Linux, `ifconfig` on macOS.
pub fn nics() -> Vec<Nic> {
    if cfg!(target_os = "macos") {
        let mut list = output("ifconfig", &[])
            .map(|t| node::parse_ifconfig_nics(&t))
            .unwrap_or_default();
        // A Mac's wired ports: the hardware ports `networksetup` calls Ethernet or Thunderbolt.
        let ports = output("networksetup", &["-listallhardwareports"]).unwrap_or_default();
        for n in &mut list {
            n.wired = ports.split("\n\n").any(|block| {
                block.contains(&format!("Device: {}", n.name)) && !block.contains("Wi-Fi")
            });
        }
        return list;
    }
    let text = output("ip", &["-o", "-4", "addr"]).unwrap_or_default();
    node::parse_ip_addr(&text)
        .into_iter()
        .filter_map(|(name, ip, prefix)| {
            let sys = Path::new("/sys/class/net").join(&name);
            let mac = read_trimmed(sys.join("address").to_str()?)?;
            Some(Nic {
                vpn: node::is_vpn(&name),
                wired: !sys.join("wireless").exists() && sys.join("device").exists(),
                broadcast: node::broadcast(ip, prefix).to_string(),
                name,
                mac,
                ipv4: ip.to_string(),
                prefix,
            })
        })
        .collect()
}

/// People logged in to the machine right now.
pub fn logins() -> Vec<Login> {
    output("who", &[])
        .map(|t| node::parse_who(&t))
        .unwrap_or_default()
}

impl Manager {
    pub fn session_counts(&self) -> SessionCounts {
        use crate::core::models::SessionStatus;
        let sessions = self.sessions();
        let count = |f: &dyn Fn(&crate::services::sessions::SessionInfo) -> bool| {
            sessions.iter().filter(|s| f(s)).count()
        };
        SessionCounts {
            total: sessions.len(),
            running: count(&|s| s.status == SessionStatus::Running),
            paused: count(&|s| s.status == SessionStatus::Paused),
            working: count(&|s| s.activity == Some(crate::core::activity::Activity::Working)),
            last_working_at: crate::services::awake::last_working_at(),
            jobs_running: self.running_jobs().len(),
        }
    }

    /// Every fact about this machine (blocking: may run `nvidia-smi` and friends).
    pub fn node(&self) -> Node {
        let (mem_total_bytes, mem_available_bytes) = crate::services::stats::system_memory();
        let logins = logins();
        Node {
            in_use: !self.blockers_with(&logins).is_empty(),
            host: self.host_name().to_string(),
            version: env!("CARGO_PKG_VERSION"),
            hardware: hardware(),
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
            load: load(),
            uptime_s: uptime_s(),
            mem_total_bytes,
            mem_available_bytes,
            disk: disk(&self.cfg.user_home),
            gpus: gpus(),
            nics: nics(),
            sessions: self.session_counts(),
            logins,
            awake: crate::services::awake::status(self.settings().keep_awake),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64),
        }
    }
}
