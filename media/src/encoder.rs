mod openh264;
mod windows;

use std::str::FromStr;

use ::windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;

use eyre::Result;

use crate::VideoBuffer;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum FrameIsKeyframe {
    Yes,
    No,
    Perhaps,
}

#[derive(Clone)]
pub enum EncoderControl {
    Frame(ID3D11Texture2D, crate::Timestamp),
}

pub enum EncoderEvent {
    Data(VideoBuffer),
}

#[derive(Clone, Copy)]
pub enum Encoder {
    MediaFoundation,
    X264,
    OpenH264,
}

impl Encoder {
    pub async fn run(
        &self,
        width: u32,
        height: u32,
        target_framerate: u32,
        target_bitrate: u32,
    ) -> Result<(mpsc::Sender<EncoderControl>, mpsc::Receiver<EncoderEvent>)> {
        match self {
            Encoder::MediaFoundation => {
                windows::h264_encoder(width, height, target_framerate, target_bitrate).await
            }
            Encoder::X264 => todo!("x264 not implemented"),
            Encoder::OpenH264 => {
                openh264::h264_encoder(width, height, target_framerate, target_bitrate).await
            }
        }
    }
}

impl FromStr for Encoder {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "media-foundation" | "MediaFoundation" => Ok(Self::MediaFoundation),
            "open-h264" | "OpenH264" => Ok(Self::OpenH264),
            _ => Err(()),
        }
    }
}
