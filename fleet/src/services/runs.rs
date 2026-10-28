//! A job's runs on machines, kept honest when the network is not. A run's id
//! (`<job>-<attempt>`) is written down before anything is sent, so an answer that never comes
//! is settled by asking the machine about that id instead of guessing:
//!
//! - the machine has it: the run goes on (it is adopted);
//! - the machine is sure it has not: the job is placed again, with the same id (a copy
//!   arriving late is then found, never run twice under two names);
//! - the machine cannot be asked: the job stays *dispatched* and keeps its lease, until it
//!   answers or is lost.
//!
//! A run that may still be alive when the job moves on (its machine stopped answering) is
//! remembered as an orphan: cancelled when its machine answers, its GPU and slots held until
//! then. Cancelling keeps the lease too, until the machine confirms the run ended.

use super::*;

/// What sending a run to its machine came to.
enum Sent {
    /// It runs there.
    Started,
    /// Certainly not there (the machine said so): it can go elsewhere.
    NotStarted(String),
    /// The machine cannot run it (a folder or repository it lacks, a bad request).
    Refused(String),
    /// Nobody knows yet.
    Unknown(String),
}

/// A run's state on its machine (`/api/jobs/{id}`).
fn worker_state(w: &Value) -> &str {
    w["state"].as_str().unwrap_or("")
}

fn finished(state: &str) -> bool {
    !matches!(state, "staged" | "starting" | "running")
}

impl Fleet {
    /// What a machine knows of a run: `Ok(Some)` it has it, `Ok(None)` it has not,
    /// `Err` it cannot be asked.
    fn ask_run(&self, dev: &str, wid: &str) -> Result<Option<Value>> {
        let (d, key) = self.device_of(dev)?;
        let key = key.ok_or_else(|| anyhow!("{dev} is not paired"))?;
        match net::kimo(&d.kimo, Some(&key)).raw(
            "GET",
            &format!("/jobs/{wid}"),
            vec![],
            None,
            net::CALL_SECS,
        )? {
            (200, _, body) => Ok(Some(serde_json::from_slice(&body)?)),
            (404, _, _) => Ok(None),
            (status, _, _) => bail!("{dev} answered {status}"),
        }
    }

    fn cancel_run(&self, dev: &str, wid: &str) -> Result<Value> {
        let (d, key) = self.device_of(dev)?;
        let key = key.ok_or_else(|| anyhow!("{dev} is not paired"))?;
        net::kimo(&d.kimo, Some(&key)).call_timeout(
            "POST",
            &format!("/jobs/{wid}/cancel"),
            None,
            net::CALL_SECS,
        )
    }

    /// The artifacts a job's inputs name; an error when one is not there (it never runs
    /// without them).
    fn inputs_of(&self, job: &Job) -> Result<Vec<(String, String, Artifact)>, String> {
        let mut out = vec![];
        for (name, from) in &job.spec.inputs {
            let (step, path) = from.split_once('/').unwrap_or_default();
            let art = self
                .step_artifact(job.workflow.as_deref(), step, path)
                .filter(|a| self.artifact_path(&a.sha256).is_some())
                .ok_or_else(|| format!("its input {from} is not available"))?;
            out.push((name.clone(), path.to_string(), art));
        }
        Ok(out)
    }

    /// Send a job to its machine (on a thread of its own).
    pub(super) fn dispatch(&self, id: &str) {
        let Ok(job) = self.job(id) else { return };
        let Some(dev) = job.device.clone() else {
            return;
        };
        let inputs = match self.inputs_of(&job) {
            Ok(i) => i,
            Err(why) => {
                self.update_job(id, |j| {
                    j.state = JobState::Failed;
                    j.ended_at = Some(now_ms());
                    j.reason = Some(why.clone());
                });
                self.notify(
                    Some(&dev),
                    Some(id),
                    &format!("{} failed: {why}", job.spec.task),
                );
                self.advance(now_ms());
                return;
            }
        };
        // Marked as being sent before its run is written down: following never takes a run
        // that is still on its way for one that is missing.
        self.lock().dispatching.insert(id.to_string());
        let Some(job) = self.update_job(id, |j| {
            if j.attempt == 0 || j.attempt_started {
                j.attempt += 1;
                j.attempt_started = false;
            }
            j.worker_job = Some(format!("{}-{}", j.id, j.attempt));
            j.unreachable_since = None;
        }) else {
            self.lock().dispatching.remove(id);
            return;
        };
        let wid = job.worker_job.clone().expect("written down");
        let sent = self.send(&job, &dev, &wid, &inputs);
        self.lock().dispatching.remove(id);
        self.settle(id, &dev, &wid, sent);
    }

