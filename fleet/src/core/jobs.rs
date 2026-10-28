//! Jobs and workflows as the coordinator sees them: what to run, what it needs, and how it
//! moves from `blocked` to `succeeded`. A command job and a headless agent job are the same
//! kind of job with another runner; an approval step waits for the owner.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Runner {
    /// A shell line (`run:`).
    #[default]
    Command,
    /// A headless agent (`claude -p`, `codex exec`) with `prompt:`.
    Agent,
    /// Waits for the owner to approve, on the coordinator.
    Approval,
}

/// What a machine must be or have.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_gb: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vram_gb: Option<f64>,
}

/// What a job holds while it runs (exclusively).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Need {
    #[serde(default)]
    pub gpu: u32,
    /// Default 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_slots: Option<u32>,
}

impl Need {
    pub fn cpu_slots(&self) -> u32 {
        self.cpu_slots.unwrap_or(1)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobPower {
    /// Wake a machine that is off when it is the best one (default yes).
    #[serde(default = "yes")]
    pub wake_if_needed: bool,
}

fn yes() -> bool {
    true
}

impl Default for JobPower {
    fn default() -> Self {
        Self {
            wake_if_needed: true,
        }
    }
}

/// One job, or one step of a workflow, as written in YAML or JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobSpec {
    pub task: String,
    #[serde(default)]
    pub runner: Runner,
    /// The shell line of a command job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The prompt of an agent job.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `claude` (default) or `codex`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `yolo`: the agent skips its approvals (unattended work needs it); default: the
    /// machine's agent settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
    /// Run in a fresh checkout of a commit on the machine (instead of `cwd`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git: Option<GitSource>,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default)]
    pub resources: Need,
    /// Steps of the workflow that must succeed first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub needs: Vec<String>,
    /// Files from earlier steps, put in place before it starts: `name: step/path`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, String>,
    /// Files it makes (relative to its folder), kept by the coordinator.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    /// The folder it runs in on the machine (default: the machine's home).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    /// `30m`, `2h`, `90s`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<String>,
    #[serde(default)]
    pub retries: u32,
    /// Its failure does not stop the workflow.
    #[serde(default)]
    pub optional: bool,
    /// The machine to use when it qualifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefer: Option<String>,
    /// The only machine it may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    /// Not started before then (Unix ms); its machine is woken a little ahead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<u64>,
    #[serde(default)]
    pub power: JobPower,
    /// What an approval shows (files of earlier steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub show: Vec<String>,
}

/// A job's checkout: a repository on the machine or a URL, at a commit, maybe on a branch
/// of its own (`{{job}}` and `{{workflow}}` in it become the ids).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitSource {
    pub repo: String,
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workflow {
    pub workflow: String,
    pub steps: Vec<JobSpec>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    /// Waiting for the steps it needs.
    Blocked,
    /// Ready to be placed on a machine.
    Ready,
    /// No machine qualifies now (asleep laptop, all busy).
    WaitingDevice,
    /// Its machine is booting for it.
    Waking,
    /// Sent to the machine.
    Dispatched,
    Running,
    AwaitingApproval,
    Succeeded,
    Failed,
    Cancelled,
    /// Not run: a step it needs failed.
    Skipped,
}

impl JobState {
    pub fn finished(self) -> bool {
        matches!(
            self,
            JobState::Succeeded | JobState::Failed | JobState::Cancelled | JobState::Skipped
        )
    }

