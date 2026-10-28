//! Choosing a machine for a job: hard filters (labels, memory, VRAM, free GPU and slots, a
//! pinned machine), then a preference order (already up before off, cheaper to keep on,
//! fewer jobs). With a handful of machines there is nothing to pack, only to rank.

use super::jobs::JobSpec;
use super::registry::Resources;

/// A machine as the scheduler sees it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Candidate {
    pub id: String,
    /// The registry's labels and those its kimo reports.
    pub labels: Vec<String>,
    pub resources: Resources,
    /// Memory and the largest GPU's memory it reports (GB).
    pub memory_gb: Option<f64>,
    pub vram_gb: Option<f64>,
    /// GPUs and slots held by the jobs placed on it.
    pub used_gpu: u32,
    pub used_slots: u32,
    pub jobs: usize,
    /// Up and taking work now.
    pub up: bool,
    /// Off, and may be woken for work.
    pub wakeable: bool,
    /// Watts when idle.
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// Send it there now.
    Run(String),
    /// Wake that machine for it (its resources are reserved meanwhile).
    Wake(String),
    /// Nowhere now, and why.
    Wait(String),
}

/// Could this machine ever run the job (whatever it is doing now)?
pub fn qualifies(spec: &JobSpec, c: &Candidate) -> bool {
    if spec.device.as_ref().is_some_and(|d| *d != c.id) {
        return false;
    }
    if !spec.requires.labels.iter().all(|l| c.labels.contains(l)) {
        return false;
    }
    let most = |a: Option<f64>, b: Option<f64>| match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    };
    if let Some(gb) = spec.requires.memory_gb {
        if most(c.resources.memory_gb, c.memory_gb).is_none_or(|m| m + 0.01 < gb) {
            return false;
        }
    }
    if let Some(gb) = spec.requires.vram_gb {
        if most(c.resources.vram_gb, c.vram_gb).is_none_or(|v| v + 0.01 < gb) {
            return false;
        }
    }
    spec.resources.gpu <= c.resources.gpu && spec.resources.cpu_slots() <= c.resources.cpu_slots
}

/// Are its GPUs and slots free for the job now?
fn has_room(spec: &JobSpec, c: &Candidate) -> bool {
    c.used_gpu + spec.resources.gpu <= c.resources.gpu
        && c.used_slots + spec.resources.cpu_slots() <= c.resources.cpu_slots
}

