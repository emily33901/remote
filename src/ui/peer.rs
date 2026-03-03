use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use anyhow::Result;
use derive_more::{Deref, DerefMut};
use tokio::sync::{mpsc, oneshot, Mutex, MutexGuard};

use crate::config::Config;
use crate::logic::{Mode, PeerStreamRequest, PeerStreamRequestResponse};
use crate::peer::{PeerControl, PeerError, PeerEvent};

use media::{
    Encoding, EncodingOptions, H264EncodingOptions, Statistics, Timestamp, VideoBuffer,
};

use signal::{ConnectionId, PeerId, SignallingControl, SignallingEvent};

use super::app::AppEvent;
use super::color;
use tracing::Instrument;

pub struct RemotePeer {
    peer_id: PeerId,
    control: mpsc::Sender<PeerControl>,
    media_control: Arc<Mutex<Option<mpsc::Sender<media::produce::MediaControl>>>>,
}

impl std::fmt::Debug for RemotePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePeer")
            .field("peer_id", &self.peer_id)
            .finish()
    }
}

impl RemotePeer {
    #[tracing::instrument(skip(app_event_tx, signalling_control))]
    async fn connected(
        controlling: bool,
        signalling_control: mpsc::Sender<SignallingControl>,
        app_event_tx: mpsc::Sender<AppEvent>,
        our_peer_id: PeerId,
        their_peer_id: PeerId,
    ) -> Result<Self> {
        let config = Config::load();

        let (control, event) = crate::peer::peer(
            config.webrtc_api,
            our_peer_id.clone(),
            their_peer_id.clone(),
            signalling_control.clone(),
            controlling,
        )
        .await?;

        let media_control: Arc<Mutex<Option<mpsc::Sender<media::produce::MediaControl>>>> =
            Default::default();

        tokio::spawn({
            let our_peer_id = our_peer_id.clone();
            let their_peer_id = their_peer_id.clone();
            let peer_control = control.downgrade();
            let media_control = Arc::downgrade(&media_control);

            Self::peer_event(
                event,
                peer_control,
                media_control,
                app_event_tx,
                our_peer_id,
                their_peer_id,
            )
            .in_current_span()
        });

        Ok(Self {
            peer_id: their_peer_id.clone(),
            media_control,
            control,
        })
    }

    #[tracing::instrument(skip(event, peer_control, media_control, app_event_tx))]
    async fn peer_event(
        mut event: mpsc::Receiver<PeerEvent>,
        peer_control: mpsc::WeakSender<PeerControl>,
        media_control: Weak<Mutex<Option<mpsc::Sender<media::produce::MediaControl>>>>,
        app_event_tx: mpsc::Sender<AppEvent>,
        our_peer_id: PeerId,
        their_peer_id: PeerId,
    ) -> Result<()> {
        let config = Config::load();
        let mut decoder_control: Option<mpsc::Sender<media::decoder::DecoderControl>> = None;

        while let Some(event) = event.recv().await {
            match event {
                PeerEvent::StreamRequest(request) => {
                    let result = Self::handle_stream_request(
                        &media_control,
                        &peer_control,
                        &app_event_tx,
                        &our_peer_id,
                        &their_peer_id,
                        request,
                    )
                    .await;
                    if let Err(e) = result {
                        tracing::warn!("Failed to handle stream request: {}", e);
                    }
                }
                PeerEvent::RequestStreamResponse(response) => {
                    let result = Self::handle_stream_response(
                        &config,
                        &app_event_tx,
                        &our_peer_id,
                        &their_peer_id,
                        response,
                    )
                    .await;
                    if let Some(control) = result? {
                        decoder_control = Some(control);
                    }
                }
                PeerEvent::Video(video) => {
                    Self::handle_video(
                        &decoder_control,
                        &app_event_tx,
                        &our_peer_id,
                        &their_peer_id,
                        video,
                    )
                    .await?;
                }
                PeerEvent::Error(PeerError::Closed) => {
                    tracing::info!(%our_peer_id, %their_peer_id, "peer closed");
                    app_event_tx
                        .send(AppEvent::PeerClosed(
                            our_peer_id.clone(),
                            their_peer_id.clone(),
                        ))
                        .await?;
                }
                event => {
                    tracing::warn!(%our_peer_id, ?event, "ignoring peer event");
                }
            }
        }

        Ok(())
    }

    async fn handle_stream_request(
        media_control: &Weak<Mutex<Option<mpsc::Sender<media::produce::MediaControl>>>>,
        peer_control: &mpsc::WeakSender<PeerControl>,
        app_event_tx: &mpsc::Sender<AppEvent>,
        our_peer_id: &PeerId,
        their_peer_id: &PeerId,
        request: PeerStreamRequest,
    ) -> Result<()> {
        if let Some(media_control) = media_control.upgrade() {
            if media_control.lock().await.is_some() {
                tracing::warn!(
                    "ignoring request to stream as we already have media control associated with this peer"
                );
                return Ok(());
            }
        }

        let response_tx = Self::stream_request(peer_control, media_control);

        app_event_tx
            .send(AppEvent::RemotePeerStreamRequest(
                our_peer_id.clone(),
                (their_peer_id.clone(), request, response_tx),
            ))
            .await?;

        Ok(())
    }

