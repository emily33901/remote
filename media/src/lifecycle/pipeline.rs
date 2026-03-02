use std::sync::Arc;
use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::mpsc;

use super::state::{LifecycleState, AtomicLifecycleState, LifecycleError};
use super::stage::{Stage, StageControl, StageEvent, StageMeta, run_stage};

pub type StageId = usize;

#[derive(Clone)]
pub struct StageRef {
    pub id: StageId,
    pub name: String,
}

pub struct StageHandle<Input, Output> {
    id: StageId,
    name: String,
    control: mpsc::Sender<StageControl<Input>>,
    state: Arc<AtomicLifecycleState>,
    _output: std::marker::PhantomData<Output>,
}

impl<Input: Send + Sync + 'static, Output: Send + Sync + 'static> StageHandle<Input, Output> {
    pub fn id(&self) -> StageId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self) -> LifecycleState {
        self.state.load()
    }

    pub async fn send(&self, input: Input) -> Result<()> {
        self.control.send(StageControl::Input(input)).await?;
        Ok(())
    }

    pub async fn start(&self) -> Result<()> {
        self.control.send(StageControl::Start).await?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<()> {
        self.control.send(StageControl::Stop).await?;
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        self.control.send(StageControl::Pause).await?;
        Ok(())
    }

    pub async fn resume(&self) -> Result<()> {
        self.control.send(StageControl::Resume).await?;
        Ok(())
    }

    pub async fn flush(&self) -> Result<()> {
        self.control.send(StageControl::Flush).await?;
        Ok(())
    }
}

pub struct PipelineBuilder {
    buffer_size: usize,
}

impl PipelineBuilder {
    pub fn new() -> Self {
        Self {
            buffer_size: 1,
        }
    }

    pub fn buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }

    pub fn build(self) -> Pipeline {
        Pipeline {
            buffer_size: self.buffer_size,
            stages: HashMap::new(),
            connections: Vec::new(),
            state: Arc::new(AtomicLifecycleState::new(LifecycleState::Created)),
            next_id: 0,
        }
    }
}

impl Default for PipelineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

struct Connection {
    from: StageId,
    to: StageId,
}

pub struct Pipeline {
    buffer_size: usize,
    stages: HashMap<StageId, Box<dyn std::any::Any + Send + Sync>>,
    connections: Vec<Connection>,
    state: Arc<AtomicLifecycleState>,
    next_id: StageId,
}

impl Pipeline {
    pub fn builder() -> PipelineBuilder {
        PipelineBuilder::new()
    }

    pub fn state(&self) -> LifecycleState {
        self.state.load()
    }

    pub fn add_stage<S: Stage + 'static>(&mut self, stage: S) -> Result<StageRef> {
        let id = self.next_id;
        self.next_id += 1;
        
        let name = stage.meta().name.clone();
        
        let running = Box::new(run_stage(stage, self.buffer_size));
        
        self.stages.insert(id, running);
        
        Ok(StageRef { id, name })
    }

    pub fn connect(&mut self, from: StageRef, to: StageRef) {
        self.connections.push(Connection {
            from: from.id,
            to: to.id,
        });
    }

    pub async fn start(&self) -> Result<(), LifecycleError> {
        self.state.transition(LifecycleState::Starting)?;
        
        self.state.transition(LifecycleState::Running)?;
        Ok(())
    }

    pub async fn stop(&self) -> Result<(), LifecycleError> {
        self.state.transition(LifecycleState::Stopping)?;
        
        self.state.store(LifecycleState::Stopped);
        Ok(())
    }

    pub async fn pause(&self) -> Result<(), LifecycleError> {
        self.state.transition(LifecycleState::Pausing)?;
        
        self.state.transition(LifecycleState::Paused)?;
        Ok(())
    }

    pub async fn resume(&self) -> Result<(), LifecycleError> {
        self.state.transition(LifecycleState::Resuming)?;
        
        self.state.transition(LifecycleState::Running)?;
        Ok(())
    }
}
