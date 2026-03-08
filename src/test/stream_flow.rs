use std::sync::Arc;
use std::time::Duration;

use super::common::{recv_logic_message_skip_pong, MockPeerConnection};
use crate::connection::logic_channel;
use crate::protocol::{LogicMessage, Mode, StreamRequest, StreamRequestResponse};
use media::{Encoding, EncodingOptions, H264EncodingOptions, RateControlMode};
use rtc::ChannelControl;

#[tokio::test]
async fn logic_channel_waits_for_open_before_sending() {
    let mock = Arc::new(MockPeerConnection::new());

    let (logic_tx, _) = logic_channel(mock.as_ref(), true).await.unwrap();

    logic_tx
        .send(LogicMessage::StreamRequest(StreamRequest::default()))
        .await
        .unwrap();

    let result =
        tokio::time::timeout(Duration::from_millis(100), mock.recv_outgoing("logic")).await;
    assert!(result.is_err(), "should timeout - channel not open yet");

    mock.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let outgoing = mock
        .recv_outgoing_skip_ping("logic")
        .await
        .expect("no message");

    match outgoing {
        ChannelControl::Send(data) => {
            let msg: LogicMessage = bincode::deserialize(&data).unwrap();
            assert!(matches!(msg, LogicMessage::StreamRequest(_)));
        }
        _ => panic!("expected Send"),
    }
}

#[tokio::test]
async fn logic_channel_forwards_incoming_messages() {
    let mock = Arc::new(MockPeerConnection::new());

    let (_, mut logic_rx) = logic_channel(mock.as_ref(), false).await.unwrap();

    mock.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let response = StreamRequestResponse::Accept {
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        },
        encoding: Encoding::H264,
        encoding_options: EncodingOptions::H264(H264EncodingOptions {
            rate_control: RateControlMode::Bitrate(8_000_000),
        }),
    };
    let data = bincode::serialize(&LogicMessage::StreamRequestResponse(response)).unwrap();

    mock.send_to_channel("logic", data).await;

    let received = logic_rx.recv().await.expect("no message");

    match received {
        LogicMessage::StreamRequestResponse(StreamRequestResponse::Accept { mode, .. }) => {
            assert_eq!(mode.width, 1920);
            assert_eq!(mode.height, 1080);
            assert_eq!(mode.refresh_rate, 60);
        }
        _ => panic!("Expected StreamRequestResponse::Accept"),
    }
}

#[tokio::test]
async fn stream_request_round_trip() {
    let peer_a = Arc::new(MockPeerConnection::new());
    let peer_b = Arc::new(MockPeerConnection::new());

    let (logic_tx_a, _) = logic_channel(peer_a.as_ref(), true).await.unwrap();
    let (_, mut logic_rx_b) = logic_channel(peer_b.as_ref(), false).await.unwrap();

    peer_a.open_channel("logic").await;
    peer_b.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let request = StreamRequest {
        preferred_mode: Some(Mode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        }),
        preferred_encoding: Some(Encoding::H264),
        preferred_encoding_options: None,
    };
    logic_tx_a
        .send(LogicMessage::StreamRequest(request))
        .await
        .unwrap();

    let outgoing = peer_a.recv_outgoing_skip_ping("logic").await;
    let data = match outgoing {
        Some(ChannelControl::Send(data)) => data,
        _ => panic!("Expected Send"),
    };

    peer_b.send_to_channel("logic", data).await;

    let received = recv_logic_message_skip_pong(&mut logic_rx_b)
        .await
        .expect("no request");

    match received {
        LogicMessage::StreamRequest(req) => {
            assert_eq!(req.preferred_mode.unwrap().width, 1920);
        }
        _ => panic!("Expected StreamRequest"),
    }
}

