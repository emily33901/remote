use media::{Encoding, EncodingOptions, H264EncodingOptions, RateControlMode};
use remote::{LogicMessage, Mode, StreamRequest, StreamRequestResponse};

#[test]
fn test_stream_request_serialization() {
    let request = StreamRequest {
        preferred_mode: Some(Mode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        }),
        preferred_encoding: Some(Encoding::H264),
        preferred_encoding_options: Some(EncodingOptions::H264(H264EncodingOptions {
            rate_control: RateControlMode::Bitrate(8_000_000),
        })),
    };

    let encoded = bincode::serialize(&request).unwrap();
    let decoded: StreamRequest = bincode::deserialize(&encoded).unwrap();

    assert_eq!(decoded.preferred_mode.as_ref().unwrap().width, 1920);
    assert_eq!(decoded.preferred_mode.as_ref().unwrap().height, 1080);
    assert_eq!(decoded.preferred_mode.as_ref().unwrap().refresh_rate, 60);
    assert!(matches!(decoded.preferred_encoding, Some(Encoding::H264)));
}

#[test]
fn test_stream_request_default() {
    let request = StreamRequest::default();

    assert!(request.preferred_mode.is_none());
    assert!(request.preferred_encoding.is_none());
    assert!(request.preferred_encoding_options.is_none());
}

#[test]
fn test_stream_request_response_accept_serialization() {
    let response = StreamRequestResponse::Accept {
        mode: Mode {
            width: 1280,
            height: 720,
            refresh_rate: 30,
        },
        encoding: Encoding::H264,
        encoding_options: EncodingOptions::H264(H264EncodingOptions {
            rate_control: RateControlMode::Quality(80),
        }),
    };

    let encoded = bincode::serialize(&response).unwrap();
    let decoded: StreamRequestResponse = bincode::deserialize(&encoded).unwrap();

    match decoded {
        StreamRequestResponse::Accept { mode, encoding, .. } => {
            assert_eq!(mode.width, 1280);
            assert_eq!(mode.height, 720);
            assert!(matches!(encoding, Encoding::H264));
        }
        _ => panic!("Expected Accept variant"),
    }
}

#[test]
fn test_stream_request_response_reject_serialization() {
    let response = StreamRequestResponse::Reject;

    let encoded = bincode::serialize(&response).unwrap();
    let decoded: StreamRequestResponse = bincode::deserialize(&encoded).unwrap();

    assert!(matches!(decoded, StreamRequestResponse::Reject));
}

#[test]
fn test_stream_request_response_negotiate_serialization() {
    let response = StreamRequestResponse::Negotiate {
        viable_modes: vec![
            Mode {
                width: 1920,
                height: 1080,
                refresh_rate: 60,
            },
            Mode {
                width: 1280,
                height: 720,
                refresh_rate: 30,
            },
        ],
        viable_encodings: vec![Encoding::H264, Encoding::H265],
    };

    let encoded = bincode::serialize(&response).unwrap();
    let decoded: StreamRequestResponse = bincode::deserialize(&encoded).unwrap();

    match decoded {
        StreamRequestResponse::Negotiate {
            viable_modes,
            viable_encodings,
        } => {
            assert_eq!(viable_modes.len(), 2);
            assert_eq!(viable_encodings.len(), 2);
        }
        _ => panic!("Expected Negotiate variant"),
    }
}

#[test]
fn test_logic_message_stream_request_serialization() {
    let request = StreamRequest::default();
    let message = LogicMessage::StreamRequest(request);

    let encoded = bincode::serialize(&message).unwrap();
    let decoded: LogicMessage = bincode::deserialize(&encoded).unwrap();

    assert!(matches!(decoded, LogicMessage::StreamRequest(_)));
}

#[test]
fn test_logic_message_stream_response_serialization() {
    let response = StreamRequestResponse::Reject;
    let message = LogicMessage::StreamRequestResponse(response);

    let encoded = bincode::serialize(&message).unwrap();
    let decoded: LogicMessage = bincode::deserialize(&encoded).unwrap();

    match decoded {
        LogicMessage::StreamRequestResponse(resp) => {
            assert!(matches!(resp, StreamRequestResponse::Reject));
        }
        _ => panic!("Expected StreamRequestResponse variant"),
    }
}

#[test]
fn test_logic_message_ping_pong() {
    let ping = LogicMessage::Ping;
    let pong = LogicMessage::Pong;

    let encoded_ping = bincode::serialize(&ping).unwrap();
    let encoded_pong = bincode::serialize(&pong).unwrap();

    let decoded_ping: LogicMessage = bincode::deserialize(&encoded_ping).unwrap();
    let decoded_pong: LogicMessage = bincode::deserialize(&encoded_pong).unwrap();

    assert!(matches!(decoded_ping, LogicMessage::Ping));
    assert!(matches!(decoded_pong, LogicMessage::Pong));
}
