use std::sync::Arc;

use super::stage::StageControl;
use super::state::{AtomicLifecycleState, LifecycleState};
use tokio::sync::mpsc;

pub struct StageRef {
    pub id: usize,
    pub name: String,
    pub state: Arc<AtomicLifecycleState>,
}

impl StageRef {
    pub fn state(&self) -> LifecycleState {
        self.state.load()
    }
}
