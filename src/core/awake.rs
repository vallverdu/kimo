//! Keeping the computer awake: always, or while agents work (and a little after, so a pause
//! between two steps does not let it fall asleep).

use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepAwake {
    /// The system's normal sleep.
    #[default]
    Off,
    /// While an agent is working.
    Agents,
    /// Always.
    On,
}

/// How long after the last working agent the computer stays awake.
pub const GRACE: Duration = Duration::from_secs(120);

/// Should the computer stay awake now? `since_working`: time since an agent last worked
/// (`Some(ZERO)` while one works).
pub fn stay_awake(mode: KeepAwake, since_working: Option<Duration>) -> bool {
    match mode {
        KeepAwake::Off => false,
        KeepAwake::On => true,
        KeepAwake::Agents => since_working.is_some_and(|d| d < GRACE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_to_stay_awake() {
        assert!(!stay_awake(KeepAwake::Off, Some(Duration::ZERO)));
        assert!(stay_awake(KeepAwake::On, None));
        assert!(stay_awake(KeepAwake::Agents, Some(Duration::ZERO)));
        assert!(stay_awake(KeepAwake::Agents, Some(Duration::from_secs(60))));
        assert!(!stay_awake(
            KeepAwake::Agents,
            Some(Duration::from_secs(180))
        ));
        assert!(!stay_awake(KeepAwake::Agents, None));
        let mode: KeepAwake = serde_json::from_str("\"agents\"").unwrap();
        assert_eq!(mode, KeepAwake::Agents);
    }
}