    async fn handle_stream_response(
        config: &Config,
        app_event_tx: &mpsc::Sender<AppEvent>,
        our_peer_id: &PeerId,
        their_peer_id: &PeerId,
        response: PeerStreamRequestResponse,
    ) -> Result<Option<mpsc::Sender<media::decoder::DecoderControl>>> {
        match response {
            PeerStreamRequestResponse::Accept {
                mode,
                encoding,
                encoding_options,
            } => {
                tracing::info!(?mode, ?encoding, ?encoding_options, "stream accepted");

                let (control, event) = config
                    .decoder_api
                    .run(mode.width, mode.height, mode.refresh_rate)
                    .await?;

                app_event_tx
                    .send(AppEvent::DecoderEvent(
                        our_peer_id.clone(),
                        (their_peer_id.clone(), event),
                    ))
                    .await?;

                Ok(Some(control))
            }
            _ => {
                tracing::warn!(
                    ?response,
                    %our_peer_id,
                    %their_peer_id,
                    "ignoring peer stream request response Reject or Negotiate"
                );
                Ok(None)
            }
        }
    }

    async fn handle_video(
        decoder_control: &Option<mpsc::Sender<media::decoder::DecoderControl>>,
        app_event_tx: &mpsc::Sender<AppEvent>,
        our_peer_id: &PeerId,
        their_peer_id: &PeerId,
        video: media::VideoBuffer,
    ) -> Result<()> {
        app_event_tx
            .send(AppEvent::VideoData(
                our_peer_id.clone(),
                their_peer_id.clone(),
                video.data.clone(),
            ))
            .await?;

        let Some(decoder_control) = decoder_control else {
            tracing::warn!(%our_peer_id, %their_peer_id, "video without decoder control");
            return Ok(());
        };

        decoder_control
            .send(media::decoder::DecoderControl::Data(video))
            .await?;

        Ok(())
    }

    fn stream_request(
        peer_control: &mpsc::WeakSender<PeerControl>,
        media_control: &Weak<Mutex<Option<mpsc::Sender<media::produce::MediaControl>>>>,
    ) -> tokio::sync::oneshot::Sender<(PeerStreamRequestResponse, Option<media::encoder::Encoder>)>
    {
        let (response_tx, response_rx) =
            oneshot::channel::<(PeerStreamRequestResponse, Option<media::encoder::Encoder>)>();

        let peer_control = peer_control.clone();
        let media_control = media_control.clone();

        tokio::spawn(async move {
            let response = response_rx.await;

            if let Some(peer_control) = peer_control.upgrade() {
                match response {
                    Ok((response, encoder)) => {
                        if let PeerStreamRequestResponse::Accept {
                            mode,
                            encoding,
                            encoding_options,
                        } = &response
                        {
                            let config = Config::load();

                            let media_sender_rx = Self::start_streaming(
                                &peer_control.downgrade(),
                                config.media_filename.as_deref(),
                                mode,
                                encoder.clone().unwrap_or(config.encoder_api),
                                encoding.clone(),
                                encoding_options.clone(),
                            );

                            tokio::spawn({
                                async move {
                                    if let Ok(media_sender) = media_sender_rx.await {
                                        if let Some(media_control) = media_control.upgrade() {
                                            *media_control.lock().await = Some(media_sender);
                                        }
                                    }
                                }
                                .in_current_span()
                            });
                        }

                        let _ = peer_control
                            .send(PeerControl::RequestStreamResponse(response))
                            .await;
                    }
                    Err(_) => {
                        let _ = peer_control
                            .send(PeerControl::RequestStreamResponse(
                                PeerStreamRequestResponse::Reject,
                            ))
                            .await;
                    }
                }
            }
        });

        response_tx
    }

    #[tracing::instrument]
    fn start_streaming(
        peer_control: &mpsc::WeakSender<PeerControl>,
        media_filename: Option<&str>,
        mode: &Mode,
        encoder: media::encoder::Encoder,
        encoding: Encoding,
        encoding_options: EncodingOptions,
    ) -> oneshot::Receiver<mpsc::Sender<media::produce::MediaControl>> {
        let (sender_tx, sender_rx) = oneshot::channel();

        tokio::spawn({
            let weak_control = peer_control.clone();
            let mode = mode.clone();
            let media_filename = media_filename.map(|s| s.to_owned());
            async move {
                let _config = Config::load();

                let (tx, mut rx) = if let Some(file) = media_filename.as_ref() {
                    media::produce::produce(
                        encoder,
                        encoding,
                        encoding_options,
                        file,
                        mode.width,
                        mode.height,
                        mode.refresh_rate,
                    )
                    .await?
                } else {
                    media::desktop_duplication::duplicate_desktop(
                        encoder,
                        encoding,
                        encoding_options,
                        mode.width,
                        mode.height,
                        mode.refresh_rate,
                    )
                    .await?
                };

                let _ = sender_tx.send(tx);

                while let Some(event) = rx.recv().await {
                    if let Some(control) = weak_control.upgrade() {
                        match event {
                            media::produce::MediaEvent::Audio(audio) => {
                                tracing::trace!("produce audio {}", audio.len());
                                control.send(PeerControl::Audio(audio)).await.unwrap();
                            }
                            media::produce::MediaEvent::Video(video) => {
                                tracing::trace!("produce video {}", video.data.len());
                                control.send(PeerControl::Video(video)).await.unwrap();
                            }
                        }
                    } else {
                        break;
                    }
                }

                anyhow::Ok(())
            }
            .in_current_span()
        });

        sender_rx
    }
}

