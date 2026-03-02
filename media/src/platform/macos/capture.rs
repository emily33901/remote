use std::time::Duration;

use anyhow::Result;

use crate::traits::Capture;
use crate::types::{CaptureConfig, CapturedFrame, CpuBuffer, OutputInfo, PixelFormat, VideoFrame};

pub struct MacOSCapture {
    width: u32,
    height: u32,
    running: bool,
}

impl MacOSCapture {
    pub fn new() -> Self {
        Self {
            width: 1920,
            height: 1080,
            running: false,
        }
    }
}

impl Capture for MacOSCapture {
    fn enumerate_outputs(&self) -> Result<Vec<OutputInfo>> {
        Ok(vec![OutputInfo {
            id: 0,
            name: "Main Display".to_string(),
            width: 1920,
            height: 1080,
            is_primary: true,
        }])
    }

    fn start(&mut self, _config: CaptureConfig) -> Result<()> {
        tracing::info!("Starting macOS screen capture");

        todo!(
            "ScreenCaptureKit implementation requires macOS. \
             Use SCScreenshotManager for single captures or SCStream for continuous capture. \
             Requires macOS 12.3+ and Screen Recording entitlement."
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
            "ScreenCaptureKit frame capture requires macOS. \
             Use CVImageBuffer from SCStreamOutputDelegate."
        );
    }
}

impl Default for MacOSCapture {
    fn default() -> Self {
        Self::new()
    }
}
