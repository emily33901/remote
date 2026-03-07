use media::encoder::Encoder;
use rtc::Api;

#[derive(Clone)]
pub struct PeerConfig {
    pub signal_server: String,
    pub webrtc_api: Api,
    pub encoder: Encoder,
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate: u32,
    pub media_filename: Option<String>,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            signal_server: "wss://signall.ing".to_string(),
            webrtc_api: Api::WebrtcRs,
            encoder: Encoder::MediaFoundation,
            width: 1920,
            height: 1080,
            framerate: 30,
            bitrate: 8_000_000,
            media_filename: None,
        }
    }
}