pub struct _Peer {
    our_peer_id: PeerId,
    last_connection_request: Option<String>,
    remote_peers: HashMap<PeerId, RemotePeer>,
    connection_peer_id: HashMap<ConnectionId, PeerId>,
    signal_control: mpsc::Sender<SignallingControl>,
    app_event_tx: mpsc::Sender<AppEvent>,
    peer_tasks: tokio::task::JoinSet<Result<()>>,
}

impl std::fmt::Debug for _Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("_Peer")
            .field("our_peer_id", &self.our_peer_id)
            .field("last_connection_request", &self.last_connection_request)
            .field("remote_peers", &self.remote_peers)
            .field("connection_peer_id", &self.connection_peer_id)
            .field("peer_tasks", &self.peer_tasks)
            .finish()
    }
}

impl _Peer {}

#[derive(Clone, Deref, DerefMut, Debug)]
pub struct UIPeer(
    PeerId,
    #[deref]
    #[deref_mut]
    Arc<Mutex<_Peer>>,
);

impl UIPeer {
    pub fn our_id(&self) -> &PeerId {
        &self.0
    }

    /// Create a new peer, blocking waiting for a connection to the signalling server and the id of this peer.
    #[tracing::instrument(skip(app_event_tx))]
    pub async fn new<S: AsRef<str> + std::fmt::Debug>(
        signalling_address: S,
        app_event_tx: mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let (control, mut event_rx) = signal::client(signalling_address.as_ref()).await?;

        let (our_peer_id, event_rx) = async {
            while let Some(event) = event_rx.recv().await {
                match event {
                    SignallingEvent::Id(id) => {
                        return (id, event_rx);
                    }
                    event => {
                        tracing::warn!(?event, "throwing away event when waiting for id");
                    }
                }
            }

            unreachable!("signalling server should always send peer id");
        }
        .await;

        let zelf = Self(
            our_peer_id.clone(),
            Arc::new(Mutex::new(_Peer {
                our_peer_id: our_peer_id,
                last_connection_request: None,
                remote_peers: Default::default(),
                connection_peer_id: Default::default(),
                signal_control: control.clone(),
                app_event_tx: app_event_tx,
                peer_tasks: Default::default(),
            })),
        );

        zelf.inner().await.peer_tasks.spawn({
            let weak_self = zelf.weak();
            async move { Self::signalling(weak_self, event_rx, control.clone()).await }
                .in_current_span()
        });

        Ok(zelf)
    }

    fn weak(&self) -> Weak<Mutex<_Peer>> {
        Arc::downgrade(&self.1)
    }

