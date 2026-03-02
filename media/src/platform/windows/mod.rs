use anyhow::Result;
use std::time::Duration;

use crate::traits::Capture;
use crate::types::{CaptureConfig, CapturedFrame, OutputInfo};

pub struct WindowsCapture;

impl WindowsCapture {
    pub fn new() -> Self {
        Self
    }
}

impl Capture for WindowsCapture {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>> {
        todo!("Implement DXGI output enumeration")
    }

    fn start(&mut self, _config: CaptureConfig) -> Result<()> {
        todo!("Implement DXGI desktop duplication start")
    }

    fn stop(&mut self) -> Result<()> {
        todo!("Implement stop")
    }

    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>> {
        todo!("Implement frame capture")
    }
}

impl Default for WindowsCapture {
    fn default() -> Self {
        Self::new()
    }
}