    fn body(&self, job: &Job, wid: &str, inputs: &[(String, String, Artifact)]) -> Value {
        let id = &job.id;
        let mut env = job.spec.env.clone();
        let mut run = job.spec.run.clone();
        for (name, path, _) in inputs {
            env.insert(
                format!("KIMO_INPUT_{}", name.to_ascii_uppercase().replace('-', "_")),
                path.clone(),
            );
            if let Some(r) = run.as_mut() {
                for key in [
                    format!("{{{{inputs.{name}}}}}"),
                    format!("{{{{artifacts.{name}}}}}"),
                ] {
                    *r = r.replace(&key, path);
                }
            }
        }
        let mut body = json!({
            "id": wid,
            "name": job.spec.task,
            "env": env,
            "start": inputs.is_empty(),
        });
        if let Some(cwd) = &job.spec.cwd {
            body["cwd"] = json!(cwd);
        }
        if let Some(g) = &job.spec.git {
            let fill = |t: &str| {
                t.replace("{{job}}", id)
                    .replace("{{workflow}}", job.workflow.as_deref().unwrap_or(id))
            };
            let mut git = json!({ "repo": g.repo });
            if let Some(r) = &g.reference {
                git["ref"] = json!(fill(r));
            }
            if let Some(b) = &g.branch {
                // A later attempt gets a branch of its own (the first one exists already).
                let b = fill(b);
                git["branch"] = json!(if job.attempt > 1 {
                    format!("{b}-{}", job.attempt)
                } else {
                    b
                });
            }
            body["git"] = git;
        }
        if let Some(t) = job.spec.timeout.as_deref().and_then(jobs::parse_duration) {
            body["timeout_s"] = json!(t);
        }
        match job.spec.runner {
            Runner::Agent => {
                let mut agent = json!({
                    "kind": job.spec.agent.as_deref().unwrap_or("claude"),
                    "prompt": job.spec.prompt.clone().unwrap_or_default(),
                });
                if let Some(m) = &job.spec.model {
                    agent["model"] = json!(m);
                }
                if let Some(e) = &job.spec.effort {
                    agent["effort"] = json!(e);
                }
                if let Some(p) = &job.spec.permissions {
                    agent["permissions"] = json!(p);
                }
                body["agent"] = agent;
            }
            _ => body["command"] = json!(run.unwrap_or_default()),
        }
        body
    }

    fn send(&self, job: &Job, dev: &str, wid: &str, inputs: &[(String, String, Artifact)]) -> Sent {
        let (d, key) = match self.device_of(dev) {
            Ok((d, Some(key))) => (d, key),
            Ok((_, None)) => return Sent::NotStarted(format!("{dev} is not paired")),
            Err(e) => return Sent::NotStarted(format!("{e:#}")),
        };
        if job.spec.runner == Runner::Approval {
            return Sent::Refused("approvals do not run on machines".into());
        }
        let client = net::kimo(&d.kimo, Some(&key));
        let body = self.body(job, wid, inputs);
        let created = match client.raw(
            "POST",
            "/jobs",
            serde_json::to_vec(&body).unwrap_or_default(),
            Some("application/json"),
            30,
        ) {
            Ok((200 | 201, _, _)) => true,
            Ok((status, _, answer)) if (400..500).contains(&status) => {
                let e: Value = serde_json::from_slice(&answer).unwrap_or_default();
                let e = e["error"].as_str().unwrap_or("refused").to_string();
                // The same run sent before (an answer lost on the way): it is there.
                if !e.starts_with("there is already a job") {
                    return match self.ask_run(dev, wid) {
                        Ok(None) => Sent::Refused(format!("{dev}: {e}")),
                        Ok(Some(_)) | Err(_) => Sent::Unknown(format!("{dev}: {e}")),
                    };
                }
                false
            }
            // No answer, or a failure on the way back: ask before believing anything.
            Ok((status, _, _)) => {
                return self.after_doubt(dev, wid, inputs, format!("{dev} answered {status}"))
            }
            Err(e) => return self.after_doubt(dev, wid, inputs, format!("{e:#}")),
        };
        if !created {
            return self.after_doubt(dev, wid, inputs, "sent before".into());
        }
        if inputs.is_empty() {
            return Sent::Started;
        }
        self.inputs_and_start(&d.kimo, &key, dev, wid, inputs)
    }

