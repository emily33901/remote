mod state;
mod stage;
mod pipeline;
pub mod stages;

pub use state::{LifecycleState, LifecycleError, AtomicLifecycleState};
pub use stage::{
    Stage, StageControl, StageEvent, StageMeta, StageError, 
    RetryPolicy, ProcessResult, run_stage, RunningStage,
    StageControlHandle, StageEventStream,
};
pub use pipeline::{Pipeline, PipelineBuilder, StageHandle, StageRef};

pub fn connect<O1, O2, F>(
    mut source: StageEventStream<O1>,
    dest: StageControlHandle<O2>,
    mut transform: F,
) -> tokio::task::JoinHandle<()>
where
    O1: Send + Sync + 'static,
    O2: Send + Sync + 'static,
    F: FnMut(O1) -> O2 + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(StageEvent::Output(output)) = source.next().await {
            let input = transform(output);
            if dest.send_input(input).await.is_err() {
                break;
            }
        }
    })
}

pub fn connect_passthrough<T>(
    source: StageEventStream<T>,
    dest: StageControlHandle<T>,
) -> tokio::task::JoinHandle<()>
where
    T: Send + Sync + 'static,
{
    connect(source, dest, |x| x)
}