    async fn inner(&self) -> tokio::sync::MappedMutexGuard<'_, _Peer> {
        MutexGuard::map(self.lock().await, |peer| peer)
    }

    #[tracing::instrument(skip(signal_rx, signal_tx))]
    async fn signalling(
        zelf: Weak<Mutex<_Peer>>,
        mut signal_rx: mpsc::Receiver<SignallingEvent>,
        signal_tx: mpsc::Sender<SignallingControl>,
    ) -> Result<()> {
        while let Some(event) = signal_rx.recv().await {
            let strong_zelf = zelf.upgrade().ok_or(anyhow::anyhow!("no peer"))?;
            let mut locked_peer = strong_zelf.lock().await;

            let span = tracing::debug_span!("SignallingEvent", %locked_peer.our_peer_id);
            let _guard = span.enter();

            match event {
                SignallingEvent::Id(_id) => {
                    unreachable!("We should only ever get our peer_id once");
                }
                SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                    Self::handle_connection_request(&mut locked_peer, peer_id, connection_id).await;
                }
                SignallingEvent::Offer(peer_id, offer) => {
                    Self::handle_peer_message(
                        &mut locked_peer,
                        &peer_id,
                        PeerControl::Offer(offer),
                        "offer",
                    )
                    .await;
                }
                SignallingEvent::Answer(peer_id, answer) => {
                    Self::handle_peer_message(
                        &mut locked_peer,
                        &peer_id,
                        PeerControl::Answer(answer),
                        "answer",
                    )
                    .await;
                }
                SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
                    Self::handle_peer_message(
                        &mut locked_peer,
                        &peer_id,
                        PeerControl::IceCandidate(ice_candidate),
                        "ice candidate",
                    )
                    .await;
                }
                SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                    Self::handle_connection_accepted(
                        &mut locked_peer,
                        &signal_tx,
                        &strong_zelf,
                        peer_id,
                        connection_id,
                    );
                }
                SignallingEvent::Error(error) => {
                    tracing::info!("signalling error {error:?}");
                }
            }
        }

        tracing::info!("client going down");

        Ok(())
    }

    async fn handle_connection_request(
        zelf: &mut _Peer,
        peer_id: PeerId,
        connection_id: ConnectionId,
    ) {
        tracing::info!(%peer_id, ?connection_id, "connection request");
        zelf.last_connection_request = Some(connection_id.to_string());
        zelf.connection_peer_id
            .insert(connection_id.clone(), peer_id.clone());

        let _ = zelf
            .app_event_tx
            .send(AppEvent::ConnectionRequest(
                zelf.our_peer_id.clone(),
                (connection_id, peer_id),
            ))
            .await;
    }

    async fn handle_peer_message(
        zelf: &mut _Peer,
        peer_id: &PeerId,
        control: PeerControl,
        message_type: &str,
    ) {
        tracing::info!(%peer_id, "{}", message_type);

        let Some(remote_peer) = zelf.remote_peers.get(peer_id) else {
            tracing::debug!(%peer_id, ?zelf.remote_peers, "got {} for unknown peer", message_type);
            return;
        };

        let _ = remote_peer.control.send(control).await;
    }

    fn handle_connection_accepted(
        zelf: &mut _Peer,
        signal_tx: &mpsc::Sender<SignallingControl>,
        strong_zelf: &Arc<Mutex<_Peer>>,
        peer_id: PeerId,
        _connection_id: ConnectionId,
    ) {
        let our_peer_id = zelf.our_peer_id.clone();
        assert!(peer_id != our_peer_id);

        tracing::info!(%peer_id, "connection accepted");

        zelf.peer_tasks.spawn({
            let our_peer_id = our_peer_id;
            let their_peer_id = peer_id;
            let tx = signal_tx.clone();
            let zelf = Arc::downgrade(strong_zelf);

            async move {
                let Some(zelf) = zelf.upgrade() else {
                    return anyhow::Ok(());
                };
                let mut locked = zelf.lock().await;

                let remote_peer = RemotePeer::connected(
                    true,
                    tx,
                    locked.app_event_tx.clone(),
                    our_peer_id,
                    their_peer_id.clone(),
                )
                .await?;

                let control = remote_peer.control.clone();
                locked
                    .remote_peers
                    .insert(their_peer_id.clone(), remote_peer);

                let _ = locked
                    .app_event_tx
                    .send(AppEvent::RemotePeerConnected(
                        locked.our_peer_id.clone(),
                        (their_peer_id, control),
                    ))
                    .await;

                anyhow::Ok(())
            }
        });
    }

    async fn connection_requests(&self) -> Result<HashMap<ConnectionId, PeerId>> {
        let zelf = self.inner().await;
        Ok(zelf.connection_peer_id.clone())
    }

    async fn connect(&self, peer_id: PeerId) -> Result<()> {
        let zelf = self.inner().await;

        Ok(zelf
            .signal_control
            .send(SignallingControl::RequestConnection(peer_id))
            .await?)
    }

    async fn accept_connection(&self, peer_id: &PeerId) -> Result<()> {
        let connection_id = (async move {
            let zelf = self.inner().await;
            for (connection_request_id, id) in zelf.connection_peer_id.iter() {
                if peer_id == id {
                    return Some(connection_request_id.clone());
                }
            }
            None
        })
        .await;

        let mut zelf = self.inner().await;

        if let Some(connection_id) = connection_id {
            let their_peer_id = zelf.connection_peer_id.remove(&connection_id).unwrap();

            let peer = RemotePeer::connected(
                false,
                zelf.signal_control.clone(),
                zelf.app_event_tx.clone(),
                zelf.our_peer_id.clone(),
                their_peer_id.clone(),
            )
            .await?;

            let control = peer.control.clone();

            zelf.remote_peers.insert(their_peer_id.clone(), peer);

            zelf.app_event_tx
                .send(AppEvent::RemotePeerConnected(
                    zelf.our_peer_id.clone(),
                    (their_peer_id, control),
                ))
                .await
                .unwrap();

            Ok(zelf
                .signal_control
                .send(SignallingControl::AcceptConnection(connection_id.clone()))
                .await?)
        } else {
            tracing::warn!(
                %zelf.our_peer_id,
                %peer_id,
                ?zelf.connection_peer_id,
                "no incoming connection request from peer"
            );
            Err(anyhow::anyhow!("no incoming connection request from peer"))
        }
    }

    async fn request_stream(
        &self,
        peer_id: PeerId,
        peer_stream_request: PeerStreamRequest,
    ) -> Result<()> {
        let zelf = self.inner().await;
        if let Some(peer) = zelf.remote_peers.get(&peer_id) {
            peer.control
                .send(PeerControl::RequestStream(peer_stream_request))
                .await?;
        }

        Ok(())
    }

    #[tracing::instrument]
    async fn submit_audio(&self, their_peer_id: &PeerId, audio: Vec<u8>) -> Result<()> {
        let zelf = self.inner().await;
        if let Some(peer) = zelf.remote_peers.get(their_peer_id) {
            peer.control.send(PeerControl::Audio(audio)).await?;
        } else {
            tracing::warn!("no such peer");
        }

        Ok(())
    }

    #[tracing::instrument]
    async fn submit_video(&self, their_peer_id: &PeerId, video: VideoBuffer) -> Result<()> {
        let zelf = self.inner().await;
        if let Some(peer) = zelf.remote_peers.get(their_peer_id) {
            peer.control.send(PeerControl::Video(video)).await?;
        } else {
            tracing::warn!("no such peer");
        }

        Ok(())
    }
}

#[derive(PartialEq, Default, Debug)]
pub enum Visible {
    No,
    #[default]
    Yes,
}

#[derive(Clone, Debug)]
pub struct PeerMediaState {
    start_time: Instant,
    start_timestamp: Timestamp,
    time: Timestamp,
    statistics: Statistics,
}

