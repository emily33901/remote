use std::sync::atomic::{AtomicU8, Ordering};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum LifecycleState {
    Created = 0,
    Starting = 1,
    Running = 2,
    Pausing = 3,
    Paused = 4,
    Resuming = 5,
    Stopping = 6,
    Stopped = 7,
    Failed = 8,
}

impl LifecycleState {
    pub fn can_start(&self) -> bool {
        matches!(self, Self::Created | Self::Stopped)
    }

    pub fn can_pause(&self) -> bool {
        matches!(self, Self::Running)
    }

    pub fn can_resume(&self) -> bool {
        matches!(self, Self::Paused)
    }

    pub fn can_stop(&self) -> bool {
        matches!(
            self,
            Self::Running | Self::Paused | Self::Starting | Self::Resuming
        )
    }

    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Failed)
    }

    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running | Self::Starting | Self::Resuming)
    }
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Pausing => write!(f, "pausing"),
            Self::Paused => write!(f, "paused"),
            Self::Resuming => write!(f, "resuming"),
            Self::Stopping => write!(f, "stopping"),
            Self::Stopped => write!(f, "stopped"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

#[derive(Debug, Clone, Error)]
pub enum LifecycleError {
    #[error("Invalid state transition from {from} to {to}")]
    InvalidTransition {
        from: LifecycleState,
        to: LifecycleState,
    },

    #[error("Stage '{stage}' failed: {message}")]
    StageFailed { stage: String, message: String },

    #[error("Stage '{stage}' not found")]
    StageNotFound { stage: String },

    #[error("Stage '{stage}' timeout during {operation}")]
    Timeout { stage: String, operation: String },

    #[error("Pipeline is in terminal state")]
    TerminalState,
}

pub struct AtomicLifecycleState {
    state: AtomicU8,
}

impl AtomicLifecycleState {
    pub fn new(state: LifecycleState) -> Self {
        Self {
            state: AtomicU8::new(state as u8),
        }
    }

    pub fn load(&self) -> LifecycleState {
        match self.state.load(Ordering::SeqCst) {
            0 => LifecycleState::Created,
            1 => LifecycleState::Starting,
            2 => LifecycleState::Running,
            3 => LifecycleState::Pausing,
            4 => LifecycleState::Paused,
            5 => LifecycleState::Resuming,
            6 => LifecycleState::Stopping,
            7 => LifecycleState::Stopped,
            8 => LifecycleState::Failed,
            _ => LifecycleState::Failed,
        }
    }

    pub fn store(&self, state: LifecycleState) {
        self.state.store(state as u8, Ordering::SeqCst);
    }

    pub fn compare_exchange(
        &self,
        current: LifecycleState,
        new: LifecycleState,
    ) -> Result<LifecycleState, LifecycleState> {
        self.state
            .compare_exchange(current as u8, new as u8, Ordering::SeqCst, Ordering::SeqCst)
            .map(|v| match v {
                0 => LifecycleState::Created,
                1 => LifecycleState::Starting,
                2 => LifecycleState::Running,
                3 => LifecycleState::Pausing,
                4 => LifecycleState::Paused,
                5 => LifecycleState::Resuming,
                6 => LifecycleState::Stopping,
                7 => LifecycleState::Stopped,
                8 => LifecycleState::Failed,
                _ => LifecycleState::Failed,
            })
            .map_err(|v| match v {
                0 => LifecycleState::Created,
                1 => LifecycleState::Starting,
                2 => LifecycleState::Running,
                3 => LifecycleState::Pausing,
                4 => LifecycleState::Paused,
                5 => LifecycleState::Resuming,
                6 => LifecycleState::Stopping,
                7 => LifecycleState::Stopped,
                8 => LifecycleState::Failed,
                _ => LifecycleState::Failed,
            })
    }

    pub fn transition(&self, to: LifecycleState) -> Result<LifecycleState, LifecycleError> {
        use LifecycleState::*;

        let current = self.load();

        let valid = match (&current, &to) {
            (Created, Starting) => true,
            (Starting, Running) | (Starting, Failed) | (Starting, Stopping) => true,
            (Running, Pausing) | (Running, Stopping) => true,
            (Pausing, Paused) | (Pausing, Stopping) => true,
            (Paused, Resuming) | (Paused, Stopping) => true,
            (Resuming, Running) | (Resuming, Stopping) => true,
            (Stopping, Stopped) | (Stopping, Failed) => true,
            (Stopped, Starting) => true,
            _ => false,
        };

        if !valid {
            return Err(LifecycleError::InvalidTransition { from: current, to });
        }

        match self.compare_exchange(current, to) {
            Ok(_) => Ok(to),
            Err(actual) => Err(LifecycleError::InvalidTransition { from: actual, to }),
        }
    }
}

impl Default for AtomicLifecycleState {
    fn default() -> Self {
        Self::new(LifecycleState::Created)
    }
}