    /// Placed on a machine (a lease on it).
    pub fn on_device(self) -> bool {
        matches!(
            self,
            JobState::Waking | JobState::Dispatched | JobState::Running
        )
    }
}

/// `30m`, `2h`, `1h30m`, `90s`, `45` (seconds) → seconds.
pub fn parse_duration(text: &str) -> Option<u64> {
    let text = text.trim();
    if let Ok(s) = text.parse::<u64>() {
        return Some(s);
    }
    let (mut total, mut number) = (0u64, String::new());
    for c in text.chars() {
        if c.is_ascii_digit() {
            number.push(c);
            continue;
        }
        let n: u64 = number.parse().ok()?;
        number.clear();
        total += n * match c {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return None,
        };
    }
    (number.is_empty() && total > 0).then_some(total)
}

/// Check one job: the problems a person can fix, in plain words.
pub fn check(spec: &JobSpec) -> Result<(), String> {
    let task = &spec.task;
    if task.is_empty()
        || task.len() > 64
        || !task
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!(
            "'{task}' is not a task name (letters, digits, - and _)"
        ));
    }
    match spec.runner {
        Runner::Command if spec.run.as_deref().is_none_or(|r| r.trim().is_empty()) => {
            return Err(format!("{task}: a command job needs run:"))
        }
        Runner::Agent if spec.prompt.as_deref().is_none_or(|p| p.trim().is_empty()) => {
            return Err(format!("{task}: an agent job needs prompt:"))
        }
        Runner::Agent
            if !matches!(spec.agent.as_deref(), None | Some("claude") | Some("codex")) =>
        {
            return Err(format!("{task}: agent is claude or codex"))
        }
        _ => {}
    }
    if spec.git.is_some() && spec.cwd.is_some() {
        return Err(format!(
            "{task}: a job runs in its cwd or in a git checkout, not both"
        ));
    }
    if !matches!(
        spec.permissions.as_deref(),
        None | Some("yolo") | Some("manual")
    ) {
        return Err(format!("{task}: permissions is yolo or manual"));
    }
    if let Some(t) = &spec.timeout {
        if parse_duration(t).is_none() {
            return Err(format!("{task}: timeout '{t}' is not a duration like 30m"));
        }
    }
    for (name, from) in &spec.inputs {
        if !from.contains('/') || from.starts_with('/') || from.contains("..") {
            return Err(format!(
                "{task}: input {name} must be step/path (an output of an earlier step)"
            ));
        }
    }
    for out in &spec.outputs {
        if out.starts_with('/') || out.split('/').any(|p| p == "..") || out.is_empty() {
            return Err(format!(
                "{task}: output '{out}' must be a path inside the job's folder"
            ));
        }
    }
    Ok(())
}

/// Check a workflow: every step, unique names, needs that exist and no cycle. Returns the
/// steps in an order where each comes after what it needs.
pub fn check_workflow(w: &Workflow) -> Result<Vec<usize>, String> {
    if w.steps.is_empty() {
        return Err(format!("{}: a workflow needs steps", w.workflow));
    }
    let mut names = BTreeMap::new();
    for (i, s) in w.steps.iter().enumerate() {
        check(s)?;
        if names.insert(s.task.as_str(), i).is_some() {
            return Err(format!("two steps are called {}", s.task));
        }
    }
    for s in &w.steps {
        for n in &s.needs {
            if !names.contains_key(n.as_str()) {
                return Err(format!("{} needs {n}, which is not a step", s.task));
            }
        }
        for from in s.inputs.values() {
            let step = from.split('/').next().unwrap_or_default();
            if !s.needs.iter().any(|n| n == step) {
                return Err(format!(
                    "{}: its input {from} comes from {step}, which it does not need",
                    s.task
                ));
            }
        }
    }
    // Kahn's order: what is left at the end is a cycle.
    let mut order = Vec::new();
    let mut done = BTreeSet::new();
    while order.len() < w.steps.len() {
        let next = w.steps.iter().enumerate().find(|(i, s)| {
            !done.contains(i) && s.needs.iter().all(|n| done.contains(&names[n.as_str()]))
        });
        match next {
            Some((i, _)) => {
                done.insert(i);
                order.push(i);
            }
            None => {
                let left: Vec<&str> = w
                    .steps
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| !done.contains(i))
                    .map(|(_, s)| s.task.as_str())
                    .collect();
                return Err(format!("these steps need each other: {}", left.join(", ")));
            }
        }
    }
    Ok(order)
}

/// A step's state once the steps it needs have moved: blocked while any runs, skipped when a
/// needed step that is not optional ended without succeeding, ready when all are through.
pub fn unblocked(needs: &[(JobState, bool)]) -> JobState {
    if needs
        .iter()
        .any(|(s, optional)| !optional && s.finished() && *s != JobState::Succeeded)
    {
        return JobState::Skipped;
    }
    if needs.iter().all(|(s, _)| s.finished()) {
        JobState::Ready
    } else {
        JobState::Blocked
    }
}

/// Why a job's run ended badly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The job itself (a non-zero exit, a timeout).
    Job,
    /// The machine (unreachable, the job lost).
    Device,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retry {
    /// Again on the same machine.
    Same,
    /// On another machine.
    Elsewhere,
    No,
}