/// Where the job goes. `exclude`: machines it already failed on (the machine's fault).
pub fn choose(spec: &JobSpec, machines: &[Candidate], exclude: &[String]) -> Choice {
    let able: Vec<&Candidate> = machines
        .iter()
        .filter(|c| !exclude.contains(&c.id) && qualifies(spec, c))
        .collect();
    if able.is_empty() {
        let mut what = vec![];
        if !spec.requires.labels.is_empty() {
            what.push(format!("labels {}", spec.requires.labels.join(", ")));
        }
        if let Some(gb) = spec.requires.vram_gb {
            what.push(format!("{gb} GB of VRAM"));
        }
        if let Some(gb) = spec.requires.memory_gb {
            what.push(format!("{gb} GB of memory"));
        }
        if spec.resources.gpu > 0 {
            what.push(format!("{} GPU", spec.resources.gpu));
        }
        if let Some(d) = &spec.device {
            what.push(format!("to be {d}"));
        }
        return Choice::Wait(if what.is_empty() {
            "no machine can run it".into()
        } else {
            format!("no machine has {}", what.join(" and "))
        });
    }
    let mut free: Vec<&&Candidate> = able
        .iter()
        .filter(|c| has_room(spec, c) && (c.up || (c.wakeable && spec.power.wake_if_needed)))
        .collect();
    if free.is_empty() {
        let names: Vec<&str> = able.iter().map(|c| c.id.as_str()).collect();
        return Choice::Wait(if able.iter().any(|c| c.up) {
            format!("busy: {}", names.join(", "))
        } else {
            format!("waiting for {} to be on", names.join(" or "))
        });
    }
    free.sort_by(|a, b| {
        b.up.cmp(&a.up)
            .then(a.cost.total_cmp(&b.cost))
            .then(a.jobs.cmp(&b.jobs))
            .then(a.id.cmp(&b.id))
    });
    let best = spec
        .prefer
        .as_ref()
        .and_then(|p| free.iter().find(|c| c.id == *p))
        .unwrap_or(&free[0]);
    if best.up {
        Choice::Run(best.id.clone())
    } else {
        Choice::Wake(best.id.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::jobs::{Need, Requires};

    fn machine(id: &str, labels: &[&str], up: bool, cost: f64) -> Candidate {
        Candidate {
            id: id.into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            resources: Resources {
                cpu_slots: 2,
                ..Default::default()
            },
            up,
            wakeable: !up,
            cost,
            ..Default::default()
        }
    }

    fn fleet() -> Vec<Candidate> {
        let mut extreme = machine("nuc-extreme", &["x86_64", "linux", "cuda"], false, 40.0);
        extreme.resources.gpu = 1;
        extreme.resources.vram_gb = Some(12.0);
        let mut enthusiast = machine("nuc-enthusiast", &["x86_64", "linux", "cuda"], false, 18.0);
        enthusiast.resources.gpu = 1;
        enthusiast.resources.vram_gb = Some(6.0);
        let mut mac = machine("macbook-air", &["macos", "arm64", "xcode"], false, 3.0);
        mac.wakeable = false;
        vec![
            extreme,
            enthusiast,
            machine("pi5", &["arm64", "linux", "rpi", "hailo-10h"], true, 3.0),
            machine("jetson", &["arm64", "linux", "jetson", "cuda"], true, 5.0),
            mac,
        ]
    }

    fn job(labels: &[&str]) -> JobSpec {
        JobSpec {
            task: "t".into(),
            run: Some("true".into()),
            requires: Requires {
                labels: labels.iter().map(|l| l.to_string()).collect(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn up_machines_first_then_the_cheapest_to_wake() {
        // CUDA: the Jetson is on, so it goes there.
        assert_eq!(
            choose(&job(&["cuda"]), &fleet(), &[]),
            Choice::Run("jetson".into())
        );
        // x86 only: both NUCs are off; the cheaper one wakes.
        assert_eq!(
            choose(&job(&["x86_64"]), &fleet(), &[]),
            Choice::Wake("nuc-enthusiast".into())
        );
        // 10 GB of VRAM: only the Extreme.
        let mut big = job(&["cuda"]);
        big.requires.vram_gb = Some(10.0);
        assert_eq!(
            choose(&big, &fleet(), &[]),
            Choice::Wake("nuc-extreme".into())
        );
        // Preferred, when it qualifies.
        let mut pref = job(&["x86_64"]);
        pref.prefer = Some("nuc-extreme".into());
        assert_eq!(
            choose(&pref, &fleet(), &[]),
            Choice::Wake("nuc-extreme".into())
        );
        // A machine it failed on is left out.
        assert_eq!(
            choose(&job(&["x86_64"]), &fleet(), &["nuc-enthusiast".into()]),
            Choice::Wake("nuc-extreme".into())
        );
    }

    #[test]
    fn waiting_when_nothing_can_run_it_now() {
        // The MacBook cannot be woken: wait for it, and say so.
        assert_eq!(
            choose(&job(&["xcode"]), &fleet(), &[]),
            Choice::Wait("waiting for macbook-air to be on".into())
        );
        assert_eq!(
            choose(&job(&["windows"]), &fleet(), &[]),
            Choice::Wait("no machine has labels windows".into())
        );
        // Not woken when the job says so.
        let mut no_wake = job(&["x86_64"]);
        no_wake.power.wake_if_needed = false;
        assert!(matches!(choose(&no_wake, &fleet(), &[]), Choice::Wait(_)));
    }

    #[test]
    fn gpus_and_slots_are_held_exclusively() {
        let mut m = fleet();
        let mut gpu_job = job(&["cuda"]);
        gpu_job.requires.vram_gb = Some(10.0);
        gpu_job.resources = Need {
            gpu: 1,
            cpu_slots: None,
        };
        m[0].up = true;
        m[0].wakeable = false;
        assert_eq!(choose(&gpu_job, &m, &[]), Choice::Run("nuc-extreme".into()));
        m[0].used_gpu = 1;
        m[0].jobs = 1;
        assert_eq!(
            choose(&gpu_job, &m, &[]),
            Choice::Wait("busy: nuc-extreme".into())
        );
        // The Jetson has no GPU in the registry to reserve: a GPU job never goes there.
        let mut small = job(&["cuda"]);
        small.resources.gpu = 1;
        assert_eq!(
            choose(&small, &m, &[]),
            Choice::Wake("nuc-enthusiast".into())
        );
        // Slots.
        m[2].used_slots = 2;
        assert_eq!(
            choose(&job(&["rpi"]), &m, &[]),
            Choice::Wait("busy: pi5".into())
        );
    }
}
