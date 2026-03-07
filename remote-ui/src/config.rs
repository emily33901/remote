use once_cell::sync::OnceCell;
use remote::PeerConfig;

pub struct Config {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub peer_config: PeerConfig,
}

static CONFIG: OnceCell<Config> = OnceCell::new();

impl Config {
    pub fn load() -> &'static Config {
        CONFIG.get_or_init(|| {
            dotenv::dotenv().ok();

            let width: u32 = std::env::var("width")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1920);
            let height: u32 = std::env::var("height")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(1080);
            let framerate: u32 = std::env::var("framerate")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            let bitrate: u32 = std::env::var("bitrate")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8_000_000);
            let signal_server =
                std::env::var("signal_server").unwrap_or_else(|_| "wss://signall.ing".to_string());
            let media_filename = std::env::var("media_filename").ok();

            Self {
                width,
                height,
                framerate,
                peer_config: PeerConfig {
                    signal_server,
                    webrtc_api: rtc::Api::WebrtcRs,
                    encoder: media::encoder::Encoder::MediaFoundation,
                    width,
                    height,
                    framerate,
                    bitrate,
                    media_filename,
                },
            }
        })
    }
}
