//! Each machine's power state, moved on by what the coordinator sees (nothing, a ping, its
//! kimo answering) and by what it holds (leases). Pure: the actions it asks for (send a
//! magic packet, switch the machine off, tell the owner) are carried out by the caller.
//!
//! `off` → `waking` → `ready`/`idle`/`busy` → `draining` → `off`; `booted` when the machine
//! answers pings but its kimo is not ready yet, `unreachable` when it should be up and isn't.
//! A machine in `manual` (the owner took control) is never woken or switched off by itself.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Power {
    /// Not looked at yet.
    #[default]
    Unknown,
    Off,
    /// A magic packet went out; waiting for it to boot and answer.
    Waking,
    /// It answers pings, its kimo does not (yet).
    Booted,
    /// Up, ready for work, not switched off when idle.
    Ready,
    /// Up and holding leases (jobs).
    Busy,
    /// Up without leases: switched off once idle long enough.
    Idle,
    /// Asked to switch off; waiting for it to stop answering.
    Draining,
    /// Should be up and isn't.
    Unreachable,
}

impl Power {
    pub fn up(self) -> bool {
        matches!(self, Power::Ready | Power::Busy | Power::Idle)
    }

    pub fn name(self) -> &'static str {
        match self {
            Power::Unknown => "unknown",
            Power::Off => "off",
            Power::Waking => "waking",
            Power::Booted => "booted",
            Power::Ready => "ready",
            Power::Busy => "busy",
            Power::Idle => "idle",
            Power::Draining => "draining",
            Power::Unreachable => "unreachable",
        }
    }
}

/// A machine's state as the coordinator keeps it (saved, so a restart keeps your overrides).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceState {
    pub power: Power,
    /// Unix time (ms) it entered `power`.
    pub since: u64,
    /// You took control: nothing automatic (no wake, no shutdown, no jobs).
    #[serde(default)]
    pub manual: bool,
    /// Kept on until then (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_on_until: Option<u64>,
    /// Never switched off for being idle.
    #[serde(default)]
    pub never_auto_shutdown: bool,
    #[serde(default)]
    pub wake_attempts: u32,
    /// Until when a wake or a shutdown may take (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<u64>,
    /// Idle (no leases, nothing working) since (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_since: Option<u64>,
    /// Last time its kimo answered (ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_seen: Option<u64>,
    /// Automatic boots on one day: (day number, count).
    #[serde(default)]
    pub boots: (u64, u32),
}

/// What the registry allows for a machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Policy {
    pub can_wake: bool,
    pub auto_shutdown: bool,
    pub idle_ms: u64,
    pub boot_ms: u64,
    pub boots_per_day: u32,
}

/// What a probe saw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Nothing,
    /// It answers pings only.
    Ping,
    /// Its kimo answered; `ready` when its facts meet the registry (the GPU is visible).
    Kimo {
        ready: bool,
    },
}

/// One look at a machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Look {
    pub now: u64,
    pub seen: Seen,
    /// Jobs placed on it and not ended.
    pub leases: usize,
    /// Its kimo has agents working, terminals open or jobs of its own running.
    pub busy_there: bool,
    /// Queued work could go to it within the next half hour.
    pub work_soon: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Send the magic packet.
    Wake,
    /// Ask its kimo to switch off (it checks again); call [`DeviceState::draining`] on success.
    Shutdown,
    /// Tell the owner (ntfy) and record it.
    Notify(String),
    /// Record it.
    Event(String),
}

/// Two magic packets at most for one wake, then `unreachable`.
pub const WAKE_ATTEMPTS: u32 = 2;
/// How long a shutdown may take before it counts as failed.
pub const DRAIN_MS: u64 = 120_000;

const DAY_MS: u64 = 86_400_000;

impl DeviceState {
    fn set(&mut self, power: Power, now: u64) {
        if self.power != power {
            self.power = power;
            self.since = now;
        }
    }

