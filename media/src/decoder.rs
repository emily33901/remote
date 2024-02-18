mod openh264;
mod windows;



use ::windows::{Win32::Graphics::Direct3D11::ID3D11Texture2D};
use eyre::Result;
use tokio::sync::mpsc;

use crate::VideoBuffer;

pub enum DecoderControl {
    Data(VideoBuffer),
}
pub enum DecoderEvent {
    Frame(ID3D11Texture2D, std::time::SystemTime),
}

pub enum Decoder {
    OpenH264,
    MediaFoundation,
}

impl Decoder {
    pub async fn run(
        &self,
        width: u32,
        height: u32,
        target_framerate: u32,
        target_bitrate: u32,
    ) -> Result<(mpsc::Sender<DecoderControl>, mpsc::Receiver<DecoderEvent>)> {
        match self {
            Decoder::MediaFoundation => {
                windows::h264_decoder(width, height, target_framerate, target_bitrate).await
            }
            Decoder::OpenH264 => {
                openh264::h264_decoder(width, height, target_framerate, target_bitrate).await
                // openh264::h264_decoder(width, height, target_framerate, target_bitrate).await
            }
        }
    }
}
