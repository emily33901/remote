use std::time::Duration;

use signal::{SignallingControl, SignallingEvent};
use tokio::sync::mpsc;

async fn wait_for_id(events: &mut mpsc::Receiver<SignallingEvent>) -> signal::PeerId {
    loop {
        match events.recv().await {
            Some(SignallingEvent::Id(id)) => return id,
            Some(_) => continue,
            None => panic!("signalling channel closed"),
        }
    }
}

#[tokio::test]
async fn signalling_server_client_connection() {
    let _ = tracing_subscriber::fmt::try_init();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    drop(listener);

    tracing::info!("Starting signalling server on {}", addr);

    let server_handle = tokio::spawn({
        let addr = addr.clone();
        async move {
            let _ = signal::server(&addr).await;
        }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let (mut ctrl_a, mut evt_a) = signal::client(&format!("ws://{}", addr))
        .await
        .expect("client A failed to connect");
    let (mut ctrl_b, mut evt_b) = signal::client(&format!("ws://{}", addr))
        .await
        .expect("client B failed to connect");

    let id_a = wait_for_id(&mut evt_a).await;
    let id_b = wait_for_id(&mut evt_b).await;

    tracing::info!("Peer A: {}, Peer B: {}", id_a, id_b);

    ctrl_a
        .send(SignallingControl::RequestConnection(id_b.clone()))
        .await
        .expect("failed to request connection");

    let mut conn_id = None;
    loop {
        match evt_b.recv().await {
            Some(SignallingEvent::ConectionRequest(peer_id, cid)) => {
                assert_eq!(peer_id, id_a);
                conn_id = Some(cid);
                break;
            }
            Some(_) => continue,
            None => panic!("signalling channel closed"),
        }
    }

    let conn_id = conn_id.expect("failed to receive connection ID");
    ctrl_b
        .send(SignallingControl::AcceptConnection(conn_id))
        .await
        .expect("failed to accept connection");

    loop {
        match evt_a.recv().await {
            Some(SignallingEvent::ConnectionAccepted(peer_id, _)) => {
                assert_eq!(peer_id, id_b);
                break;
            }
            Some(_) => continue,
            None => panic!("signalling channel closed"),
        }
    }

    tracing::info!("Signalling connection established successfully!");
    server_handle.abort();
}