#[derive(Debug)]
pub struct ConnectedPeer {
    pub peer_control: mpsc::Sender<PeerControl>,
    pub peer_media_state: Option<PeerMediaState>,
    pub decoder_receiver: Option<mpsc::Receiver<media::decoder::DecoderEvent>>,
    pub stream_requests: Vec<(
        PeerStreamRequest,
        oneshot::Sender<(PeerStreamRequestResponse, Option<media::encoder::Encoder>)>,
    )>,
    pub statistics_average: VecDeque<Statistics>,
    pub frame_timelines: VecDeque<FrameTimeline>,
    pub latest_h264_data: std::sync::Mutex<Option<Vec<u8>>>,
    pub d3d11_sender: Option<mpsc::Sender<VideoBuffer>>,
}

#[derive(Clone, Debug)]
struct FrameTimeline {
    conversion: Duration,
    encode: Duration,
    network: Duration,
    decode: Duration,
    total: Duration,
    gap: Duration,
}

impl ConnectedPeer {
    pub fn new(peer_control: mpsc::Sender<PeerControl>) -> Self {
        Self {
            peer_control,
            peer_media_state: None,
            decoder_receiver: None,
            stream_requests: vec![],
            statistics_average: VecDeque::new(),
            frame_timelines: VecDeque::new(),
            latest_h264_data: std::sync::Mutex::new(None),
            d3d11_sender: None,
        }
    }
}

#[derive(Default)]
pub struct PeerWindowState {
    pub visible: Visible,
    pub connect_peer_id: String,
    pub connection_requests: HashMap<ConnectionId, PeerId>,
    pub connected_peers: HashMap<PeerId, ConnectedPeer>,
    pub stream_request: PeerStreamRequest,
}

pub enum ShouldRemove {
    Yes,
    No,
}

enum MediaResult {
    Done,
    Empty(Option<PeerMediaState>),
    Texture(PeerMediaState),
}

impl PeerStreamRequest {
    pub fn ui(&mut self, ui: &mut egui::Ui, peer: &UIPeer, their_peer_id: &PeerId) {
        ui.group(|ui| {
            ui.label("start stream");
            ui.end_row();

            ui.separator();
            ui.end_row();

            egui::Grid::new(ui.id()).num_columns(2).show(ui, |ui| {
                let mut changed = false;
                ui.label("encoding");

                ui.horizontal(|ui| {
                    changed = changed
                        || ui
                            .selectable_value(
                                &mut self.preferred_encoding,
                                Some(Encoding::H264),
                                "H264",
                            )
                            .changed();
                    changed = changed
                        || ui
                            .selectable_value(
                                &mut self.preferred_encoding,
                                Some(Encoding::H265),
                                "H265",
                            )
                            .changed();
                    changed = changed
                        || ui
                            .selectable_value(
                                &mut self.preferred_encoding,
                                Some(Encoding::AV1),
                                "AV1",
                            )
                            .changed();
                    changed = changed
                        || ui
                            .selectable_value(
                                &mut self.preferred_encoding,
                                Some(Encoding::VP9),
                                "VP9",
                            )
                            .changed();
                    changed = changed
                        || ui
                            .selectable_value(&mut self.preferred_encoding, None, "Default")
                            .changed();
                });

                ui.end_row();

                if changed {
                    self.preferred_encoding_options = match self.preferred_encoding {
                        Some(Encoding::H264) => Some(EncodingOptions::H264(H264EncodingOptions {
                            rate_control: media::RateControlMode::Quality(70),
                        })),
                        Some(Encoding::AV1) => {
                            Some(EncodingOptions::AV1(media::AV1EncodingOptions {}))
                        }
                        Some(Encoding::H265) => {
                            Some(EncodingOptions::H265(media::H2565EncodingOptions {}))
                        }
                        Some(Encoding::VP9) => {
                            Some(EncodingOptions::VP9(media::VP9EncodingOptions {}))
                        }
                        None => None,
                    }
                }

                match &mut self.preferred_encoding_options {
                    Some(EncodingOptions::H264(encoding_options)) => {
                        ui.label("rate control");
                        ui.horizontal(|ui| {
                            ui.selectable_value(
                                &mut encoding_options.rate_control,
                                media::RateControlMode::Quality(70),
                                "Quality",
                            )
                            .changed();
                            ui.selectable_value(
                                &mut encoding_options.rate_control,
                                media::RateControlMode::Bitrate(8000000),
                                "Bitrate",
                            )
                            .changed();
                        });
                        ui.end_row();

                        ui.label("parameter");

                        match &mut encoding_options.rate_control {
                            media::RateControlMode::Bitrate(bitrate) => {
                                ui.add(
                                    egui::Slider::new(bitrate, 10000..=80000000).logarithmic(true),
                                );
                            }
                            media::RateControlMode::Quality(quality) => {
                                ui.add(egui::Slider::new(quality, 0..=100));
                            }
                        }
                        ui.end_row();
                    }
                    Some(_) => {}
                    None => {}
                }

                ui.end_row();
            });

            if ui.button("request").clicked() {
                tokio::spawn({
                    let peer = peer.clone();
                    let peer_id = their_peer_id.clone();
                    let stream_request = self.clone();
                    async move {
                        peer.request_stream(peer_id, stream_request).await.unwrap();
                    }
                });
            }
        });
    }
}