    /// Kept on by the owner (manual, never, or for some hours).
    pub fn held(&self, now: u64) -> bool {
        self.manual || self.never_auto_shutdown || self.keep_on_until.is_some_and(|t| t > now)
    }

    /// The state shown: `manual` over the power state while you have control.
    pub fn shown(&self) -> &'static str {
        if self.manual {
            "manual"
        } else {
            self.power.name()
        }
    }

    /// May the scheduler place work here now?
    pub fn takes_work(&self) -> bool {
        !self.manual && self.power.up()
    }

    /// May the scheduler wake it for work?
    pub fn wakeable(&self, policy: &Policy, now: u64) -> bool {
        !self.manual
            && policy.can_wake
            && matches!(self.power, Power::Off | Power::Unknown)
            && self.boots_today(now) < policy.boots_per_day
    }

    pub fn boots_today(&self, now: u64) -> u32 {
        if self.boots.0 == now / DAY_MS {
            self.boots.1
        } else {
            0
        }
    }

    /// Start waking it (asked by you, or by the scheduler for work).
    pub fn wake(&mut self, policy: &Policy, now: u64) -> Result<Vec<Action>, String> {
        if !policy.can_wake {
            return Err("this machine cannot be woken from here (no wake: wol)".into());
        }
        match self.power {
            p if p.up() || p == Power::Booted => return Ok(vec![]),
            Power::Waking => return Ok(vec![]),
            Power::Draining => return Err("it is switching off: wake it once it is off".into()),
            _ => {}
        }
        let day = now / DAY_MS;
        self.boots = (day, self.boots_today(now) + 1);
        self.set(Power::Waking, now);
        self.wake_attempts = 1;
        self.deadline = Some(now + policy.boot_ms);
        Ok(vec![Action::Wake, Action::Event("waking".into())])
    }

    /// Its kimo accepted to switch off: wait for it to stop answering.
    pub fn draining(&mut self, now: u64) {
        self.set(Power::Draining, now);
        self.deadline = Some(now + DRAIN_MS);
        self.idle_since = None;
    }

    /// Its kimo refused to switch off (something runs there): idle again from now.
    pub fn refused(&mut self, now: u64) {
        self.idle_since = Some(now);
    }

    fn come_up(&mut self, policy: &Policy, look: &Look) -> Vec<Action> {
        let now = look.now;
        self.last_seen = Some(now);
        self.deadline = None;
        self.wake_attempts = 0;
        let held = self.held(now);
        let power = if look.leases > 0 {
            Power::Busy
        } else if policy.auto_shutdown && !held {
            Power::Idle
        } else {
            Power::Ready
        };
        self.set(power, now);
        if power != Power::Idle {
            self.idle_since = None;
            return vec![];
        }
        // Work over there, or work on its way, starts the idle time again.
        if look.busy_there || look.work_soon || self.idle_since.is_none() {
            self.idle_since = Some(now);
        }
        let idle_for = now.saturating_sub(self.idle_since.unwrap_or(now));
        if !self.manual && idle_for >= policy.idle_ms {
            return vec![Action::Shutdown];
        }
        vec![]
    }

    /// Move on after a look at the machine.
    pub fn step(&mut self, policy: &Policy, look: &Look) -> Vec<Action> {
        let now = look.now;
        let ready = matches!(look.seen, Seen::Kimo { ready: true });
        let on = look.seen != Seen::Nothing;
        match self.power {
            Power::Draining => {
                if !on {
                    self.set(Power::Off, now);
                    self.deadline = None;
                    return vec![Action::Event("switched off".into())];
                }
                if self.deadline.is_some_and(|d| now > d) {
                    let mut a = self.come_up(policy, look);
                    a.retain(|a| *a != Action::Shutdown);
                    self.idle_since = Some(now);
                    a.push(Action::Notify("did not switch off: still answering".into()));
                    return a;
                }
                vec![]
            }
            Power::Waking => {
                if ready {
                    let took = now.saturating_sub(self.since) / 1000;
                    let mut a = vec![Action::Event(format!("awake after {took} s"))];
                    a.extend(self.come_up(policy, look));
                    return a;
                }
                if self.deadline.is_none_or(|d| now <= d) {
                    return vec![];
                }
                if self.wake_attempts < WAKE_ATTEMPTS {
                    self.wake_attempts += 1;
                    self.deadline = Some(now + policy.boot_ms);
                    return vec![
                        Action::Wake,
                        Action::Event("no answer yet: magic packet sent again".into()),
                    ];
                }
                self.set(Power::Unreachable, now);
                self.deadline = None;
                vec![Action::Notify(if on {
                    "is on but its kimo does not answer".into()
                } else {
                    "did not wake up".into()
                })]
            }
            _ if ready => {
                let was = self.power;
                let mut a = self.come_up(policy, look);
                if !was.up() && was != Power::Unknown {
                    a.insert(0, Action::Event("answering again".into()));
                }
                a
            }
            _ if on => {
                if self.power != Power::Booted {
                    self.set(Power::Booted, now);
                }
                vec![]
            }
            p if p.up() || p == Power::Booted => {
                self.idle_since = None;
                if look.leases > 0 {
                    self.set(Power::Unreachable, now);
                    vec![Action::Notify(format!(
                        "stopped answering with {} job(s) on it",
                        look.leases
                    ))]
                } else {
                    self.set(Power::Off, now);
                    vec![Action::Event("stopped answering".into())]
                }
            }
            Power::Unknown => {
                self.set(Power::Off, now);
                vec![]
            }
            _ => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUC: Policy = Policy {
        can_wake: true,
        auto_shutdown: true,
        idle_ms: 20 * 60_000,
        boot_ms: 180_000,
        boots_per_day: 10,
    };

    fn look(now: u64, seen: Seen) -> Look {
        Look {
            now,
            seen,
            leases: 0,
            busy_there: false,
            work_soon: false,
        }
    }

    const UP: Seen = Seen::Kimo { ready: true };

    #[test]
    fn wake_boot_and_answer() {
        let mut s = DeviceState::default();
        assert!(s.step(&NUC, &look(0, Seen::Nothing)).is_empty());
        assert_eq!(s.power, Power::Off);
        assert!(s.wakeable(&NUC, 0));
        assert_eq!(s.wake(&NUC, 1000).unwrap()[0], Action::Wake);
        assert_eq!(s.power, Power::Waking);
        // Pings first, then kimo without the GPU, then ready.
        assert!(s.step(&NUC, &look(30_000, Seen::Ping)).is_empty());
        assert!(s
            .step(&NUC, &look(40_000, Seen::Kimo { ready: false }))
            .is_empty());
        assert_eq!(s.power, Power::Waking);
        let a = s.step(&NUC, &look(61_000, UP));
        assert_eq!(a[0], Action::Event("awake after 60 s".into()));
        assert_eq!(s.power, Power::Idle);
        assert_eq!(s.boots_today(61_000), 1);
    }

    #[test]
    fn a_wake_that_fails_is_tried_again_once_then_unreachable() {
        let mut s = DeviceState {
            power: Power::Off,
            ..Default::default()
        };
        s.wake(&NUC, 0).unwrap();
        assert!(s.step(&NUC, &look(100_000, Seen::Nothing)).is_empty());
        let a = s.step(&NUC, &look(181_000, Seen::Nothing));
        assert_eq!(a[0], Action::Wake);
        assert_eq!(s.power, Power::Waking);
        let a = s.step(&NUC, &look(362_000, Seen::Nothing));
        assert_eq!(a, vec![Action::Notify("did not wake up".into())]);
        assert_eq!(s.power, Power::Unreachable);
        // It shows up later by itself: fine.
        s.step(&NUC, &look(500_000, UP));
        assert_eq!(s.power, Power::Idle);
    }

    #[test]
    fn idle_long_enough_then_switched_off() {
        let mut s = DeviceState::default();
        s.step(&NUC, &look(0, UP));
        assert_eq!(s.power, Power::Idle);
        assert!(s.step(&NUC, &look(10 * 60_000, UP)).is_empty());
        // An agent working over there starts the idle time again.
        let busy = Look {
            busy_there: true,
            ..look(15 * 60_000, UP)
        };
        assert!(s.step(&NUC, &busy).is_empty());
        assert!(s.step(&NUC, &look(30 * 60_000, UP)).is_empty());
        assert_eq!(s.step(&NUC, &look(36 * 60_000, UP)), vec![Action::Shutdown]);
        s.draining(36 * 60_000);
        assert_eq!(s.power, Power::Draining);
        // Still answering while it goes down; then gone.
        assert!(s.step(&NUC, &look(36 * 60_000 + 5000, UP)).is_empty());
        let a = s.step(&NUC, &look(37 * 60_000, Seen::Nothing));
        assert_eq!(a, vec![Action::Event("switched off".into())]);
        assert_eq!(s.power, Power::Off);
    }

    #[test]
    fn a_shutdown_that_does_not_happen_is_reported() {
        let mut s = DeviceState::default();
        s.step(&NUC, &look(0, UP));
        s.draining(1000);
        let a = s.step(&NUC, &look(1000 + DRAIN_MS + 1, UP));
        assert!(a.contains(&Action::Notify(
            "did not switch off: still answering".into()
        )));
        assert_eq!(s.power, Power::Idle);
    }

    #[test]
    fn leases_held_and_manual_keep_it_on() {
        let mut s = DeviceState::default();
        let leased = Look {
            leases: 1,
            ..look(0, UP)
        };
        s.step(&NUC, &leased);
        assert_eq!(s.power, Power::Busy);
        assert!(s.takes_work());
        // Kept on for two hours: ready, not idle.
        s.keep_on_until = Some(2 * 3_600_000);
        s.step(&NUC, &look(60_000, UP));
        assert_eq!(s.power, Power::Ready);
        assert!(s.step(&NUC, &look(3_600_000, UP)).is_empty());
        // Afterwards the idle time starts.
        s.step(&NUC, &look(2 * 3_600_000 + 1, UP));
        assert_eq!(s.power, Power::Idle);
        s.manual = true;
        assert_eq!(s.shown(), "manual");
        assert!(!s.takes_work());
        assert!(s.step(&NUC, &look(9 * 3_600_000, UP)).is_empty());
        assert_eq!(s.power, Power::Ready);
    }

    #[test]
    fn stopping_to_answer_with_jobs_is_news() {
        let mut s = DeviceState::default();
        s.step(&NUC, &look(0, UP));
        let gone = Look {
            leases: 2,
            ..look(30_000, Seen::Nothing)
        };
        let a = s.step(&NUC, &gone);
        assert_eq!(
            a,
            vec![Action::Notify(
                "stopped answering with 2 job(s) on it".into()
            )]
        );
        assert_eq!(s.power, Power::Unreachable);
        assert!(!s.wakeable(&NUC, 30_000));
        // Without jobs it simply went off (a laptop closed, a machine switched off by hand).
        let mut laptop = DeviceState::default();
        let mac = Policy {
            can_wake: false,
            auto_shutdown: false,
            ..NUC
        };
        laptop.step(&mac, &look(0, UP));
        assert_eq!(laptop.power, Power::Ready);
        laptop.step(&mac, &look(30_000, Seen::Nothing));
        assert_eq!(laptop.power, Power::Off);
        assert!(!laptop.wakeable(&mac, 30_000));
        assert!(laptop.wake(&mac, 30_000).is_err());
    }

    #[test]
    fn automatic_boots_are_limited_per_day() {
        let mut s = DeviceState {
            power: Power::Off,
            ..Default::default()
        };
        let p = Policy {
            boots_per_day: 2,
            ..NUC
        };
        for i in 0..2 {
            assert!(s.wakeable(&p, i * 1000));
            s.wake(&p, i * 1000).unwrap();
            s.power = Power::Off;
        }
        assert!(!s.wakeable(&p, 5000));
        // A new day.
        assert!(s.wakeable(&p, DAY_MS + 1));
    }
}
