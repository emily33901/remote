use std::fmt::Debug;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc;

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

pub enum StageControl<Input> {
    Input(Input),
    Start,
    Stop,
    Pause,
    Resume,
    Flush,
}

pub enum StageEvent<Output> {
    Output(Output),
    Started,
    Stopped,
    Paused,
    Resumed,
    Flushed,
    Error(String),
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
            StageEvent::Error(e) => StageEvent::Error(e),
        }
    }
}

#[async_trait]
pub trait Stage: Send + Sync {
    type Input: Send + Sync + 'static;
    type Output: Send + Sync + 'static;

    fn meta(&self) -> &StageMeta;
    
    fn state(&self) -> LifecycleState {
        self.atomic_state().load()
    }
    
    fn atomic_state(&self) -> &AtomicLifecycleState;

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

    async fn start(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        state.transition(LifecycleState::Starting)?;
        
        if let Err(e) = self.on_start().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: self.meta().name.clone(),
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Running)?;
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        state.transition(LifecycleState::Stopping)?;
        
        if let Err(e) = self.on_stop().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: self.meta().name.clone(),
                message: e.to_string(),
            });
        }
        
        state.store(LifecycleState::Stopped);
        Ok(())
    }

    async fn pause(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        state.transition(LifecycleState::Pausing)?;
        
        if let Err(e) = self.on_pause().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: self.meta().name.clone(),
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Paused)?;
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), LifecycleError> {
        let state = self.atomic_state();
        state.transition(LifecycleState::Resuming)?;
        
        if let Err(e) = self.on_resume().await {
            state.store(LifecycleState::Failed);
            return Err(LifecycleError::StageFailed {
                stage: self.meta().name.clone(),
                message: e.to_string(),
            });
        }
        
        state.transition(LifecycleState::Running)?;
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), LifecycleError> {
        if let Err(e) = self.on_flush().await {
            return Err(LifecycleError::StageFailed {
                stage: self.meta().name.clone(),
                message: e.to_string(),
            });
        }
        Ok(())
    }
}

pub struct RunningStage<S: Stage> {
    stage: std::sync::Arc<tokio::sync::Mutex<S>>,
    control_tx: mpsc::Sender<StageControl<S::Input>>,
    event_rx: mpsc::Receiver<StageEvent<S::Output>>,
    state: std::sync::Arc<AtomicLifecycleState>,
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
}

pub async fn run_stage<S: Stage + 'static>(
    mut stage: S,
    buffer_size: usize,
) -> Result<RunningStage<S>> {
    stage.initialize().await?;
    
    let (control_tx, mut control_rx) = mpsc::channel::<StageControl<S::Input>>(buffer_size);
    let (event_tx, event_rx) = mpsc::channel::<StageEvent<S::Output>>(buffer_size);
    
    let stage = std::sync::Arc::new(tokio::sync::Mutex::new(stage));
    let state = std::sync::Arc::new(AtomicLifecycleState::new(LifecycleState::Created));
    
    let stage_clone = stage.clone();
    let state_clone = state.clone();
    let event_tx_clone = event_tx.clone();
    
    tokio::spawn(async move {
        loop {
            let control = match control_rx.recv().await {
                Some(c) => c,
                None => break,
            };
            
            let mut stage_guard = stage_clone.lock().await;
            
            match control {
                StageControl::Start => {
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
                StageControl::Input(input) => {
                    if stage_guard.state() == LifecycleState::Running {
                        match stage_guard.process(input).await {
                            Ok(Some(output)) => {
                                let _ = event_tx_clone.send(StageEvent::Output(output)).await;
                            }
                            Ok(None) => {}
                            Err(e) => {
                                let _ = event_tx_clone.send(StageEvent::Error(e.to_string())).await;
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
