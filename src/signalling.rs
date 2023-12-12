use std::error::Error;
use std::{collections::HashMap, sync::Arc};

use futures::stream::SplitSink;
use futures::{FutureExt, StreamExt, TryStreamExt};
use futures::{Sink, SinkExt};
use serde::Deserialize;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message::{self, Binary, Close, Frame, Ping, Pong, Text};
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use eyre::{eyre, Result};

use crate::{ConnectionId, PeerId, ARBITRARY_CHANNEL_LIMIT};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum SignallingError {
    NoSuchPeer(PeerId),
}

#[derive(Debug, Serialize, Deserialize)]
struct PeerToServerMessage {
    job_id: usize,
    inner: PeerToServer,
}

#[derive(Debug, Serialize, Deserialize)]
struct ServerToPeerMessage {
    job_id: usize,
    inner: ServerToPeer,
}

#[derive(Debug, Serialize, Deserialize)]
enum PeerToServer {
    IceCandidate(PeerId, String),
    Offer(PeerId, String),
    Answer(PeerId, String),
    ConnectToPeer(PeerId),
    AcceptConnection(ConnectionId),
}

#[derive(Debug, Serialize, Deserialize)]
enum ServerToPeer {
    Id(PeerId),
    ConnectionRequest(PeerId, ConnectionId),
    ConnectionAccepted(PeerId, ConnectionId),
    Offer(PeerId, String),
    Answer(PeerId, String),
    IceCandidate(PeerId, String),
    Error(SignallingError),
}

fn make_id() -> Uuid {
    Uuid::new_v4()
}

type PeerMap = Arc<Mutex<HashMap<Uuid, mpsc::Sender<ServerToPeerMessage>>>>;

#[derive(Debug)]
struct ConnectionRequest {
    accept: tokio::sync::oneshot::Sender<()>,
    reject: tokio::sync::oneshot::Sender<()>,
    requester: PeerId,
    requestee: PeerId,
}

type ConnectionRequestMap = Arc<Mutex<HashMap<Uuid, ConnectionRequest>>>;

async fn handle_incoming_message_inner(
    our_peer_id: PeerId,
    connection_requests: ConnectionRequestMap,
    peers: PeerMap,
    message: PeerToServerMessage,
) -> Result<()> {
    log::debug!("{message:?}");
    match message.inner {
        PeerToServer::IceCandidate(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::IceCandidate(our_peer_id, inner),
                    })
                    .await?)
            } else {
                todo!("handle no such peer")
            }
        }
        PeerToServer::Offer(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::Offer(our_peer_id, inner),
                    })
                    .await?)
            } else {
                todo!("handle no such peer")
            }
        }
        PeerToServer::Answer(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::Answer(our_peer_id, inner),
                    })
                    .await?)
            } else {
                todo!("handle no such peer")
            }
        }
        PeerToServer::ConnectToPeer(peer_id) => {
            if peer_id == our_peer_id {
                log::debug!("ignoring self connection {peer_id}");
                Ok(())
            } else if let Some(peer) = peers.lock().await.get(&peer_id) {
                let connection_id = make_id();

                let (accept_tx, accept_rx) = tokio::sync::oneshot::channel();
                let (reject_tx, reject_rx) = tokio::sync::oneshot::channel();

                let a_peers = peers.clone();

                tokio::spawn(async move {
                    let peers = a_peers;
                    if let Ok(_) = accept_rx.await {
                        let peers = peers.lock().await;
                        let requester = peers.get(&our_peer_id);
                        let requestee = peers.get(&peer_id);

                        if requester.is_none() || requestee.is_none() {
                            log::debug!("while accepting connection, peer disapeared {our_peer_id} {peer_id}");
                        } else {
                            log::debug!("accepting connection {connection_id} {peer_id}");
                            requester
                                .unwrap()
                                .send(ServerToPeerMessage {
                                    job_id: 0,
                                    inner: ServerToPeer::ConnectionAccepted(peer_id, connection_id),
                                })
                                .await
                                .unwrap();
                            // requestee
                            //     .unwrap()
                            //     .send(ServerToPeerMessage {
                            //         job_id: 0,
                            //         inner: ServerToPeer::ConnectionAccepted(peer_id, connection_id),
                            //     })
                            //     .await
                            //     .unwrap();
                        }
                    }
                });

                let r_peers = peers.clone();

                tokio::spawn(async move {
                    let peers = r_peers;
                    if let Ok(_) = reject_rx.await {
                        todo!();
                    }
                });

                connection_requests.lock().await.insert(
                    connection_id,
                    ConnectionRequest {
                        accept: accept_tx,
                        reject: reject_tx,
                        requester: our_peer_id,
                        requestee: peer_id,
                    },
                );

                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: message.job_id,
                        inner: ServerToPeer::ConnectionRequest(our_peer_id, connection_id),
                    })
                    .await?)
            } else {
                todo!("handle no such peer")
            }
        }
        PeerToServer::AcceptConnection(connection_id) => {
            let mut connection_requests = connection_requests.lock().await;
            if let Some(ConnectionRequest { requestee, .. }) =
                connection_requests.get(&connection_id)
            {
                if requestee != &our_peer_id {
                    log::debug!("ignoring acecpt connection from different requestee");
                    todo!();
                }
            } else {
                log::debug!("ignoring accept connection for unknown connection id");
                todo!();
            }

            let connection_request = connection_requests.remove(&connection_id).unwrap();

            Ok(connection_request
                .accept
                .send(())
                .map_err(|err| eyre!("Unable to accept connection"))?)
        }
    }
}

