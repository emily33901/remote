use std::{collections::HashMap, sync::Arc};

use futures::stream::SplitSink;
use futures::SinkExt;
use futures::{FutureExt, StreamExt, TryStreamExt};
use serde::Deserialize;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio_tungstenite::tungstenite::Message::{self, Binary, Close, Frame, Ping, Pong, Text};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use eyre::{eyre, Result};

pub type PeerId = String;
pub type ConnectionId = Uuid;

pub(crate) const ARBITRARY_CHANNEL_LIMIT: usize = 5;

#[derive(Debug, Serialize, Deserialize)]
pub enum SignallingError {
    NoSuchPeer(PeerId),
    InternalError,
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

fn make_id() -> PeerId {
    use rand::Rng;
    use std::iter;

    const LEN: usize = 5;

    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let one_char = || CHARSET[rng.gen_range(0..CHARSET.len())] as char;
    iter::repeat_with(one_char).take(LEN).collect()
}

fn make_connection_id() -> ConnectionId {
    Uuid::new_v4()
}

type PeerMap = Arc<Mutex<HashMap<PeerId, mpsc::Sender<ServerToPeerMessage>>>>;

#[derive(Debug)]
struct ConnectionRequest {
    accept: tokio::sync::oneshot::Sender<()>,
    reject: tokio::sync::oneshot::Sender<()>,
    requester: PeerId,
    requestee: PeerId,
}

type ConnectionRequestMap = Arc<Mutex<HashMap<ConnectionId, ConnectionRequest>>>;

async fn handle_incoming_message_inner(
    our_peer_id: PeerId,
    connection_requests: ConnectionRequestMap,
    peers: PeerMap,
    message: PeerToServerMessage,
) -> core::result::Result<(), SignallingError> {
    log::debug!("{message:?}");
    match message.inner {
        PeerToServer::IceCandidate(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::IceCandidate(our_peer_id, inner),
                    })
                    .await
                    .map_err(|err| SignallingError::InternalError)?)
            } else {
                Err(SignallingError::NoSuchPeer(peer_id))
            }
        }
        PeerToServer::Offer(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::Offer(our_peer_id, inner),
                    })
                    .await
                    .map_err(|err| SignallingError::InternalError)?)
            } else {
                Err(SignallingError::NoSuchPeer(peer_id))
            }
        }
        PeerToServer::Answer(peer_id, inner) => {
            if let Some(peer) = peers.lock().await.get(&peer_id) {
                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: 0,
                        inner: ServerToPeer::Answer(our_peer_id, inner),
                    })
                    .await
                    .map_err(|err| SignallingError::InternalError)?)
            } else {
                Err(SignallingError::NoSuchPeer(peer_id))
            }
        }
        PeerToServer::ConnectToPeer(peer_id) => {
            if peer_id == our_peer_id {
                log::debug!("ignoring self connection {peer_id}");
                Ok(())
            } else if let Some(peer) = peers.lock().await.get(&peer_id) {
                let connection_id = make_connection_id();

                let (accept_tx, accept_rx) = tokio::sync::oneshot::channel();
                let (reject_tx, reject_rx) = tokio::sync::oneshot::channel();

                let a_peers = peers.clone();

                tokio::spawn({
                    let peer_id = peer_id.clone();
                    let our_peer_id = our_peer_id.clone();
                    async move {
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
                                        inner: ServerToPeer::ConnectionAccepted(
                                            peer_id,
                                            connection_id,
                                        ),
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
                    }
                });

                let r_peers = peers.clone();

                tokio::spawn(async move {
                    let _peers = r_peers;
                    if let Ok(_) = reject_rx.await {
                        todo!();
                    }
                });

                connection_requests.lock().await.insert(
                    connection_id,
                    ConnectionRequest {
                        accept: accept_tx,
                        reject: reject_tx,
                        requester: our_peer_id.clone(),
                        requestee: peer_id.clone(),
                    },
                );

                Ok(peer
                    .send(ServerToPeerMessage {
                        job_id: message.job_id,
                        inner: ServerToPeer::ConnectionRequest(our_peer_id, connection_id),
                    })
                    .await
                    .map_err(|err| SignallingError::InternalError)?)
            } else {
                Err(SignallingError::NoSuchPeer(peer_id))
            }
        }
        PeerToServer::AcceptConnection(connection_id) => {
            let mut connection_requests = connection_requests.lock().await;
            if let Some(ConnectionRequest { requestee, .. }) =
                connection_requests.get(&connection_id)
            {
                if requestee != &our_peer_id {
                    log::debug!("ignoring accept connection from different requestee");
                    return Ok(());
                    // todo!();
                }
            } else {
                log::debug!("ignoring accept connection for unknown connection id");
                return Ok(());
            }

            let connection_request = connection_requests.remove(&connection_id).unwrap();

            Ok(connection_request
                .accept
                .send(())
                .map_err(|err| SignallingError::InternalError)?)
        }
    }
}

