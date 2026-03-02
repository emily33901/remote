use std::fmt::Debug;
use std::time::Duration;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;
use rand::Rng;

use super::state::{LifecycleState, AtomicLifecycleState, LifecycleError};

#[derive(Debug, Clone)]
pub struct StageMeta {
    pub name: String,
    pub version: u32,
}

impl StageMeta {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: 1,
        }
    }
}

#[derive(Debug, Clone)]
pub enum StageError {
    Transient { message: String, retry_after: Option<Duration> },
    Fatal { message: String },
}

impl StageError {
    pub fn transient(message: impl Into<String>) -> Self {
        Self::Transient {
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn transient_with_backoff(message: impl Into<String>, retry_after: Duration) -> Self {
        Self::Transient {
            message: message.into(),
            retry_after: Some(retry_after),
        }
    }

    pub fn fatal(message: impl Into<String>) -> Self {
        Self::Fatal { message: message.into() }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient { .. })
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Transient { message, .. } => message,
            Self::Fatal { message } => message,
        }
    }
}

impl std::fmt::Display for StageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transient { message, retry_after } => {
                if let Some(d) = retry_after {
                    write!(f, "Transient error: {} (retry after {:?})", message, d)
                } else {
                    write!(f, "Transient error: {}", message)
                }
            }
            Self::Fatal { message } => write!(f, "Fatal error: {}", message),
        }
    }
}

impl std::error::Error for StageError {}

impl From<anyhow::Error> for StageError {
    fn from(err: anyhow::Error) -> Self {
        StageError::transient(err.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub backoff_multiplier: f32,
    pub jitter: bool,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(30),
            backoff_multiplier: 2.0,
            jitter: true,
        }
    }
}

impl RetryPolicy {
    pub fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            ..Default::default()
        }
    }

    pub fn no_retries() -> Self {
        Self {
            max_retries: 0,
            ..Default::default()
        }
    }

    pub fn with_initial_backoff(mut self, duration: Duration) -> Self {
        self.initial_backoff = duration;
        self
    }

    pub fn with_max_backoff(mut self, duration: Duration) -> Self {
        self.max_backoff = duration;
        self
    }

    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }

    pub fn calculate_backoff(&self, attempt: u32) -> Duration {
        let multiplier = self.backoff_multiplier.powi(attempt as i32);
        let mut backoff = self.initial_backoff.mul_f32(multiplier);
        
        if backoff > self.max_backoff {
            backoff = self.max_backoff;
        }

        if self.jitter {
            let jitter_range = backoff.as_millis() as f32 * 0.3;
            let jitter = rand::thread_rng().gen_range(0.0..jitter_range) as u64;
            backoff = Duration::from_millis(backoff.as_millis() as u64 + jitter);
        }

        backoff
    }

    pub fn can_retry(&self, attempt: u32) -> bool {
        attempt < self.max_retries
    }
}

pub enum StageControl<Input> {
    Input(Input),
    Start,
    Stop,
    Pause,
    Resume,
    Flush,
    Reset,
}

#[derive(Debug, Clone)]
pub enum StageEvent<Output> {
    Output(Output),
    Started,
    Stopped,
    Paused,
    Resumed,
    Flushed,
    Reset,
    Retrying { attempt: u32, backoff: Duration },
    Error(String),
    FatalError(String),
}

