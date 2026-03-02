use std::time::Duration;

use anyhow::Result;

use crate::traits::Capture;
use crate::types::{CaptureConfig, CapturedFrame, OutputInfo, VideoFrame};

pub struct WindowsCapture {
    width: u32,
    height: u32,
    running: bool,
}

impl WindowsCapture {
    pub fn new() -> Self {
        Self {
            width: 1920,
            height: 1080,
            running: false,
        }
    }
}

impl Capture for WindowsCapture {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(vec![OutputInfo {
            id: 0,
            name: "Primary Display".to_string(),
            width: 1920,
            height: 1080,
            is_primary: true,
        }])
    }

    fn start(&mut self, _config: CaptureConfig) -> Result<()> {
        tracing::info!("Starting Windows DXGI desktop duplication");

        self.running = true;

        todo!(
            "Use existing desktop_duplication.rs implementation. \
             Create DXGI Desktop Duplication context and start frame loop. \
             Next step DX12 interop."
        );
    }

    fn stop(&mut self) -> Result<()> {
        self.running = false;
        Ok(())
    }

    fn next_frame(&mut self, _timeout: Duration) -> Result<Option<CapturedFrame>> {
        if !self.running {
            return Ok(None);
        }

        todo!(
            "Get next frame from DXGI Desktop Duplication. \
             Convert ID3D11Texture2D to wgpu texture via DX12 interop."
        );
    }
}

impl Default for WindowsCapture {
    fn default() -> Self {
        Self::new()
    }
}