impl PeerWindowState {
    fn stat(
        ui: &mut egui::Ui,
        label: &str,
        len: usize,
        time: Duration,
        time_iter: impl Iterator<Item = Duration>,
    ) {
        let config = Config::load();

        let (total_duration, count) = time_iter.fold(
            (core::time::Duration::from_secs(0), 0_usize),
            |(duration, count), other_duration| (duration + other_duration, count + 1),
        );
        let average_millis = total_duration.as_millis() as f32 / count as f32;

        ui.label(format!("{len:8} {label} queued",));
        ui.end_row();
        ui.label(format!(
            "{:8.2}ms {:8.2}ms avg ({:2.2} frames) {label} time",
            time.as_millis(),
            average_millis,
            time.as_secs_f32() / (1.0 / (config.framerate as f32)),
        ));
        ui.end_row();
    }

    fn show_media_stats(
        ui: &mut egui::Ui,
        media: &PeerMediaState,
        average_statistics: &mut std::collections::VecDeque<media::Statistics>,
    ) {
        let config = Config::load();

        if let Some(encode) = &media.statistics.encode {
            let duration_iter = average_statistics
                .iter()
                .filter_map(|s| s.encode.as_ref().map(|e| e.time));
            Self::stat(
                ui,
                "encoder",
                encode.media_queue_len,
                encode.time,
                duration_iter,
            );
        }

        if let Some(decode) = &media.statistics.decode {
            let duration_iter = average_statistics
                .iter()
                .filter_map(|s| s.decode.as_ref().map(|e| e.time));
            Self::stat(
                ui,
                "decoder",
                decode.media_queue_len,
                decode.time,
                duration_iter,
            );
        }

        if let Some(conversion) = &media.statistics.convert {
            let duration_iter = average_statistics
                .iter()
                .filter_map(|s| s.convert.as_ref().map(|e| e.time));
            Self::stat(
                ui,
                "conversion",
                conversion.media_queue_len,
                conversion.time,
                duration_iter,
            );
        }
    }

