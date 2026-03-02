use crate::lifecycle::Stage;
use crate::Encoding;

use super::types::*;

pub trait Capture: Stage<Input = (), Output = CaptureFrame> + Send + Sync {
    fn output_format(&self) -> PixelFormat;
}

pub trait Converter: Stage<Input = ConvertFrame, Output = ConvertFrame> + Send + Sync {
    fn input_format(&self) -> PixelFormat;
    fn output_format(&self) -> PixelFormat;
}

pub trait Encoder: Stage<Input = EncodeFrame, Output = EncodedData> + Send + Sync {
    fn encoding(&self) -> Encoding;
    fn set_bitrate(&mut self, bitrate: u32);
    fn request_keyframe(&mut self);
}

pub trait Decoder: Stage<Input = EncodedData, Output = DecodeFrame> + Send + Sync {
    fn encoding(&self) -> Encoding;
}

pub trait Presenter: Stage<Input = PresentFrame, Output = ()> + Send + Sync {
    fn dimensions(&self) -> (u32, u32);
    fn resize(&mut self, width: u32, height: u32);
}
