//! What a machine is and can do right now (`GET /api/node`), parsed from the tools that
//! know: `nvidia-smi`, the Jetson and Raspberry Pi device tree, `hailortcli`, `xcodebuild`,
//! `who`. A fleet coordinator matches jobs on these facts and on the labels made from them.

use serde::Serialize;

/// One NVIDIA GPU (`nvidia-smi --query-gpu`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Gpu {
    pub name: String,
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    pub utilization_percent: Option<u32>,
    pub temperature_c: Option<u32>,
}

/// The query kimo runs: `nvidia-smi --query-gpu=<this> --format=csv,noheader,nounits`.
pub const NVIDIA_QUERY: &str = "name,memory.total,memory.used,utilization.gpu,temperature.gpu";

/// GPUs from the CSV lines of [`NVIDIA_QUERY`]. Missing figures (`[N/A]`, on Jetson and some
/// laptops) are left out rather than guessed.
pub fn parse_nvidia_smi(csv: &str) -> Vec<Gpu> {
    csv.lines()
        .filter_map(|line| {
            let f: Vec<&str> = line.split(',').map(str::trim).collect();
            if f.len() < 5 || f[0].is_empty() {
                return None;
            }
            let num = |s: &str| s.parse::<u64>().ok();
            Some(Gpu {
                name: f[0].to_string(),
                memory_total_mb: num(f[1])?,
                memory_used_mb: num(f[2]).unwrap_or(0),
                utilization_percent: f[3].parse().ok(),
                temperature_c: f[4].parse().ok(),
            })
        })
        .collect()
}

/// `Driver Version: 580.82.07   CUDA Version: 13.0` from plain `nvidia-smi`.
pub fn parse_nvidia_versions(text: &str) -> (Option<String>, Option<String>) {
    let after = |key: &str| {
        let rest = &text[text.find(key)? + key.len()..];
        let v: String = rest
            .trim_start_matches([' ', ':'])
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '.')
            .collect();
        (!v.is_empty()).then_some(v)
    };
    (after("Driver Version"), after("CUDA Version"))
}

/// The L4T release of a Jetson from `/etc/nv_tegra_release`:
/// `# R36 (release), REVISION: 4.3, …` → `36.4.3`.
pub fn parse_tegra_release(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("REVISION"))?;
    let major: String = line
        .split_once('R')?
        .1
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let revision: String = line
        .split_once("REVISION:")?
        .1
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    (!major.is_empty() && !revision.is_empty()).then(|| format!("{major}.{revision}"))
}

/// `Xcode 26.0.1\nBuild version 17A400` → `26.0.1`.
pub fn parse_xcode(text: &str) -> Option<String> {
    let v = text.lines().next()?.strip_prefix("Xcode ")?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// The accelerator a `hailortcli fw-control identify` reports: `Device Architecture: HAILO10H`.
pub fn parse_hailo_architecture(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("Device Architecture:"))
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
}

/// Someone logged in to the machine (a desktop, a console, SSH), from `who`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Login {
    pub user: String,
    /// `tty2`, `:0`, `pts/0`, `console`.
    pub line: String,
    /// Where from (an address for SSH), when `who` says.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

/// `who` lines: `jordi    tty2   2026-10-28 10:00 (tty2)` or
/// `jordi    pts/0  2026-10-28 10:01 (10.147.17.5)`.
pub fn parse_who(text: &str) -> Vec<Login> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let user = words.next()?.to_string();
            let line_name = words.next()?.to_string();
            let from = line
                .rsplit_once('(')
                .and_then(|(_, r)| r.strip_suffix(')'))
                .map(str::to_string)
                .filter(|f| *f != line_name && !f.is_empty());
            Some(Login {
                user,
                line: line_name,
                from,
            })
        })
        .collect()
}

/// The accelerators and toolchains found, as [`labels`] reads them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Found {
    pub os: String,
    pub arch: String,
    pub cuda: bool,
    pub jetson: bool,
    pub raspberry_pi: bool,
    pub hailo: Option<String>,
    pub xcode: bool,
}