/// After a failed attempt (`attempt` counts from 1): job failures retry where they ran,
/// machine failures move on to the next machine.
pub fn retry(attempt: u32, retries: u32, why: Failure) -> Retry {
    if attempt > retries {
        return Retry::No;
    }
    match why {
        Failure::Job => Retry::Same,
        Failure::Device => Retry::Elsewhere,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VISION: &str = r#"
workflow: segmentation-to-ios
steps:
  - task: develop-model
    runner: agent
    agent: claude
    requires: { labels: [arm64, rpi] }
    prompt: "Implement the segmentation model in models/seg. Push a branch."
  - task: train
    needs: [develop-model]
    requires: { labels: [cuda], vram_gb: 10 }
    run: python train.py --export onnx
    outputs: [out/model.onnx]
  - task: bench-jetson
    needs: [train]
    requires: { labels: [jetson] }
    run: ./bench_trt.sh out/model.onnx
    inputs: { model: train/out/model.onnx }
    outputs: [results/jetson.json]
  - task: bench-pi5
    needs: [train]
    requires: { labels: [hailo-10h] }
    run: ./bench_hailo.sh out/model.onnx
    outputs: [results/pi5.json]
    optional: true
  - task: analyse
    runner: agent
    needs: [bench-jetson, bench-pi5]
    requires: { labels: [rpi] }
    prompt: "Compare results/*.json and write REPORT.md with a recommendation."
  - task: ios-build
    needs: [analyse]
    requires: { labels: [xcode] }
    run: ./scripts/ios_build.sh out/model.onnx
  - task: approve-release
    runner: approval
    needs: [ios-build]
    show: [REPORT.md, results/jetson.json]
"#;

    #[test]
    fn the_vision_workflow_from_the_design() {
        let w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        let order = check_workflow(&w).unwrap();
        let names: Vec<&str> = order.iter().map(|&i| w.steps[i].task.as_str()).collect();
        assert_eq!(names[0], "develop-model");
        assert_eq!(names[1], "train");
        assert_eq!(*names.last().unwrap(), "approve-release");
        assert_eq!(w.steps[1].requires.vram_gb, Some(10.0));
        assert!(w.steps[3].optional);
        assert_eq!(w.steps[6].runner, Runner::Approval);
    }

    #[test]
    fn workflow_mistakes() {
        let mut w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        w.steps[0].needs = vec!["approve-release".into()];
        assert!(check_workflow(&w).unwrap_err().contains("need each other"));
        let mut w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        w.steps[1].needs = vec!["nope".into()];
        assert!(check_workflow(&w).unwrap_err().contains("not a step"));
        let mut w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        w.steps[2]
            .inputs
            .insert("x".into(), "analyse/REPORT.md".into());
        assert!(check_workflow(&w).unwrap_err().contains("does not need"));
        let mut w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        w.steps[1].run = None;
        assert!(check_workflow(&w).unwrap_err().contains("needs run:"));
        let mut w: Workflow = serde_yaml_ng::from_str(VISION).unwrap();
        w.steps[1].outputs = vec!["../escape".into()];
        assert!(check_workflow(&w)
            .unwrap_err()
            .contains("inside the job's folder"));
        assert!(serde_yaml_ng::from_str::<Workflow>("workflow: x\nsteps: []\nextra: 1").is_err());
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("45"), Some(45));
        assert_eq!(parse_duration("2d"), Some(172_800));
        for bad in ["", "m", "10x", "1h3"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }

    #[test]
    fn steps_move_on_when_what_they_need_ends() {
        use JobState::*;
        assert_eq!(unblocked(&[]), Ready);
        assert_eq!(unblocked(&[(Succeeded, false), (Running, false)]), Blocked);
        assert_eq!(unblocked(&[(Succeeded, false), (Succeeded, false)]), Ready);
        // An optional step may fail.
        assert_eq!(unblocked(&[(Succeeded, false), (Failed, true)]), Ready);
        assert_eq!(unblocked(&[(Failed, false), (Running, false)]), Skipped);
        assert_eq!(unblocked(&[(Skipped, true)]), Ready);
        assert_eq!(unblocked(&[(Skipped, false)]), Skipped);
        assert_eq!(unblocked(&[(Cancelled, false)]), Skipped);
    }

    #[test]
    fn retries() {
        assert_eq!(retry(1, 1, Failure::Job), Retry::Same);
        assert_eq!(retry(1, 1, Failure::Device), Retry::Elsewhere);
        assert_eq!(retry(2, 1, Failure::Job), Retry::No);
        assert_eq!(retry(1, 0, Failure::Device), Retry::No);
    }
}
