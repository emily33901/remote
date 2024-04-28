use media::{decoder::Decoder, encoder::Encoder};

use once_cell::sync::OnceCell;
use std::str::FromStr;

pub(crate) struct Config {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) bitrate: u32,
    pub(crate) framerate: u32,
    pub(crate) media_filename: Option<String>,
    pub(crate) encoder_api: Encoder,
    pub(crate) decoder_api: Decoder,
    pub(crate) log_level: tracing::level_filters::LevelFilter,
    pub(crate) webrtc_api: rtc::Api,
    pub(crate) signal_server: String,
}

static CONFIG: OnceCell<Config> = OnceCell::new();

impl Config {
    pub(crate) fn load() -> &'static Config {
        CONFIG
            .get_or_try_init(|| {
                dotenv::dotenv()?;

                eyre::Ok(Config {
                    width: u32::from_str(&std::env::var("width")?)?,
                    height: u32::from_str(&std::env::var("height")?)?,
                    bitrate: u32::from_str(&std::env::var("bitrate")?)?,
                    framerate: u32::from_str(&std::env::var("framerate")?)?,

                    media_filename: std::env::var("media_filename").ok(),

                    webrtc_api: rtc::Api::from_str(&std::env::var("webrtc_api")?)?,
                    decoder_api: media::decoder::Decoder::from_str(&std::env::var("decoder_api")?)
                        .map_err(|_| eyre::eyre!(""))?,
                    encoder_api: media::encoder::Encoder::from_str(&std::env::var("encoder_api")?)
                        .map_err(|_| eyre::eyre!(""))?,
                    log_level: tracing::level_filters::LevelFilter::from_str(&std::env::var(
                        "log_level",
                    )?)?,
                    signal_server: std::env::var("signal_server")?,
                })
            })
            .unwrap()
    }
}