    /// The machine was not clear: what it knows of the run decides.
    fn after_doubt(
        &self,
        dev: &str,
        wid: &str,
        inputs: &[(String, String, Artifact)],
        why: String,
    ) -> Sent {
        match self.ask_run(dev, wid) {
            Ok(None) => Sent::NotStarted(why),
            Ok(Some(w)) if worker_state(&w) == "staged" && !inputs.is_empty() => {
                match self.device_of(dev) {
                    Ok((d, Some(key))) => self.inputs_and_start(&d.kimo, &key, dev, wid, inputs),
                    _ => Sent::Unknown(why),
                }
            }
            Ok(Some(_)) => Sent::Started,
            Err(e) => Sent::Unknown(format!("{why}; asking again: {e:#}")),
        }
    }

    /// A staged run: its inputs, then start. Half way, it is cancelled (a cancelled staged
    /// run never starts) before the job goes anywhere else.
    fn inputs_and_start(
        &self,
        url: &str,
        key: &str,
        dev: &str,
        wid: &str,
        inputs: &[(String, String, Artifact)],
    ) -> Sent {
        let client = net::kimo(url, Some(key));
        let mut problem = None;
        for (_, path, art) in inputs {
            if let Err(e) = self.upload(url, key, wid, path, art) {
                problem = Some(format!("sending {path} to {dev}: {e:#}"));
                break;
            }
        }
        if problem.is_none() {
            match client.call_timeout("POST", &format!("/jobs/{wid}/start"), None, 30) {
                Ok(_) => return Sent::Started,
                Err(e) => problem = Some(format!("starting it on {dev}: {e:#}")),
            }
        }
        let why = problem.unwrap_or_default();
        match self.ask_run(dev, wid) {
            Ok(Some(w)) if worker_state(&w) == "staged" => match self.cancel_run(dev, wid) {
                Ok(c) if worker_state(&c) == "cancelled" => Sent::NotStarted(why),
                _ => Sent::Unknown(why),
            },
            Ok(Some(w)) if matches!(worker_state(&w), "starting" | "running") => Sent::Started,
            Ok(Some(w)) if worker_state(&w) == "cancelled" => Sent::NotStarted(why),
            Ok(Some(_)) => Sent::Started,
            Ok(None) => Sent::NotStarted(why),
            Err(_) => Sent::Unknown(why),
        }
    }

    /// Record what sending came to.
    fn settle(&self, id: &str, dev: &str, wid: &str, sent: Sent) {
        let now = now_ms();
        let cancel = self
            .job(id)
            .is_ok_and(|j| j.cancel_requested || j.state == JobState::Cancelled);
        match sent {
            Sent::Started => {
                self.update_job(id, |j| {
                    j.state = JobState::Running;
                    j.attempt_started = true;
                    j.started_at = Some(now);
                    j.unreachable_since = None;
                    if !j.cancel_requested {
                        j.reason = None;
                    }
                });
                if cancel {
                    // Cancelled while it was on its way: ended over there too.
                    let _ = self.cancel_run(dev, wid);
                }
                let job = self.job(id).ok();
                self.event(
                    Some(dev),
                    Some(id),
                    "job",
                    &format!(
                        "running {} (attempt {})",
                        job.as_ref().map_or("", |j| j.spec.task.as_str()),
                        job.as_ref().map_or(0, |j| j.attempt)
                    ),
                );
            }
            Sent::NotStarted(why) if cancel => self.cancelled_now(id, &why),
            Sent::NotStarted(why) => {
                self.update_job(id, |j| {
                    j.state = JobState::Ready;
                    j.device = None;
                    j.reason = Some(format!("not started on {dev}: {why}"));
                    if !j.exclude.contains(&dev.to_string()) {
                        j.exclude.push(dev.to_string());
                    }
                    // A copy arriving late over there would be found and ended.
                    j.orphans.push(Orphan {
                        device: dev.to_string(),
                        worker_job: wid.to_string(),
                    });
                });
                self.event(Some(dev), Some(id), "job", &format!("not started: {why}"));
            }
            Sent::Refused(why) => {
                self.update_job(id, |j| {
                    j.state = if j.cancel_requested {
                        JobState::Cancelled
                    } else {
                        JobState::Failed
                    };
                    j.cancel_requested = false;
                    j.ended_at = Some(now);
                    j.reason = Some(why.clone());
                });
                self.notify(Some(dev), Some(id), &format!("cannot run it: {why}"));
                self.advance(now);
            }
            Sent::Unknown(why) => {
                self.update_job(id, |j| {
                    j.unreachable_since = Some(now);
                    j.reason = Some(format!("not sure it started on {dev} ({why}): checking"));
                });
                self.event(
                    Some(dev),
                    Some(id),
                    "job",
                    &format!("outcome unknown: {why}"),
                );
            }
        }
    }