#[tokio::test]
async fn multiple_requests() {
    let peer_a = Arc::new(MockPeerConnection::new());
    let peer_b = Arc::new(MockPeerConnection::new());

    let (logic_tx_a, _) = logic_channel(peer_a.as_ref(), true).await.unwrap();
    let (_, mut logic_rx_b) = logic_channel(peer_b.as_ref(), false).await.unwrap();

    peer_a.open_channel("logic").await;
    peer_b.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    for i in 1u32..=3 {
        let request = StreamRequest {
            preferred_mode: Some(Mode {
                width: 1920 / i,
                height: 1080 / i,
                refresh_rate: 60,
            }),
            preferred_encoding: Some(Encoding::H264),
            preferred_encoding_options: None,
        };
        logic_tx_a
            .send(LogicMessage::StreamRequest(request))
            .await
            .unwrap();

        let outgoing = peer_a.recv_outgoing_skip_ping("logic").await;
        let data = match outgoing {
            Some(ChannelControl::Send(data)) => data,
            _ => panic!("Expected Send"),
        };

        peer_b.send_to_channel("logic", data).await;

        let received = recv_logic_message_skip_pong(&mut logic_rx_b)
            .await
            .expect("no request");

        match received {
            LogicMessage::StreamRequest(req) => {
                assert_eq!(req.preferred_mode.unwrap().width, 1920 / i);
            }
            _ => panic!("Expected StreamRequest"),
        }
    }
}

#[tokio::test]
async fn ping_pong() {
    let mock = Arc::new(MockPeerConnection::new());

    let (_, _logic_rx) = logic_channel(mock.as_ref(), true).await.unwrap();

    mock.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let ping_data = bincode::serialize(&LogicMessage::Ping).unwrap();
    mock.send_to_channel("logic", ping_data).await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let outgoing = mock.recv_outgoing_skip_ping("logic").await;

    let pong_data = match outgoing {
        Some(ChannelControl::Send(data)) => data,
        _ => panic!("Expected Send"),
    };

    let response: LogicMessage = bincode::deserialize(&pong_data).unwrap();
    assert!(matches!(response, LogicMessage::Pong));
}

#[tokio::test]
async fn stream_request_response_flow() {
    let peer_a = Arc::new(MockPeerConnection::new());
    let peer_b = Arc::new(MockPeerConnection::new());

    let (logic_tx_a, mut logic_rx_a) = logic_channel(peer_a.as_ref(), true).await.unwrap();
    let (logic_tx_b, mut logic_rx_b) = logic_channel(peer_b.as_ref(), false).await.unwrap();

    peer_a.open_channel("logic").await;
    peer_b.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    let request = StreamRequest {
        preferred_mode: Some(Mode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        }),
        preferred_encoding: Some(Encoding::H264),
        preferred_encoding_options: None,
    };
    logic_tx_a
        .send(LogicMessage::StreamRequest(request))
        .await
        .unwrap();

    let outgoing = peer_a.recv_outgoing_skip_ping("logic").await;
    let data = match outgoing {
        Some(ChannelControl::Send(data)) => data,
        _ => panic!("Expected Send"),
    };

    peer_b.send_to_channel("logic", data).await;

    let received = recv_logic_message_skip_pong(&mut logic_rx_b)
        .await
        .expect("no request");

    let received_request = match received {
        LogicMessage::StreamRequest(req) => req,
        _ => panic!("Expected StreamRequest"),
    };
    assert_eq!(received_request.preferred_mode.unwrap().width, 1920);

    let response = StreamRequestResponse::Accept {
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_rate: 60,
        },
        encoding: Encoding::H264,
        encoding_options: EncodingOptions::H264(H264EncodingOptions {
            rate_control: RateControlMode::Bitrate(8_000_000),
        }),
    };
    logic_tx_b
        .send(LogicMessage::StreamRequestResponse(response))
        .await
        .unwrap();

    let outgoing = peer_b.recv_outgoing_skip_ping("logic").await;
    let data = match outgoing {
        Some(ChannelControl::Send(data)) => data,
        _ => panic!("Expected Send"),
    };

    peer_a.send_to_channel("logic", data).await;

    let received = recv_logic_message_skip_pong(&mut logic_rx_a)
        .await
        .expect("no response");

    match received {
        LogicMessage::StreamRequestResponse(StreamRequestResponse::Accept { mode, .. }) => {
            assert_eq!(mode.width, 1920);
            assert_eq!(mode.height, 1080);
            assert_eq!(mode.refresh_rate, 60);
        }
        _ => panic!("Expected StreamRequestResponse::Accept"),
    }
}
