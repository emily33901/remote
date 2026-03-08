mod common;

use std::sync::Arc;
use std::time::Duration;

use common::MockPeerConnection;
use remote::connection::logic_channel;
use remote::protocol::{LogicMessage, Mode, StreamRequest, StreamRequestResponse};
use media::{Encoding, EncodingOptions, H264EncodingOptions, RateControlMode};
use rtc::ChannelControl;

use tokio::time::timeout;

async fn wait_for_open(mock: &MockPeerConnection, controlling: bool) {
    let (logic_tx, _) = logic_channel(mock.as_ref(), controlling).await.unwrap();
    
    logic_tx.send(LogicMessage::StreamRequest(StreamRequest::default())).await.unwrap();
    
    let result = tokio::time::timeout(
        Duration::from_millis(100), 
        mock.recv_outgoing("logic")
    ).await;
    assert!(result.is_err(), "should timeout - channel not open yet");
    
    mock.open_channel("logic").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    
    let result = tokio::time::timeout(
        Duration::from_millis(200), 
        mock.recv_outgoing("logic")
    ).await;
    assert!(result.is_ok(), "should receive message after open");
    
    match result.unwrap() {
        ChannelControl::Send(data) => {
            let msg: LogicMessage = bincode::deserialize(&data).unwrap();
            assert!(matches!(msg, LogicMessage::StreamRequest(_)));
        }
        _ => panic!("expected Send"),
    }
}