    fn cancelled_now(&self, id: &str, why: &str) {
        self.update_job(id, |j| {
            j.state = JobState::Cancelled;
            j.cancel_requested = false;
            j.ended_at = Some(now_ms());
            j.reason = Some("cancelled".into());
        });
        self.event(None, Some(id), "job", &format!("cancelled ({why})"));
        self.advance(now_ms());
    }

    /// Cancel a job. One that runs on a machine stays *cancelling* (its lease too) until the
    /// machine confirms its run ended.
    pub fn cancel(&self, id: &str) -> Result<Job> {
        let job = self.job(id)?;
        if job.state.finished() {
            return Ok(job);
        }
        let on_machine = matches!(job.state, JobState::Dispatched | JobState::Running);
        if !on_machine {
            self.cancelled_now(id, "before it started");
            return self.job(id);
        }
        self.update_job(id, |j| {
            j.cancel_requested = true;
            j.reason = Some("cancelling".into());
        });
        self.event(job.device.as_deref(), Some(id), "job", "cancelling");
        // Being sent: the sender ends it when it arrives.
        if self.lock().dispatching.contains(id) {
            return self.job(id);
        }
        if let (Some(dev), Some(wid)) = (&job.device, &job.worker_job) {
            match self.cancel_run(dev, wid) {
                Ok(w) if finished(worker_state(&w)) => {
                    self.end(&self.job(id)?, None, Some(Failure::Job), "cancelled");
                }
                Ok(_) => {}
                Err(e) => {
                    self.update_job(id, |j| {
                        j.reason = Some(format!(
                            "cancelling: {dev} does not answer ({e:#}); it holds its machine until it does"
                        ));
                    });
                }
            }
        }
        self.job(id)
    }

    /// Follow the runs on machines (each on a thread of its own): their state, uncertain
    /// sends, cancellations, and earlier runs that may still be alive.
    pub(super) fn follow(self: &Arc<Self>) {
        let todo: Vec<Job> = {
            let mut inner = self.lock();
            let list: Vec<Job> = inner
                .jobs
                .values()
                .filter(|j| {
                    (matches!(j.state, JobState::Running | JobState::Dispatched)
                        && j.worker_job.is_some()
                        && !inner.dispatching.contains(&j.id))
                        || !j.orphans.is_empty()
                })
                .filter(|j| !inner.following.contains(&j.id))
                .cloned()
                .collect();
            for j in &list {
                inner.following.insert(j.id.clone());
            }
            list
        };
        for job in todo {
            let me = self.clone();
            let _ = std::thread::Builder::new()
                .name("fleet-follow".into())
                .spawn(move || {
                    if !job.orphans.is_empty() {
                        me.follow_orphans(&job);
                    }
                    if matches!(job.state, JobState::Running | JobState::Dispatched)
                        && job.worker_job.is_some()
                    {
                        me.follow_run(&job, now_ms());
                    }
                    me.lock().following.remove(&job.id);
                });
        }
    }

