//! Session state machine: which user actions are allowed in which status.

use super::models::SessionStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    /// Start a stopped/exited session (continuing its conversation when it ran before).
    Start,
    /// Freeze the process tree (SIGSTOP): no CPU, memory kept.
    Pause,
    /// Unfreeze a paused session (SIGCONT).
    Resume,
    /// End the processes to free memory; `Start` brings the conversation back.
    Stop,
    /// Stop then start again.
    Restart,
}

/// The status after `action`, or `None` when the action makes no sense in `status`.
pub fn next(status: SessionStatus, action: Action) -> Option<SessionStatus> {
    use Action::*;
    use SessionStatus::*;
    match (status, action) {
        (Stopped | Exited, Start) => Some(Running),
        (Running, Pause) => Some(Paused),
        (Paused, Resume) => Some(Running),
        (Running | Paused, Stop) => Some(Stopped),
        (_, Restart) => Some(Running),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::Action::*;
    use super::*;
    use SessionStatus::*;

    #[test]
    fn allowed_transitions() {
        assert_eq!(next(Stopped, Start), Some(Running));
        assert_eq!(next(Exited, Start), Some(Running));
        assert_eq!(next(Running, Pause), Some(Paused));
        assert_eq!(next(Paused, Resume), Some(Running));
        assert_eq!(next(Paused, Stop), Some(Stopped));
        assert_eq!(next(Exited, Restart), Some(Running));
    }

    #[test]
    fn refused_transitions() {
        assert_eq!(next(Running, Start), None);
        assert_eq!(next(Stopped, Pause), None);
        assert_eq!(next(Running, Resume), None);
        assert_eq!(next(Exited, Stop), None);
    }
}