/// Labels a coordinator can match jobs on: `x86_64`/`arm64`, `linux`/`macos`, `cuda`,
/// `jetson` + `tensorrt`, `rpi`, `hailo-10h`/`hailo-8`, `xcode`.
pub fn labels(f: &Found) -> Vec<String> {
    let mut l = vec![
        match f.arch.as_str() {
            "aarch64" => "arm64".to_string(),
            a => a.to_string(),
        },
        f.os.clone(),
    ];
    if f.cuda {
        l.push("cuda".into());
    }
    if f.jetson {
        l.extend(["jetson".into(), "tensorrt".into()]);
    }
    if f.raspberry_pi {
        l.push("rpi".into());
    }
    if let Some(arch) = &f.hailo {
        // HAILO10H → hailo-10h, HAILO8L → hailo-8l
        let rest = arch.to_ascii_lowercase();
        let rest = rest.strip_prefix("hailo").unwrap_or(&rest);
        l.push(format!("hailo-{rest}"));
    }
    if f.xcode {
        l.push("xcode".into());
    }
    l.retain(|x| !x.is_empty());
    l.dedup();
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia() {
        let gpus = parse_nvidia_smi(
            "NVIDIA GeForce RTX 3060, 12288, 517, 3, 41\nOrin, [N/A], [N/A], [N/A], [N/A]\n\n",
        );
        assert_eq!(
            gpus,
            vec![Gpu {
                name: "NVIDIA GeForce RTX 3060".into(),
                memory_total_mb: 12288,
                memory_used_mb: 517,
                utilization_percent: Some(3),
                temperature_c: Some(41),
            }]
        );
        let header =
            "| NVIDIA-SMI 580.82.07   Driver Version: 580.82.07   CUDA Version: 13.0     |";
        assert_eq!(
            parse_nvidia_versions(header),
            (Some("580.82.07".into()), Some("13.0".into()))
        );
        assert_eq!(parse_nvidia_versions("nothing"), (None, None));
    }

    #[test]
    fn jetson_xcode_hailo() {
        assert_eq!(
            parse_tegra_release(
                "# R36 (release), REVISION: 4.3, GCID: 38968081, BOARD: generic, EABI: aarch64"
            )
            .as_deref(),
            Some("36.4.3")
        );
        assert_eq!(parse_tegra_release("garbage"), None);
        assert_eq!(
            parse_xcode("Xcode 26.0.1\nBuild version 17A400\n").as_deref(),
            Some("26.0.1")
        );
        assert_eq!(parse_xcode("xcode-select: error"), None);
        assert_eq!(
            parse_hailo_architecture(
                "Executing on device: 0001:01:00.0\nFirmware Version: 5.1.0\nDevice Architecture: HAILO10H\n"
            )
            .as_deref(),
            Some("HAILO10H")
        );
    }

    #[test]
    fn logins() {
        let who = "jordi    tty2         2026-10-28 10:00 (tty2)\n\
                   jordi    pts/0        2026-10-28 10:01 (10.147.17.5)\n\
                   jordi    console  Oct 28 09:12 \n";
        let l = parse_who(who);
        assert_eq!(l.len(), 3);
        assert_eq!(l[0].line, "tty2");
        assert_eq!(l[0].from, None);
        assert_eq!(l[1].from.as_deref(), Some("10.147.17.5"));
        assert_eq!(l[2].line, "console");
        assert!(parse_who("").is_empty());
    }

    #[test]
    fn labels_from_facts() {
        let nuc = Found {
            os: "linux".into(),
            arch: "x86_64".into(),
            cuda: true,
            ..Default::default()
        };
        assert_eq!(labels(&nuc), ["x86_64", "linux", "cuda"]);
        let pi = Found {
            os: "linux".into(),
            arch: "aarch64".into(),
            raspberry_pi: true,
            hailo: Some("HAILO10H".into()),
            ..Default::default()
        };
        assert_eq!(labels(&pi), ["arm64", "linux", "rpi", "hailo-10h"]);
        let jetson = Found {
            os: "linux".into(),
            arch: "aarch64".into(),
            cuda: true,
            jetson: true,
            ..Default::default()
        };
        assert_eq!(
            labels(&jetson),
            ["arm64", "linux", "cuda", "jetson", "tensorrt"]
        );
        let mac = Found {
            os: "macos".into(),
            arch: "aarch64".into(),
            xcode: true,
            ..Default::default()
        };
        assert_eq!(labels(&mac), ["arm64", "macos", "xcode"]);
    }
}
