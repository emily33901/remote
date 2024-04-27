mod openh264;
mod windows;

use std::str::FromStr;

use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use eyre::Result;
use tokio::sync::mpsc;

use crate::VideoBuffer;

pub enum DecoderControl {
    Data(VideoBuffer),
}
pub enum DecoderEvent {
    Frame(ID3D11Texture2D, crate::Timestamp),
}

#[derive(Debug)]
pub enum Decoder {
    OpenH264,
    MediaFoundation,
}

impl Decoder {
    #[tracing::instrument]
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

impl FromStr for Decoder {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "media-foundation" | "MediaFoundation" => Ok(Self::MediaFoundation),
            "open-h264" | "OpenH264" => Ok(Self::OpenH264),
            _ => Err(()),
        }
    }
}
