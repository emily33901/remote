mod state;
mod stage;
mod pipeline;
pub mod stages;

pub use state::{LifecycleState, LifecycleError, AtomicLifecycleState};
pub use stage::{
    Stage, StageControl, StageEvent, StageMeta, StageError, 
    RetryPolicy, ProcessResult, run_stage, RunningStage,
};
pub use pipeline::{Pipeline, PipelineBuilder, StageHandle, StageRef};

pub fn connect_stages<S1, S2, F>(
    mut source: RunningStage<S1>,
    dest: RunningStage<S2>,
    mut transform: F,
) -> tokio::task::JoinHandle<()>
where
    S1: Stage + 'static,
    S2: Stage + 'static,
    F: FnMut(<S1 as Stage>::Output) -> <S2 as Stage>::Input + Send + 'static,
{
    tokio::spawn(async move {
        while let Some(event) = source.next_event().await {
            match event {
                StageEvent::Output(output) => {
                    let input = transform(output);
                    if dest.send_input(input).await.is_err() {
                        break;
                    }
                }
                StageEvent::FatalError(e) => {
                    tracing::error!("Source stage fatal error: {}", e);
                    break;
                }
                StageEvent::Stopped => {
                    break;
                }
                _ => {}
            }
        }
    })
}

pub fn connect_stages_passthrough<S1, S2>(
    source: RunningStage<S1>,
    dest: RunningStage<S2>,
) -> tokio::task::JoinHandle<()>
where
    S1: Stage<Output = <S2 as Stage>::Input> + 'static,
    S2: Stage + 'static,
{
    connect_stages(source, dest, |x| x)
}