async fn handle_incoming_message(
    our_peer_id: PeerId,
    connection_requests: ConnectionRequestMap,
    peers: PeerMap,
    msg: Message,
) -> core::result::Result<(), Option<ServerToPeerMessage>> {
    match msg {
        Text(text_message) => {
            let message = serde_json::from_str::<PeerToServerMessage>(&text_message).unwrap();
            let job_id = message.job_id;
            Ok(
                handle_incoming_message_inner(our_peer_id, connection_requests, peers, message)
                    .await
                    .map_err(|err| ServerToPeerMessage {
                        job_id: job_id,
                        inner: ServerToPeer::Error(err),
                    })?,
            )
        }
        Binary(_) | Frame(_) => panic!("No idea what to do with binary"),
        Close(_) => {
            println!("{} i close", our_peer_id);
            Err(None)
        }
        Ping(_data) | Pong(_data) => todo!(),
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

pub async fn server(address: &str) -> Result<()> {
    let peers: PeerMap = Default::default();
    let connection_requests: ConnectionRequestMap = Default::default();
    let listener = tokio::net::TcpListener::bind(address).await?;

    while let Ok((conn, addr)) = listener.accept().await {
        let peers = peers.clone();
        let connection_requests = connection_requests.clone();
        tokio::spawn(async move {
            match async move {
                println!("i {}", addr);

                let ws_stream = tokio_tungstenite::accept_async(conn).await?;

                let peer_id = make_id();

                println!("{} {}", peer_id, addr);

                let (mut outgoing, mut incoming) = ws_stream.split();
                let (tx, mut rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

                peers.lock().await.insert(peer_id.clone(), tx.clone());

                tx.send(ServerToPeerMessage {
                    job_id: 0,
                    inner: ServerToPeer::Id(peer_id.clone()),
                })
                .await
                .unwrap();

                loop {
                    let peers = peers.clone();
                    let connection_requests = connection_requests.clone();
                    futures::select! {
                        msg = incoming.next().fuse() => {
                            match msg {
                                Some(Ok(msg)) => {
                                    match handle_incoming_message(peer_id.clone(), connection_requests, peers, msg).await {
                                        Err(Some(response)) => {
                                            tx.send(response).await?
                                        }
                                        Err(None) => {
                                            println!("{} d err None", peer_id);
                                            break;
                                        }
                                        _ => {}
                                    }
                                }
                                None => {
                                    println!("{} d None", peer_id);
                                    break;
                                }
                                Some(Err(err)) => {
                                    println!("{} d err {err}", peer_id);
                                    break;
                                },
                            }
                        },
                        msg = rx.recv().fuse() => {
                            match msg {
                                Some(msg) => {
                                    handle_outgoing(&mut outgoing, msg).await?
                                }
                                None => {
                                    println!("{} d outgoing None", peer_id);
                                    break;
                                },
                            }
                        }
                    }
                }

                println!("{} {} disconnected", peer_id, &addr);
                peers.lock().await.remove(&peer_id);

                eyre::Ok(())
            }.await {
                Ok(_) => {}
                Err(err) => {}
            }
        });
    }

    Ok(())
}

#[derive(Debug)]
pub enum SignallingControl {
    IceCandidate(PeerId, String),
    Offer(PeerId, String),
    Answer(PeerId, String),
    RequestConnection(PeerId),
    AcceptConnection(ConnectionId),
    RejectConnection(ConnectionId),
}

#[derive(Debug)]
pub enum SignallingEvent {
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
        SignallingControl::RejectConnection(_connection_id) => todo!(),
    };

    Ok(send_message(write, message).await?)
}

async fn handle_message(event_tx: mpsc::Sender<SignallingEvent>, msg: Message) -> Result<()> {
    match msg {
        Text(text) => {
            let message = serde_json::from_str::<ServerToPeerMessage>(&text)?;
            log::debug!("received {message:?}");
            let _job_id = message.job_id;
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
        Ping(_data) | Pong(_data) => todo!(),
    }
}

pub async fn client(
    address: &str,
) -> Result<(
    mpsc::Sender<SignallingControl>,
    mpsc::Receiver<SignallingEvent>,
)> {
    log::info!("starting signal client");
    let (ws_stream, _) = tokio_tungstenite::connect_async(address)
        .await
        .expect("Error during the websocket handshake occurred");

    let (mut write, mut read) = ws_stream.split();

    let (control_tx, mut control_rx) = mpsc::channel::<SignallingControl>(ARBITRARY_CHANNEL_LIMIT);
    let (event_tx, event_rx) = mpsc::channel::<SignallingEvent>(ARBITRARY_CHANNEL_LIMIT);

    log::info!("client signal connected");

    tokio::spawn(async move {
        loop {
            match futures::select! {
                control = control_rx.recv().fuse() => {
                    match control {
                        None => {
                            log::warn!("control_rx None");
                            break;
                        },
                        Some(control) => handle_control(&mut write, control).await,
                    }
                }
                msg = read.next().fuse() => {
                    match msg {
                        Some(Ok(msg)) => handle_message(event_tx.clone(), msg).await,
                        None => {
                            log::warn!("error reading from websocket (None)");
                            break
                        },
                        Some(Err(err)) => {
                            log::warn!("error reading from websocket ({err})");
                            break
                        },
                    }
                }
            } {
                Ok(_ok) => {}
                Err(err) => {
                    log::error!("err {err}");
                    break;
                }
            }
        }
    });

    Ok((control_tx, event_rx))
}