    fn draw_timeline_bar(
        ui: &mut egui::Ui,
        x: f32,
        y: f32,
        conv_w: f32,
        enc_w: f32,
        net_w: f32,
        dec_w: f32,
        gap_w: f32,
        bar_height: f32,
    ) {
        if conv_w > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(conv_w, bar_height)),
                0.0,
                egui::Color32::from_rgb(100, 149, 237),
            );
        }
        if enc_w > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(x + conv_w, y), egui::vec2(enc_w, bar_height)),
                0.0,
                egui::Color32::from_rgb(60, 179, 113),
            );
        }
        if net_w > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x + conv_w + enc_w, y),
                    egui::vec2(net_w, bar_height),
                ),
                0.0,
                egui::Color32::from_rgb(255, 215, 0),
            );
        }
        if dec_w > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x + conv_w + enc_w + net_w, y),
                    egui::vec2(dec_w, bar_height),
                ),
                0.0,
                egui::Color32::from_rgb(220, 20, 60),
            );
        }
        if gap_w > 0.0 {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    egui::pos2(x + conv_w + enc_w + net_w + dec_w, y),
                    egui::vec2(gap_w, bar_height),
                ),
                0.0,
                egui::Color32::GRAY,
            );
        }
    }

    fn draw_frame_timelines(
        ui: &mut egui::Ui,
        frame_timelines: &std::collections::VecDeque<FrameTimeline>,
    ) {
        if frame_timelines.is_empty() {
            return;
        }

        ui.label("Frame Timeline");
        ui.end_row();

        let avg_conversion: Duration = frame_timelines
            .iter()
            .map(|t| t.conversion)
            .sum::<Duration>()
            / frame_timelines.len() as u32;
        let avg_encode: Duration = frame_timelines.iter().map(|t| t.encode).sum::<Duration>()
            / frame_timelines.len() as u32;
        let avg_network: Duration = frame_timelines.iter().map(|t| t.network).sum::<Duration>()
            / frame_timelines.len() as u32;
        let avg_decode: Duration = frame_timelines.iter().map(|t| t.decode).sum::<Duration>()
            / frame_timelines.len() as u32;
        let avg_gap: Duration =
            frame_timelines.iter().map(|t| t.gap).sum::<Duration>() / frame_timelines.len() as u32;
        let avg_total: Duration = frame_timelines.iter().map(|t| t.total).sum::<Duration>()
            / frame_timelines.len() as u32;

        let total_f = avg_total.as_secs_f32();
        if total_f > 0.0 {
            let available_width = ui.available_width();
            let bar_height = 15.0;

            let conv_width = (avg_conversion.as_secs_f32() / total_f) * available_width;
            let enc_width = (avg_encode.as_secs_f32() / total_f) * available_width;
            let net_width = (avg_network.as_secs_f32() / total_f) * available_width;
            let dec_width = (avg_decode.as_secs_f32() / total_f) * available_width;
            let gap_width = (avg_gap.as_secs_f32() / total_f) * available_width;

            let cursor = ui.cursor();
            let y = cursor.min.y;

            let x = ui.min_rect().min.x;
            Self::draw_timeline_bar(
                ui, x, y, conv_width, enc_width, net_width, dec_width, gap_width, bar_height,
            );

            ui.allocate_space(egui::vec2(available_width, bar_height));
            ui.end_row();

            ui.label(format!(
                "avg: conv={:.1}ms enc={:.1}ms net={:.1}ms dec={:.1}ms gap={:.1}ms total={:.1}ms",
                avg_conversion.as_secs_f32() * 1000.0,
                avg_encode.as_secs_f32() * 1000.0,
                avg_network.as_secs_f32() * 1000.0,
                avg_decode.as_secs_f32() * 1000.0,
                avg_gap.as_secs_f32() * 1000.0,
                avg_total.as_secs_f32() * 1000.0,
            ));
            ui.end_row();
        }

        for timeline in frame_timelines.iter().take(10) {
            let total_f = timeline.total.as_secs_f32();
            if total_f > 0.0 {
                let available_width = ui.available_width();
                let bar_height = 10.0;

                let conv_w = (timeline.conversion.as_secs_f32() / total_f) * available_width;
                let enc_w = (timeline.encode.as_secs_f32() / total_f) * available_width;
                let net_w = (timeline.network.as_secs_f32() / total_f) * available_width;
                let dec_w = (timeline.decode.as_secs_f32() / total_f) * available_width;
                let gap_w = (timeline.gap.as_secs_f32() / total_f) * available_width;

                let cursor = ui.cursor();
                let y = cursor.min.y;

                let x = ui.min_rect().min.x;
                Self::draw_timeline_bar(ui, x, y, conv_w, enc_w, net_w, dec_w, gap_w, bar_height);

                ui.allocate_space(egui::vec2(available_width, bar_height));
                ui.end_row();
            }
        }
    }

    pub fn window_ui(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        peer: &UIPeer,
        _gl: &glow::Context,
    ) {
        let config = Config::load();

        ui.text_edit_singleline(&mut self.connect_peer_id);
        if ui.button("connect").clicked() {
            tokio::spawn({
                let connect_peer_id: PeerId = self.connect_peer_id.clone().into();
                let peer = peer.clone();
                async move {
                    peer.connect(connect_peer_id.clone()).await.unwrap();
                }
            });
        }
        ui.end_row();

        ui.heading("Connected Peers");
        ui.end_row();

        let stream_request = &mut self.stream_request;

        for (their_peer_id, connected_peer) in &mut self.connected_peers {
            ui.group(|ui| {
                ui.heading(format!("{}", their_peer_id));

                stream_request.ui(ui, peer, their_peer_id);

                ui.end_row();

                let Some(decoder_receiver) = &mut connected_peer.decoder_receiver else {
                    return;
                };

                if ui.button("Open D3D11 Window").clicked() {
                    let config = Config::load();
                    match crate::presenter::create_d3d11_window(
                        config.width,
                        config.height,
                        &format!("Remote - {}", their_peer_id),
                    ) {
                        Ok(sender) => {
                            connected_peer.d3d11_sender = Some(sender);
                            tracing::info!("D3D11 window created");
                        }
                        Err(e) => {
                            tracing::error!("Failed to create D3D11 window: {}", e);
                        }
                    }
                }

                let media = Self::poll_decoder_events(
                    decoder_receiver,
                    &mut connected_peer.peer_media_state,
                );

                match media {
                    MediaResult::Done => {
                        connected_peer.peer_media_state = None;
                        connected_peer.statistics_average.clear();
                    }
                    MediaResult::Empty(None) => {}
                    MediaResult::Empty(Some(media)) | MediaResult::Texture(media) => {
                        connected_peer.peer_media_state = Some(media.clone());
                        Self::display_media_info(
                            ui,
                            &config,
                            &media,
                            &mut connected_peer.statistics_average,
                            &mut connected_peer.frame_timelines,
                        );

                        Self::draw_frame_timelines(ui, &connected_peer.frame_timelines);
                    }
                }
            });
        }

        ui.heading("Connection Requests");
        ui.end_row();

        for (c_id, p_id) in &self.connection_requests {
            ui.label(format!("{}", p_id));
            ui.label(format!("{}", c_id));

            if ui.button("accept").clicked() {
                tokio::spawn({
                    let peer = peer.clone();
                    let p_id = p_id.clone();
                    async move {
                        peer.accept_connection(&p_id).await.unwrap();
                    }
                });
            }
            ui.end_row();
        }

        ui.heading("Stream Requests");
        ui.end_row();

        Self::handle_stream_requests(ui, &mut self.connected_peers);
    }

    fn poll_decoder_events(
        decoder_receiver: &mut mpsc::Receiver<media::decoder::DecoderEvent>,
        last_media: &mut Option<PeerMediaState>,
    ) -> MediaResult {
        let mut media = MediaResult::Empty(last_media.clone());

        loop {
            match decoder_receiver.try_recv() {
                Ok(media::decoder::DecoderEvent::Frame(_new_texture, time, statistics)) => {
                    media = MediaResult::Texture(PeerMediaState {
                        start_time: Instant::now(),
                        start_timestamp: time.clone(),
                        time,
                        statistics,
                    });
                }
                Err(err) => {
                    if let mpsc::error::TryRecvError::Disconnected = err {
                        media = MediaResult::Done;
                    }
                    break;
                }
            }
        }

        media
    }

    fn display_media_info(
        ui: &mut egui::Ui,
        config: &Config,
        media: &PeerMediaState,
        average_statistics: &mut VecDeque<Statistics>,
        frame_timelines: &mut VecDeque<FrameTimeline>,
    ) {
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);

        let start_ts = media.start_timestamp.clone();
        let media_time = media.time.sub(start_ts);
        let time_diff = (media.start_time.elapsed().saturating_sub(media_time))
            .max(media_time.saturating_sub(media.start_time.elapsed()));

        ui.colored_label(
            color::interpolate_color(
                egui::Color32::GREEN,
                egui::Color32::RED,
                ((time_diff.as_secs_f32() * 1000.0) / 50.0).clamp(0.0, 1.0),
            ),
            format!(
                "{:8}ms total (from frame capture to display) ({:8}ms)",
                time_diff.as_millis(),
                media.time.duration().as_millis(),
            ),
        );
        ui.end_row();

        let stats = media.statistics.clone();
        average_statistics.push_back(stats);
        if average_statistics.len() > config.framerate as usize {
            average_statistics.pop_front();
        }

        Self::show_media_stats(ui, media, average_statistics);

        Self::update_frame_timeline(ui, config, &media, time_diff, frame_timelines);
    }

    fn update_frame_timeline(
        ui: &mut egui::Ui,
        config: &Config,
        media: &PeerMediaState,
        time_diff: Duration,
        frame_timelines: &mut VecDeque<FrameTimeline>,
    ) {
        let encode_stats = media.statistics.encode.as_ref();
        let decode_stats = media.statistics.decode.as_ref();
        let convert_stats = media.statistics.convert.as_ref();

        let Some(network_time) = decode_stats
            .and_then(|d| encode_stats.and_then(|e| d.start_time.duration_since(e.end_time).ok()))
        else {
            return;
        };

        ui.label(format!(
            "{:8}ms ({:2.2} frames) network time (decode start - encode end)",
            network_time.as_millis(),
            network_time.as_secs_f32() / (1.0 / (config.framerate as f32)),
        ));
        ui.end_row();

        let conversion_time = convert_stats.map(|c| c.time).unwrap_or_default();
        let encode_time = encode_stats.map(|e| e.time).unwrap_or_default();
        let decode_time = decode_stats.map(|d| d.time).unwrap_or_default();
        let known_sum = conversion_time + encode_time + network_time + decode_time;
        let gap = time_diff.saturating_sub(known_sum);

        let timeline = FrameTimeline {
            conversion: conversion_time,
            encode: encode_time,
            network: network_time,
            decode: decode_time,
            total: time_diff,
            gap,
        };

        frame_timelines.push_front(timeline);
        if frame_timelines.len() > 10 {
            frame_timelines.pop_back();
        }
    }

    fn handle_stream_requests(
        ui: &mut egui::Ui,
        connected_peers: &mut HashMap<PeerId, ConnectedPeer>,
    ) {
        for (
            peer_id,
            ConnectedPeer {
                stream_requests, ..
            },
        ) in connected_peers
        {
            let mut stream_request_clicked = None;

            for (i, (request, _response)) in stream_requests.iter().enumerate() {
                ui.label(format!("{} {:?}", peer_id, request));

                if ui.button("accept").clicked() {
                    let config = Config::load();
                    stream_request_clicked =
                        Some((i, Self::create_accept_response(request, &config)));
                }
                ui.end_row();
            }

            if let Some((i, (response, encoder))) = stream_request_clicked {
                let (_request, response_channel) = stream_requests.remove(i);
                response_channel.send((response, Some(encoder))).unwrap();
            }
        }
    }

    fn create_accept_response(
        request: &PeerStreamRequest,
        config: &Config,
    ) -> (PeerStreamRequestResponse, media::encoder::Encoder) {
        use crate::logic::Mode;

        (
            PeerStreamRequestResponse::Accept {
                mode: request
                    .preferred_mode
                    .as_ref()
                    .map(|m| Mode {
                        width: m.width,
                        height: m.height,
                        refresh_rate: m.refresh_rate,
                    })
                    .unwrap_or(Mode {
                        width: config.width,
                        height: config.height,
                        refresh_rate: config.framerate,
                    }),
                encoding: request.preferred_encoding.clone().unwrap_or(Encoding::H264),
                encoding_options: request.preferred_encoding_options.clone().unwrap_or(
                    EncodingOptions::H264(H264EncodingOptions {
                        rate_control: media::RateControlMode::Quality(70),
                    }),
                ),
            },
            media::encoder::Encoder::MediaFoundation,
        )
    }

    pub fn ui(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        peer: &UIPeer,
        gl: &glow::Context,
    ) -> ShouldRemove {
        let _config = Config::load();

        let mut result = ShouldRemove::No;

        let id = peer.our_id();
        ui.horizontal(|ui| {
            ui.label(format!("{}", id));

            ui.selectable_value(&mut self.visible, Visible::No, "hide");
            ui.selectable_value(&mut self.visible, Visible::Yes, "show");

            if ui.button("disconnect").clicked() {
                result = ShouldRemove::Yes;
            }
        });

        let mut peer_window_open = true;

        egui::Window::new(format!("Peer {id}"))
            .open(&mut peer_window_open)
            .show(ctx, |ui| {
                self.window_ui(ctx, ui, peer, gl);
            });

        if !peer_window_open {
            ShouldRemove::Yes
        } else {
            result
        }
    }
}

impl std::fmt::Debug for PeerWindowState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PeerWindowState")
            .field("visible", &self.visible)
            .field("connect_peer_id", &self.connect_peer_id)
            .field("connection_requests", &self.connection_requests)
            .field("connected_peers", &self.connected_peers)
            .finish()
    }
}