async fn handle_incoming_message(
    our_peer_id: PeerId,
    connection_requests: ConnectionRequestMap,
    peers: PeerMap,
    msg: Message,
) -> Result<()> {
    match msg {
        Text(text_message) => {
            let message = serde_json::from_str::<PeerToServerMessage>(&text_message)?;
            Ok(
                handle_incoming_message_inner(our_peer_id, connection_requests, peers, message)
                    .await?,
            )
        }
        Binary(_) | Frame(_) => Err(eyre!("No idea what to do with binary")),
        Close(_) => Err(eyre!("Going down")),
        Ping(data) | Pong(data) => todo!(),
    }
}

async fn handle_outgoing(
    outgoing: &mut SplitSink<WebSocketStream<TcpStream>, Message>,
    msg: ServerToPeerMessage,
) -> Result<()> {
    let text = serde_json::to_string(&msg)?;
    let message = tokio_tungstenite::tungstenite::Message::text(text);
    outgoing.send(message).await?;

    eyre::Ok(())
}

pub(crate) async fn server(address: &str) -> Result<()> {
    let peers: PeerMap = Default::default();
    let connection_requests: ConnectionRequestMap = Default::default();
    let listener = tokio::net::TcpListener::bind(address).await?;

    while let Ok((conn, addr)) = listener.accept().await {
        let peers = peers.clone();
        let connection_requests = connection_requests.clone();
        tokio::spawn(async move {
            println!("Incoming TCP connection from: {}", addr);

            let ws_stream = tokio_tungstenite::accept_async(conn)
                .await
                .expect("Error during the websocket handshake occurred");

            let peer_id = make_id();

            println!("WebSocket connection established: {} {}", peer_id, addr);

            let (mut outgoing, mut incoming) = ws_stream.split();
            let (tx, mut rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

            peers.lock().await.insert(peer_id, tx.clone());

            tx.send(ServerToPeerMessage {
                job_id: 0,
                inner: ServerToPeer::Id(peer_id),
            })
            .await
            .unwrap();

            loop {
                let peers = peers.clone();
                let connection_requests = connection_requests.clone();
                match futures::select! {
                    msg = incoming.next().fuse() => {
                        match msg {
                            Some(Ok(msg)) => {
                                handle_incoming_message(peer_id, connection_requests, peers, msg).await
                            }
                            None | Some(Err(_)) => break,
                        }
                    },
                    msg = rx.recv().fuse() => {
                        match msg {
                            Some(msg) => {
                                handle_outgoing(&mut outgoing, msg).await
                            }
                            None => break,
                        }
                    }
                } {
                    Ok(_) => {}
                    Err(_) => break,
                }
            }

            println!("{} {} disconnected", peer_id, &addr);
            peers.lock().await.remove(&peer_id);
        });
    }

    Ok(())
}

#[derive(Debug)]
pub(crate) enum SignallingControl {
    IceCandidate(PeerId, String),
    Offer(PeerId, String),
    Answer(PeerId, String),
    RequestConnection(PeerId),
    AcceptConnection(ConnectionId),
    RejectConnection(ConnectionId),
}

#[derive(Debug)]
pub(crate) enum SignallingEvent {
    Id(PeerId),
    ConectionRequest(PeerId, ConnectionId),
    Offer(PeerId, String),
    Answer(PeerId, String),
    IceCandidate(PeerId, String),
    ConnectionAccepted(PeerId, ConnectionId),
    Error(SignallingError),
}

async fn send_message(
    write: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    message: PeerToServerMessage,
) -> Result<()> {
    let string = serde_json::to_string(&message)?;
    write.send(Message::text(string)).await?;
    eyre::Ok(())
}

async fn handle_control(
    write: &mut SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
    control: SignallingControl,
) -> Result<()> {
    log::debug!("sending to server {control:?}");

    let message = match control {
        SignallingControl::IceCandidate(peer_id, candidate) => PeerToServerMessage {
            job_id: 0,
            inner: PeerToServer::IceCandidate(peer_id, candidate),
        },
        SignallingControl::Offer(peer_id, offer) => PeerToServerMessage {
            job_id: 0,
            inner: PeerToServer::Offer(peer_id, offer),
        },
        SignallingControl::Answer(peer_id, answer) => PeerToServerMessage {
            job_id: 0,
            inner: PeerToServer::Answer(peer_id, answer),
        },
        SignallingControl::RequestConnection(peer_id) => PeerToServerMessage {
            job_id: 0,
            inner: PeerToServer::ConnectToPeer(peer_id),
        },
        SignallingControl::AcceptConnection(connection_id) => PeerToServerMessage {
            job_id: 0,
            inner: PeerToServer::AcceptConnection(connection_id),
        },
        SignallingControl::RejectConnection(connection_id) => todo!(),
    };

    Ok(send_message(write, message).await?)
}

async fn handle_message(event_tx: mpsc::Sender<SignallingEvent>, msg: Message) -> Result<()> {
    match msg {
        Text(text) => {
            let message = serde_json::from_str::<ServerToPeerMessage>(&text)?;
            log::debug!("received {message:?}");
            let job_id = message.job_id;
            Ok(event_tx
                .send(match message.inner {
                    ServerToPeer::Id(peer_id) => SignallingEvent::Id(peer_id),
                    ServerToPeer::ConnectionRequest(peer_id, connection_id) => {
                        SignallingEvent::ConectionRequest(peer_id, connection_id)
                    }
                    ServerToPeer::ConnectionAccepted(peer_id, connection_id) => {
                        SignallingEvent::ConnectionAccepted(peer_id, connection_id)
                    }
                    ServerToPeer::Offer(peer_id, offer) => SignallingEvent::Offer(peer_id, offer),
                    ServerToPeer::Answer(peer_id, answer) => {
                        SignallingEvent::Answer(peer_id, answer)
                    }
                    ServerToPeer::IceCandidate(peer_id, ice_candidate) => {
                        SignallingEvent::IceCandidate(peer_id, ice_candidate)
                    }
                    ServerToPeer::Error(error) => SignallingEvent::Error(error),
                })
                .await?)
        }
        Binary(_) | Frame(_) => Err(eyre!("No idea what to do with binary")),
        Close(_) => Err(eyre!("Going down")),
        Ping(data) | Pong(data) => todo!(),
    }
}

pub(crate) async fn client(
    address: &str,
) -> Result<(
    mpsc::Sender<SignallingControl>,
    mpsc::Receiver<SignallingEvent>,
)> {
    log::debug!("starting client");
    let (ws_stream, _) = tokio_tungstenite::connect_async(address)
        .await
        .expect("Error during the websocket handshake occurred");

    let (mut write, mut read) = ws_stream.split();

    let (control_tx, mut control_rx) = mpsc::channel::<SignallingControl>(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel::<SignallingEvent>(ARBITRARY_CHANNEL_LIMIT);

    log::debug!("client connected");

    tokio::spawn(async move {
        loop {
            match futures::select! {
                control = control_rx.recv().fuse() => {
                    match control {
                        None => break,
                        Some(control) => handle_control(&mut write, control).await,
                    }
                }
                msg = read.next().fuse() => {
                    match msg {
                        Some(Ok(msg)) => handle_message(event_tx.clone(), msg).await,
                        None | Some(Err(_)) => break,
                    }
                }
            } {
                Ok(ok) => {}
                Err(err) => break,
            }
        }
    });

    Ok((control_tx, event_rx))
}
