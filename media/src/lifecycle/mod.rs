mod state;
mod stage;
mod pipeline;
mod handle;
pub mod stages;

pub use state::{LifecycleState, LifecycleError};
pub use stage::{Stage, StageControl, StageEvent, StageMeta, run_stage, RunningStage};
pub use pipeline::{Pipeline, PipelineBuilder, StageHandle};
pub use handle::StageRef;
