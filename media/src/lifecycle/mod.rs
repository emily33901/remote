mod pipeline;
mod stage;
pub mod stages;
mod state;

pub use pipeline::{Pipeline, PipelineBuilder, StageHandle, StageRef};
pub use stage::{
    run_stage, ProcessResult, RetryPolicy, RunningStage, Stage, StageControl, StageControlHandle,
    StageError, StageEvent, StageEventStream, StageMeta,
};
pub use state::{AtomicLifecycleState, LifecycleError, LifecycleState};

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