    fn follow_run(&self, job: &Job, now: u64) {
        let dev = job.device.clone().unwrap_or_default();
        let wid = job.worker_job.clone().unwrap_or_default();
        let uncertain = job.state == JobState::Dispatched;
        let Ok((d, _)) = self.device_of(&dev) else {
            self.end(
                job,
                None,
                Some(Failure::Device),
                "its machine left the registry",
            );
            return;
        };
        match self.ask_run(&dev, &wid) {
            Ok(Some(w)) => {
                let state = worker_state(&w).to_string();
                if job.unreachable_since.is_some() || uncertain {
                    self.update_job(&job.id, |j| {
                        j.unreachable_since = None;
                        if uncertain && state != "staged" {
                            j.state = JobState::Running;
                            j.attempt_started = true;
                            j.started_at.get_or_insert(now);
                        }
                        if !j.cancel_requested {
                            j.reason = None;
                        }
                    });
                }
                if state == "staged" {
                    // Sent, inputs perhaps half there, never started: ended there, then placed
                    // again once the machine confirms it.
                    if let Ok(c) = self.cancel_run(&dev, &wid) {
                        if worker_state(&c) == "cancelled" {
                            self.settle(
                                &job.id,
                                &dev,
                                &wid,
                                Sent::NotStarted("its inputs did not all arrive".into()),
                            );
                        }
                    }
                    return;
                }
                if !finished(&state) {
                    if job.cancel_requested {
                        let _ = self.cancel_run(&dev, &wid);
                    }
                    return;
                }
                let job = self.job(&job.id).unwrap_or_else(|_| job.clone());
                let key = self
                    .device_of(&dev)
                    .ok()
                    .and_then(|(_, k)| k)
                    .unwrap_or_default();
                self.collect(&job, &d.kimo, &key, &wid, &w);
                let code = w["exit_code"].as_i64().map(|c| c as i32);
                match state.as_str() {
                    "succeeded" => self.end(&job, code, None, ""),
                    "cancelled" => {
                        self.end(&job, code, Some(Failure::Job), "cancelled on its machine")
                    }
                    "lost" => self.end(&job, code, Some(Failure::Device), "lost on its machine"),
                    "timed_out" => self.end(&job, code, Some(Failure::Job), "timed out"),
                    _ => self.end(
                        &job,
                        code,
                        Some(Failure::Job),
                        &code.map_or("failed".into(), |c| format!("exit code {c}")),
                    ),
                }
            }
            Ok(None) if uncertain => {
                self.settle(
                    &job.id,
                    &dev,
                    &wid,
                    Sent::NotStarted(format!("{dev} does not have it")),
                );
            }
            Ok(None) => self.end(
                job,
                None,
                Some(Failure::Device),
                "its machine does not know it",
            ),
            Err(_) => {
                let since = job.unreachable_since.unwrap_or(now);
                if job.unreachable_since.is_none() {
                    self.update_job(&job.id, |j| j.unreachable_since = Some(now));
                }
                let lost_after = LOST_AFTER_MS.max(d.power.boot_deadline_s * 1000);
                if now.saturating_sub(since) <= lost_after {
                    return;
                }
                // Gone too long: the job moves on, and this run is remembered, so it is ended
                // (and its GPU held) if the machine comes back with it.
                self.update_job(&job.id, |j| {
                    j.orphans.push(Orphan {
                        device: dev.clone(),
                        worker_job: wid.clone(),
                    })
                });
                let job = self.job(&job.id).unwrap_or_else(|_| job.clone());
                self.end(
                    &job,
                    None,
                    Some(Failure::Device),
                    "its machine stopped answering",
                );
            }
        }
    }

    /// Earlier runs: ended when their machine answers, forgotten once it says they ended or
    /// it never had them.
    fn follow_orphans(&self, job: &Job) {
        for o in &job.orphans {
            let gone = match self.ask_run(&o.device, &o.worker_job) {
                Ok(None) => true,
                Ok(Some(w)) if finished(worker_state(&w)) => {
                    self.event(
                        Some(&o.device),
                        Some(&job.id),
                        "job",
                        &format!(
                            "an earlier run ({}) ended: {}",
                            o.worker_job,
                            worker_state(&w)
                        ),
                    );
                    true
                }
                Ok(Some(_)) => {
                    let _ = self.cancel_run(&o.device, &o.worker_job);
                    false
                }
                Err(_) => false,
            };
            if gone {
                self.update_job(&job.id, |j| {
                    j.orphans.retain(|x| x != o);
                });
            }
        }
    }
}