impl<Output> StageEvent<Output> {
    pub fn map_output<T, F: FnOnce(Output) -> T>(self, f: F) -> StageEvent<T> {
        match self {
            StageEvent::Output(o) => StageEvent::Output(f(o)),
            StageEvent::Started => StageEvent::Started,
            StageEvent::Stopped => StageEvent::Stopped,
            StageEvent::Paused => StageEvent::Paused,
            StageEvent::Resumed => StageEvent::Resumed,
            StageEvent::Flushed => StageEvent::Flushed,
            StageEvent::Reset => StageEvent::Reset,
            StageEvent::Retrying { attempt, backoff } => StageEvent::Retrying { attempt, backoff },
            StageEvent::Error(e) => StageEvent::Error(e),
            StageEvent::FatalError(e) => StageEvent::FatalError(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessResult {
    Continue,
    Pause,
}

#[async_trait]
pub trait Stage: Send + Sync {
    type Input: Send + Sync + 'static;
    type Output: Send + Sync + 'static;

    fn meta(&self) -> &StageMeta;
    
    fn state(&self) -> LifecycleState {
        self.atomic_state().load()
    }
    
    fn atomic_state(&self) -> Arc<AtomicLifecycleState>;

    async fn initialize(&mut self) -> Result<()>;
    
    async fn process(&mut self, input: Self::Input) -> Result<Option<Self::Output>>;
    
    async fn on_start(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn on_stop(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn on_pause(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn on_resume(&mut self) -> Result<()> {
        Ok(())
    }
    
    async fn on_flush(&mut self) -> Result<()> {
        Ok(())
    }

    async fn on_reset(&mut self) -> Result<()> {
        Ok(())
    }

    fn retry_policy(&self) -> RetryPolicy {
        RetryPolicy::default()
    }

    async fn start(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        let name = self.meta().name.clone();
        state.transition(LifecycleState::Starting)?;
        
        if let Err(e) = self.on_start().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Running)?;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        let name = self.meta().name.clone();
        state.transition(LifecycleState::Stopping)?;
        
        if let Err(e) = self.on_stop().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }
        
        state.store(LifecycleState::Stopped);
        Ok(())
    }

    async fn pause(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        let name = self.meta().name.clone();
        state.transition(LifecycleState::Pausing)?;
        
        if let Err(e) = self.on_pause().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Paused)?;
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        let name = self.meta().name.clone();
        state.transition(LifecycleState::Resuming)?;
        
        if let Err(e) = self.on_resume().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Running)?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), LifecycleError> {
        let name = self.meta().name.clone();
        if let Err(e) = self.on_flush().await {
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }
        Ok(())
    }

    async fn reset(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        let name = self.meta().name.clone();
        
        if !state.load().can_reset() {
            return Err(LifecycleError::InvalidTransition {
                from: state.load(),
                to: LifecycleState::Created,
            });
        }

        if let Err(e) = self.on_reset().await {
            return Err(LifecycleError::StageFailed {
                stage: name,
                message: e.to_string(),
            });
        }

        state.store(LifecycleState::Created);
        Ok(())
    }
}

pub struct RunningStage<S: Stage> {
    stage: Arc<tokio::sync::Mutex<S>>,
    control_tx: mpsc::Sender<StageControl<S::Input>>,
    event_rx: mpsc::Receiver<StageEvent<S::Output>>,
    state: Arc<AtomicLifecycleState>,
}

impl<S: Stage + 'static> RunningStage<S> {
    pub fn control(&self) -> mpsc::Sender<StageControl<S::Input>> {
        self.control_tx.clone()
    }

    pub async fn next_event(&mut self) -> Option<StageEvent<S::Output>> {
        self.event_rx.recv().await
    }

    pub fn state(&self) -> LifecycleState {
        self.state.load()
    }

    pub async fn send_input(&self, input: S::Input) -> Result<()> {
        self.control_tx.send(StageControl::Input(input)).await?;
        Ok(())
    }

    pub async fn send_start(&self) -> Result<()> {
        self.control_tx.send(StageControl::Start).await?;
        Ok(())
    }

    pub async fn send_stop(&self) -> Result<()> {
        self.control_tx.send(StageControl::Stop).await?;
        Ok(())
    }

    pub async fn send_pause(&self) -> Result<()> {
        self.control_tx.send(StageControl::Pause).await?;
        Ok(())
    }

    pub async fn send_resume(&self) -> Result<()> {
        self.control_tx.send(StageControl::Resume).await?;
        Ok(())
    }

    pub async fn send_flush(&self) -> Result<()> {
        self.control_tx.send(StageControl::Flush).await?;
        Ok(())
    }

    pub async fn send_reset(&self) -> Result<()> {
        self.control_tx.send(StageControl::Reset).await?;
        Ok(())
    }
}

pub async fn run_stage<S: Stage + 'static>(
    mut stage: S,
    buffer_size: usize,
) -> Result<RunningStage<S>> {
    stage.initialize().await?;
    
    let (control_tx, mut control_rx) = mpsc::channel::<StageControl<S::Input>>(buffer_size);
    let (event_tx, event_rx) = mpsc::channel::<StageEvent<S::Output>>(buffer_size);
    
    let stage = Arc::new(tokio::sync::Mutex::new(stage));
    let state = Arc::new(AtomicLifecycleState::new(LifecycleState::Created));
    
    let stage_clone = stage.clone();
    let state_clone = state.clone();
    let event_tx_clone = event_tx.clone();
    
    tokio::spawn(async move {
        let mut retry_count: u32 = 0;
        
        loop {
            let control = match control_rx.recv().await {
                Some(c) => c,
                None => break,
            };
            
            let mut stage_guard = stage_clone.lock().await;
            let retry_policy = stage_guard.retry_policy();
            
            match control {
                StageControl::Start => {
                    retry_count = 0;
                    match stage_guard.start().await {
                        Ok(()) => {
                            let _ = event_tx_clone.send(StageEvent::Started).await;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Stop => {
                    match stage_guard.stop().await {
                        Ok(()) => {
                            let _ = event_tx_clone.send(StageEvent::Stopped).await;
                            break;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Pause => {
                    match stage_guard.pause().await {
                        Ok(()) => {
                            let _ = event_tx_clone.send(StageEvent::Paused).await;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Resume => {
                    match stage_guard.resume().await {
                        Ok(()) => {
                            let _ = event_tx_clone.send(StageEvent::Resumed).await;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Flush => {
                    match stage_guard.flush().await {
                        Ok(()) => {
                            let _ = event_tx_clone.send(StageEvent::Flushed).await;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Reset => {
                    match stage_guard.reset().await {
                        Ok(()) => {
                            retry_count = 0;
                            let _ = event_tx_clone.send(StageEvent::Reset).await;
                        }
                        Err(e) => {
                            let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
                        }
                    }
                }
                StageControl::Input(input) => {
                    let current_state = stage_guard.state();
                    if current_state == LifecycleState::Running {
                        match stage_guard.process(input).await {
                            Ok(Some(output)) => {
                                retry_count = 0;
                                let _ = event_tx_clone.send(StageEvent::Output(output)).await;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let backoff = retry_policy.calculate_backoff(retry_count);
                                
                                if retry_policy.can_retry(retry_count) {
                                    retry_count += 1;
                                    
                                    let _ = event_tx_clone.send(StageEvent::Retrying {
                                        attempt: retry_count,
                                        backoff,
                                    }).await;
                                    
                                    drop(stage_guard);
                                    tokio::time::sleep(backoff).await;
                                    stage_guard = stage_clone.lock().await;
                                    
                                    if stage_guard.state() == LifecycleState::Running {
                                        continue;
                                    }
                                } else {
                                    stage_guard.atomic_state().store(LifecycleState::Failed);
                                    let _ = event_tx_clone.send(StageEvent::FatalError(
                                        format!("{} (exhausted {} retries)", e, retry_count)
                                    )).await;
                                }
                            }
                        }
                    }
                }
            }
        }
    });
    
    Ok(RunningStage {
        stage,
        control_tx,
        event_rx,
        state,
    })
}
