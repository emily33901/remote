use std::time::Duration;

use anyhow::Result;

use crate::types::{CaptureConfig, CapturedFrame, OutputInfo};

pub trait Capture: Send + Sync {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>>;
    fn start(&mut self, config: CaptureConfig) -> Result<()>;
    fn stop(&mut self) -> Result<()>;
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<CapturedFrame>>;
}
