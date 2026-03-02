use std::time::Duration;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    Bgra,
    Rgba,
    Nv12,
    I420,
    I444,
}

impl PixelFormat {
    pub fn bytes_per_pixel(&self) -> u32 {
        match self {
            PixelFormat::Bgra | PixelFormat::Rgba => 4,
            PixelFormat::Nv12 => 1,
            PixelFormat::I420 => 1,
            PixelFormat::I444 => 1,
        }
    }

    pub fn has_alpha(&self) -> bool {
        matches!(self, PixelFormat::Bgra | PixelFormat::Rgba)
    }

    pub fn is_planar(&self) -> bool {
        matches!(
            self,
            PixelFormat::Nv12 | PixelFormat::I420 | PixelFormat::I444
        )
    }
}

#[derive(Debug, Clone)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

impl FrameSize {
    pub fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn pixel_count(&self) -> u64 {
        self.width as u64 * self.height as u64
    }
}

#[derive(Debug, Clone)]
pub struct GpuTexture {
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
}

#[derive(Debug, Clone)]
pub struct CpuBuffer {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub stride: Vec<u32>,
}

impl CpuBuffer {
    pub fn new_i420(width: u32, height: u32) -> Self {
        let y_size = (width * height) as usize;
        let uv_size = (width / 2 * height / 2) as usize;
        let total_size = y_size + 2 * uv_size;

        Self {
            data: vec![0u8; total_size],
            width,
            height,
            format: PixelFormat::I420,
            stride: vec![width, width / 2, width / 2],
        }
    }

    pub fn new_nv12(width: u32, height: u32) -> Self {
        let y_size = (width * height) as usize;
        let uv_size = (width / 2 * height) as usize;
        let total_size = y_size + uv_size;

        Self {
            data: vec![0u8; total_size],
            width,
            height,
            format: PixelFormat::Nv12,
            stride: vec![width, width],
        }
    }

    pub fn new_bgra(width: u32, height: u32) -> Self {
        let size = (width * height * 4) as usize;

        Self {
            data: vec![0u8; size],
            width,
            height,
            format: PixelFormat::Bgra,
            stride: vec![width * 4],
        }
    }
}

#[derive(Debug, Clone)]
pub enum VideoFrame {
    Gpu(GpuTexture),
    Cpu(CpuBuffer),
}

impl VideoFrame {
    pub fn size(&self) -> FrameSize {
        match self {
            VideoFrame::Gpu(tex) => FrameSize::new(tex.width, tex.height),
            VideoFrame::Cpu(buf) => FrameSize::new(buf.width, buf.height),
        }
    }

    pub fn format(&self) -> PixelFormat {
        match self {
            VideoFrame::Gpu(tex) => tex.format,
            VideoFrame::Cpu(buf) => buf.format,
        }
    }

    pub fn is_gpu(&self) -> bool {
        matches!(self, VideoFrame::Gpu(_))
    }
}

#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub is_keyframe: bool,
    pub timestamp: Duration,
    pub duration: Duration,
    pub sequence_header: Option<Vec<u8>>,
}

impl EncodedPacket {
    pub fn new(data: Vec<u8>, is_keyframe: bool, timestamp: Duration) -> Self {
        Self {
            data,
            is_keyframe,
            timestamp,
            duration: Duration::ZERO,
            sequence_header: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct OutputInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub output_id: Option<u32>,
    pub capture_cursor: bool,
    pub capture_audio: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            output_id: None,
            capture_cursor: true,
            capture_audio: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub frame: VideoFrame,
    pub timestamp: Duration,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureRegion {
    FullScreen,
    Region {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    Window {
        window_id: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Timestamp(Duration);

impl Timestamp {
    pub fn new(offset: Duration) -> Self {
        Self(offset)
    }

    pub fn new_hns(hns: i64) -> Self {
        Timestamp(Duration::from_nanos(100 * hns as u64))
    }

    pub fn new_millis(millis: u64) -> Self {
        Timestamp(Duration::from_millis(millis))
    }

    pub fn new_diff(
        start: std::time::SystemTime,
        now: std::time::SystemTime,
    ) -> Result<Self, std::time::SystemTimeError> {
        Ok(Self(now.duration_since(start)?))
    }

    pub fn new_diff_instant(start: std::time::Instant, now: std::time::Instant) -> Self {
        Self(now.duration_since(start))
    }

    pub fn hns(&self) -> i64 {
        (self.0.as_nanos() / 100) as i64
    }

    pub fn sub(&self, other: Self) -> Duration {
        self.0.saturating_sub(other.0)
    }

    pub fn duration(&self) -> Duration {
        self.0
    }
}

impl From<Duration> for Timestamp {
    fn from(d: Duration) -> Self {
        Self(d)
    }
}

impl From<Timestamp> for Duration {
    fn from(t: Timestamp) -> Self {
        t.0
    }
}
