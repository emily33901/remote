mod openh264;
mod windows;

use std::str::FromStr;

use serde::Deserialize;
use serde::Serialize;
use tokio::sync::mpsc;

use eyre::Result;

use crate::texture_pool::Texture;
use crate::Encoding;
use crate::EncodingOptions;
use crate::H264EncodingOptions;
use crate::Statistics;
use crate::SupportsEncodingOptions;
use crate::VideoBuffer;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum FrameIsKeyframe {
    Yes,
    No,
    Perhaps,
}

// #[derive(Clone)]
pub enum EncoderControl {
    Frame(Texture, crate::Timestamp, Statistics),
}

pub enum EncoderEvent {
    Data(VideoBuffer),
}

#[derive(Debug, Clone, Copy)]
pub enum Encoder {
    MediaFoundation,
    X264,
    OpenH264,
}

impl Encoder {
    #[tracing::instrument]
    pub async fn run(
        &self,
        width: u32,
        height: u32,
        frame_rate: u32,
        encoding: Encoding,
        encoding_options: EncodingOptions,
    ) -> Result<(mpsc::Sender<EncoderControl>, mpsc::Receiver<EncoderEvent>)> {
        match encoding {
            Encoding::H264 => {
                let options: H264EncodingOptions = encoding_options.try_into()?;
                match self {
                    Encoder::MediaFoundation => {
                        windows::h264_encoder(width, height, frame_rate, options.rate_control).await
                    }
                    Encoder::X264 => todo!("x264 not implemented"),
                    Encoder::OpenH264 => {
                        openh264::h264_encoder(width, height, frame_rate, options.rate_control)
                            .await
                    }
                }
            }
            _ => unimplemented!(),
        }
    }

    pub fn supported_encodings(&self) -> &[Encoding] {
        match self {
            Encoder::MediaFoundation => {
                &[Encoding::AV1, Encoding::H264, Encoding::H265, Encoding::VP9]
            }
            Encoder::X264 => &[Encoding::H264],
            Encoder::OpenH264 => &[Encoding::H264],
        }
    }

    pub fn supports_encoding_options(&self, options: &EncodingOptions) -> SupportsEncodingOptions {
        SupportsEncodingOptions::Yes
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
