mod color;

use crate::config::{self, Config};
use crate::logic::{Mode, PeerStreamRequest, PeerStreamRequestResponse};
use crate::player::video::NV12TextureRender;

use core::time;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use derive_more::{Deref, DerefMut};

use egui::Layout;
use media::decoder::DecoderEvent;
use media::encoder::Encoder;
use media::produce::MediaControl;
use media::{
    Encoding, EncodingOptions, H264EncodingOptions, Statistics, Texture, Timestamp, VideoBuffer,
};

use tracing::Instrument;

use crate::peer::{PeerControl, PeerError, PeerEvent};

use eyre::Result;
use media::dx::create_device_and_swapchain;

use signal::{ConnectionId, PeerId};
use signal::{SignallingControl, SignallingEvent};

use tokio::sync::{mpsc, oneshot, Mutex, MutexGuard};

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{ID3D11Device, ID3D11RenderTargetView, ID3D11Texture2D};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoopBuilder;

use winit::raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawWindowHandle};
use winit::window::WindowBuilder;

fn create_render_target_for_swap_chain(
    device: &ID3D11Device,
    swap_chain: &IDXGISwapChain,
) -> Result<ID3D11RenderTargetView> {
    let swap_chain_texture = unsafe { swap_chain.GetBuffer::<ID3D11Texture2D>(0) }?;
    let mut render_target = None;
    unsafe { device.CreateRenderTargetView(&swap_chain_texture, None, Some(&mut render_target)) }?;
    Ok(render_target.unwrap())
}

fn resize_swap_chain_and_render_target(
    device: &ID3D11Device,
    swap_chain: &IDXGISwapChain,
    render_target: &mut Option<ID3D11RenderTargetView>,
    new_width: u32,
    new_height: u32,
    new_format: DXGI_FORMAT,
) -> Result<()> {
    render_target.take();

    unsafe { swap_chain.ResizeBuffers(1, new_width, new_height, new_format, 0) }?;
    render_target.replace(create_render_target_for_swap_chain(device, swap_chain)?);
    Ok(())
}

struct RemotePeer {
    peer_id: PeerId,
    control: mpsc::Sender<PeerControl>,
    media_control: Arc<Mutex<Option<mpsc::Sender<MediaControl>>>>,
}

impl std::fmt::Debug for RemotePeer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemotePeer")
            .field("peer_id", &self.peer_id)
            // .field("control", &self.control)
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
        let config = config::Config::load();

        let (control, event) = crate::peer::peer(
            config.webrtc_api,
            our_peer_id.clone(),
            their_peer_id.clone(),
            signalling_control.clone(),
            controlling,
        )
        .await?;

        let media_control: Arc<Mutex<Option<mpsc::Sender<MediaControl>>>> = Default::default();

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
        media_control: Weak<Mutex<Option<mpsc::Sender<MediaControl>>>>,
        app_event_tx: mpsc::Sender<AppEvent>,
        our_peer_id: PeerId,
        their_peer_id: PeerId,
    ) -> Result<()> {
        let config = Config::load();

        let mut decoder_control: Option<mpsc::Sender<media::decoder::DecoderControl>> = None;

        while let Some(event) = event.recv().await {
            match event {
                PeerEvent::StreamRequest(request) => {
                    // Check that we arent already streaming to this peer
                    if let Some(media_control) = media_control.upgrade() {
                        if media_control.lock().await.is_some() {
                            tracing::warn!(
                            "ignoring request to stream as we already have media control associated with this peer"
                        );
                            continue;
                        }
                    }

                    let response_tx = Self::stream_request(&peer_control, &media_control);

                    app_event_tx
                        .send(AppEvent::RemotePeerStreamRequest(
                            our_peer_id.clone(),
                            (their_peer_id.clone(), request, response_tx),
                        ))
                        .await?;
                }

                PeerEvent::RequestStreamResponse(response) => match response {
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

                        decoder_control = Some(control);

                        app_event_tx
                            .send(AppEvent::DecoderEvent(
                                our_peer_id.clone(),
                                (their_peer_id.clone(), event),
                            ))
                            .await?;
                    }
                    _ => {
                        tracing::warn!(
                            ?response,
                            %our_peer_id,
                            %their_peer_id,
                            "ignoring peer stream request response Reject or Negotiate"
                        );
                    }
                },
                PeerEvent::Video(video) => {
                    if let Some(decoder_control) = &decoder_control {
                        decoder_control
                            .send(media::decoder::DecoderControl::Data(video))
                            .await?;
                    } else {
                        tracing::warn!(
                            "got video when we were not expecting video (we dont have a decoder)"
                        );
                    }
                }
                PeerEvent::Error(PeerError::Closed) => {
                    tracing::info!("peer is done forever");
                    app_event_tx
                        .send(AppEvent::PeerClosed(our_peer_id, their_peer_id))
                        .await
                        .unwrap();
                    break;
                }
                event => {
                    tracing::warn!(%our_peer_id, ?event, "ignoring peer event");
                }
            }
        }
        eyre::Ok(())
    }

    #[tracing::instrument(skip(peer_control, media_control))]
    fn stream_request(
        peer_control: &mpsc::WeakSender<PeerControl>,
        media_control: &Weak<Mutex<Option<mpsc::Sender<MediaControl>>>>,
    ) -> oneshot::Sender<(PeerStreamRequestResponse, Option<Encoder>)> {
        let (response_tx, response_rx): (
            oneshot::Sender<(PeerStreamRequestResponse, Option<Encoder>)>,
            _,
        ) = tokio::sync::oneshot::channel();

        tokio::spawn({
            let peer_control = peer_control.clone();
            let media_control = media_control.clone();
            async move {
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
                                // We accepted the stream
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
                            // Sender was dropped without sending anything
                            // Tell other peer that we are not going to be streaming to them
                            let _ = peer_control
                                .send(PeerControl::RequestStreamResponse(
                                    PeerStreamRequestResponse::Reject,
                                ))
                                .await;
                        }
                    }
                }
            }
            .in_current_span()
        });
        response_tx
    }

    // Start streaming to this remote peer
    #[tracing::instrument]
    fn start_streaming(
        peer_control: &mpsc::WeakSender<PeerControl>,
        media_filename: Option<&str>,
        mode: &Mode,
        encoder: Encoder,
        encoding: Encoding,
        encoding_options: EncodingOptions,
    ) -> oneshot::Receiver<mpsc::Sender<MediaControl>> {
        // TODO(emily): Thinking that here we can return 2 channels, one for video_tx and one for audio_tx.
        // these could even come from higher up in peer instead of just sending to peer-control. This would also avoid
        // the 'bottleneck' from sending everything through peer control.
        // returning channels here would also allow us to pass media through from higher up in the application
        // (maybe from a single desktop duplication instance instead of the one for each peer that we would have right
        // now).

        // For now this is fine.

        let (sender_tx, sender_rx) = oneshot::channel();

        tokio::spawn({
            let weak_control = peer_control.clone();
            let mode = mode.clone();
            let media_filename = media_filename.map(|s| s.to_owned());
            async move {
                let config = Config::load();

                // TODO(emily): Race condition here where tx is being kept alive by rx
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

                // Ignore result here, if RemotePeer doesnt take sender here then we will go down immediately
                // and thats fine, because the only case that it would not take it is if it has gone already.
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

                eyre::Ok(())
            }
            .in_current_span()
        });

        sender_rx
    }
}

struct _Peer {
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
            // .field("signal_control", &self.signal_control)
            // .field("app_event_tx", &self.app_event_tx)
            .field("peer_tasks", &self.peer_tasks)
            .finish()
    }
}

impl _Peer {}

#[derive(Clone, Deref, DerefMut, Debug)]
struct UIPeer(
    PeerId,
    #[deref]
    #[deref_mut]
    Arc<Mutex<_Peer>>,
);

impl UIPeer {
    fn our_id(&self) -> &PeerId {
        &self.0
    }

    /// Create a new peer, blocking waiting for a connection to the signalling server and the id of this peer.
    #[tracing::instrument(skip(app_event_tx))]
    async fn new<S: AsRef<str> + std::fmt::Debug>(
        signalling_address: S,
        app_event_tx: mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let (control, mut event_rx) = signal::client(signalling_address.as_ref()).await?;

        // TODO(emily): Hold onto other events that might turn up here, right now we just THROW them away,
        // not very nice.

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

    async fn inner(&self) -> tokio::sync::MappedMutexGuard<_Peer> {
        MutexGuard::map(self.lock().await, |peer| peer)
    }

    #[tracing::instrument(skip(signal_rx, signal_tx))]
    async fn signalling(
        zelf: Weak<Mutex<_Peer>>,
        mut signal_rx: mpsc::Receiver<SignallingEvent>,
        signal_tx: mpsc::Sender<SignallingControl>,
    ) -> Result<()> {
        while let Some(event) = signal_rx.recv().await {
            let strong_zelf = zelf.upgrade().ok_or(eyre::eyre!("no peer"))?;
            let mut zelf = strong_zelf.lock().await;

            let span = tracing::debug_span!("SignallingEvent", %zelf.our_peer_id);
            let _guard = span.enter();

            match event {
                signal::SignallingEvent::Id(_id) => {
                    unreachable!("We should only ever get our peer_id once");
                }
                signal::SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                    tracing::info!(%peer_id, ?connection_id, "connection request");
                    zelf.last_connection_request = Some(connection_id.to_string());
                    zelf.connection_peer_id
                        .insert(connection_id.clone(), peer_id.clone());

                    // TODO(emily): Throwing app event because we don't care at the moment
                    let _ = zelf
                        .app_event_tx
                        .send(AppEvent::ConnectionRequest(
                            zelf.our_peer_id.clone(),
                            (connection_id, peer_id),
                        ))
                        .await;
                }
                signal::SignallingEvent::Offer(peer_id, offer) => {
                    tracing::info!(%peer_id, offer, "offer");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(peer_data) = remote_peers.get(&peer_id) {
                        peer_data
                            .control
                            .send(PeerControl::Offer(offer))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(%peer_id, ?remote_peers, "got offer for unknown peer");
                    }
                }
                signal::SignallingEvent::Answer(peer_id, answer) => {
                    tracing::info!(%peer_id, answer, "answer");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(remote_peer) = remote_peers.get(&peer_id) {
                        remote_peer
                            .control
                            .send(PeerControl::Answer(answer))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(
                            %peer_id,
                            ?remote_peers,
                            "got answer for unknown peer {peer_id}"
                        );
                    }
                }
                signal::SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
                    tracing::info!(%peer_id, ?ice_candidate, "ice candidate");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(remote_peer) = remote_peers.get(&peer_id) {
                        remote_peer
                            .control
                            .send(PeerControl::IceCandidate(ice_candidate))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(
                            %peer_id,
                            ?remote_peers,
                            "got ice candidate for unknown peer"
                        );
                    }
                }
                signal::SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                    // NOTE(emily): We sent the request so we are controlling
                    let our_peer_id = zelf.our_peer_id.clone();
                    assert!(peer_id != our_peer_id);

                    tracing::info!(%peer_id, ?connection_id, "connection accepted");

                    zelf.peer_tasks.spawn({
                        let our_peer_id = our_peer_id;
                        let their_peer_id = peer_id.clone();
                        let tx = signal_tx.clone();
                        let zelf = Arc::downgrade(&strong_zelf);

                        async move {
                            if let Some(zelf) = zelf.upgrade() {
                                let mut zelf = zelf.lock().await;

                                let remote_peer = RemotePeer::connected(
                                    true,
                                    tx,
                                    zelf.app_event_tx.clone(),
                                    our_peer_id,
                                    their_peer_id.clone(),
                                )
                                .await?;

                                let control = remote_peer.control.clone();

                                zelf.remote_peers.insert(their_peer_id.clone(), remote_peer);

                                zelf.app_event_tx
                                    .send(AppEvent::RemotePeerConnected(
                                        zelf.our_peer_id.clone(),
                                        (their_peer_id, control),
                                    ))
                                    .await
                                    .unwrap();
                            }
                            eyre::Ok(())
                        }
                    });
                }
                signal::SignallingEvent::Error(error) => {
                    tracing::info!("signalling error {error:?}");
                }
            }
        }

        tracing::info!("client going down");

        Ok(())
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
        // TODO(emily): Store a peer-id to connection id relation aswell.
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

        // Make a remote peer for this connection
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
            Err(eyre::eyre!("no incoming connection request from peer"))
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
enum Visible {
    No,
    #[default]
    Yes,
}

#[derive(Clone)]
struct PeerMediaState {
    start_time: Instant,
    start_timestamp: Timestamp,
    time: Timestamp,
    texture: Arc<Texture>,
    statistics: Statistics,
}

#[derive(Default)]
struct PeerWindowState {
    visible: Visible,
    connect_peer_id: String,
    connection_requests: HashMap<ConnectionId, PeerId>,
    connected_peers: HashMap<PeerId, mpsc::Sender<PeerControl>>,
    stream_request: PeerStreamRequest,
    stream_requests: HashMap<
        PeerId,
        (
            PeerStreamRequest,
            oneshot::Sender<(PeerStreamRequestResponse, Option<Encoder>)>,
        ),
    >,

    stream_texture_renderer: Arc<std::sync::OnceLock<NV12TextureRender>>,
    sink: std::sync::OnceLock<mpsc::Sender<(Arc<media::Texture>, Timestamp)>>,
    connected_peer_media: HashMap<
        PeerId,
        (
            Option<PeerMediaState>,
            mpsc::Receiver<media::decoder::DecoderEvent>,
        ),
    >,
    peer_statistics_average: HashMap<PeerId, VecDeque<Statistics>>,
}

enum ShouldRemove {
    Yes,
    No,
}

impl PeerStreamRequest {
    fn ui(&mut self, ui: &mut egui::Ui, peer: &UIPeer, their_peer_id: &PeerId) {
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
    fn window_ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, peer: &UIPeer) {
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

        for (their_peer_id, _control) in &self.connected_peers {
            ui.group(|ui| {
                ui.heading(format!("{}", their_peer_id));

                self.stream_request.ui(ui, peer, their_peer_id);

                ui.end_row();

                enum MediaResult {
                    Done,
                    Empty(Option<PeerMediaState>),
                    Texture(PeerMediaState),
                }

                if let Some((last_media, decoder_event)) =
                    self.connected_peer_media.get_mut(their_peer_id)
                {
                    let mut media = MediaResult::Empty(last_media.clone());

                    loop {
                        match decoder_event.try_recv() {
                            Ok(DecoderEvent::Frame(new_texture, time, statistics)) => {
                                media = MediaResult::Texture(PeerMediaState {
                                    start_time: last_media
                                        .as_ref()
                                        .map(|m|m.start_time)
                                        .unwrap_or(Instant::now()),
                                    start_timestamp: last_media
                                        .as_ref()
                                        .map(|m|m.start_timestamp.clone())
                                        .unwrap_or(time.clone()),
                                    time: time,
                                    texture: Arc::new(new_texture),
                                    statistics: statistics,
                                })
                            }
                            Err(err) => {
                                if let mpsc::error::TryRecvError::Disconnected = err {
                                    media = MediaResult::Done;
                                }
                                break;
                            }
                        }
                    }

                    match media {
                        MediaResult::Done => {
                            self.connected_peer_media.remove(their_peer_id).unwrap();
                        }
                        MediaResult::Empty(None) => {}
                        MediaResult::Empty(Some(media)) | MediaResult::Texture(media) => {
                            *last_media = Some(media.clone());

                            {
                                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);

                                let media_time = media.time.sub(media.start_timestamp);
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

                                fn stat(ui: &mut egui::Ui, label: &str, len: usize, time: Duration, time_iter: impl Iterator<Item = Duration>) {
                                    let config = Config::load();

                                    let (total_duration, count) = time_iter.fold((time::Duration::from_secs(0), 0_usize), |(duration, count), other_duration| { (duration + other_duration, count + 1)});
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


                                let average_statistics = self.peer_statistics_average
                                    .get_mut(their_peer_id);

                                let average_statistics = if let None = average_statistics {
                                    self.peer_statistics_average.insert(their_peer_id.clone(), VecDeque::new());
                                        self.peer_statistics_average.get_mut(their_peer_id).unwrap()
                                } else {
                                    average_statistics.unwrap()
                                };

                                average_statistics.push_back(media.statistics.clone());
                                if average_statistics.len() > config.framerate as usize {
                                    average_statistics.pop_front();
                                }

                                if let Some(encode) = &media.statistics.encode {
                                    let duration_iter = average_statistics
                                        .iter()
                                        .filter_map(|s| s.encode.as_ref().map(|e| e.time));
                                    stat(ui, "encoder", encode.media_queue_len, encode.time, duration_iter);
                                }

                                if let Some(decode) = &media.statistics.decode {
                                    let duration_iter = average_statistics
                                        .iter()
                                        .filter_map(|s| s.decode.as_ref().map(|e| e.time));

                                    stat(
                                        ui,
                                        "decoder",
                                        decode.media_queue_len,
                                        decode.time,
                                        duration_iter
                                    );
                                }

                                if let Some(conversion) = &media.statistics.convert {
                                    let duration_iter = average_statistics
                                        .iter()
                                        .filter_map(|s| s.convert.as_ref().map(|e| e.time));
                                    stat(ui, "conversion", conversion.media_queue_len, conversion.time, duration_iter);
                                }

                                if let Ok(network_time) = media
                                    .statistics
                                    .decode
                                    .unwrap()
                                    .start_time
                                    .duration_since(media.statistics.encode.unwrap().end_time)
                                {
                                    ui.label(format!(
                                        "{:8}ms ({:2.2} frames) network time (decode start - encode end)",
                                        network_time.as_millis(),
                                        network_time.as_secs_f32()
                                            / (1.0 / (config.framerate as f32)),
                                    ));
                                    ui.end_row();
                                }
                            }

                            let config = Config::load();
                            let aspect: f32 = config.height as f32 / config.width as f32;

                            let desired_size = ui.available_width() * egui::vec2(1.0, aspect);
                            let (_id, rect) = ui.allocate_space(desired_size);

                            ctx.request_repaint();
                            let cb = egui::PaintCallback {
                                rect: rect,
                                callback: std::sync::Arc::new(egui_directx11::CallbackFn::new({
                                    let texture_renderer = self.stream_texture_renderer.clone();
                                    move |_info, renderer| {
                                        let texture_renderer = texture_renderer.get_or_init(|| {
                                            NV12TextureRender::new(renderer.device()).unwrap()
                                        });

                                        texture_renderer
                                            .render_texture(&media.texture, renderer.device())
                                            .unwrap();
                                    }
                                })),
                            };
                            ui.painter().add(cb);
                        }
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

        let mut stream_request_clicked = None;

        for (peer_id, (request, _)) in &self.stream_requests {
            ui.label(format!("{} {:?}", peer_id, request));

            if ui.button("accept").clicked() {
                let config = Config::load();

                stream_request_clicked = Some((
                    peer_id.clone(),
                    PeerStreamRequestResponse::Accept {
                        mode: request
                            .preferred_mode
                            .as_ref()
                            .map(|m| crate::logic::Mode {
                                width: m.width,
                                height: m.height,
                                refresh_rate: m.refresh_rate,
                            })
                            .unwrap_or(crate::logic::Mode {
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
                    // TODO(emily): Do not hardcode this value please please please please
                    Encoder::MediaFoundation,
                ));
            }
            ui.end_row();
        }

        if let Some((peer_id, response, encoder)) = stream_request_clicked {
            let (_request, response_channel) = self.stream_requests.remove(&peer_id).unwrap();

            response_channel.send((response, Some(encoder))).unwrap();
        }
    }

    fn ui(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, peer: &UIPeer) -> ShouldRemove {
        let config = Config::load();

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
                self.window_ui(ctx, ui, peer);
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
            .field("stream_requests", &self.stream_requests)
            // .field("stream_texture_renderer", &self.stream_texture_renderer)
            .finish()
    }
}

enum AppEvent {
    Peer(UIPeer),
    ConnectionRequest(PeerId, (ConnectionId, PeerId)),
    RemotePeerConnected(PeerId, (PeerId, mpsc::Sender<PeerControl>)),
    RemotePeerStreamRequest(
        PeerId,
        (
            PeerId,
            PeerStreamRequest,
            tokio::sync::oneshot::Sender<(PeerStreamRequestResponse, Option<Encoder>)>,
        ),
    ),
    DecoderEvent(
        PeerId,
        (PeerId, mpsc::Receiver<media::decoder::DecoderEvent>),
    ),
    PeerClosed(PeerId, PeerId),
}

struct App {
    peers: HashMap<PeerId, (PeerWindowState, UIPeer)>,
    event_rx: mpsc::Receiver<AppEvent>,
    event_tx: mpsc::Sender<AppEvent>,
    start_time: std::time::Instant,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("peers", &self.peers)
            // .field("event_rx", &self.event_rx)
            // .field("event_tx", &self.event_tx)
            .finish()
    }
}

impl Default for App {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::channel(10);

        Self {
            peers: Default::default(),
            event_rx,
            event_tx,
            start_time: Instant::now(),
        }
    }
}

impl App {
    #[tracing::instrument(skip(ctx))]
    fn ui(&mut self, ctx: &egui::Context) {
        let wallclock = egui::Window::new("Clock");

        wallclock.show(ctx, |ui| {
            let elapsed = self.start_time.elapsed();
            ui.label(
                egui::RichText::new(format!(
                    "{:8}:{:04}",
                    elapsed.as_secs(),
                    elapsed.subsec_millis()
                ))
                .font(egui::FontId::monospace(30.0))
                .size(50.0),
            )
        });

        let ui = egui::Window::new("App");

        ui.show(ctx, |ui| {
            if ui.button("new peer").clicked() {
                tokio::spawn({
                    let event_tx = self.event_tx.clone();
                    async move {
                        let config = Config::load();
                        let peer = UIPeer::new(&config.signal_server, event_tx.clone())
                            .await
                            .unwrap();
                        event_tx.send(AppEvent::Peer(peer)).await.unwrap();
                    }
                });
            }

            ui.heading("Peers");

            let mut remove_peers = vec![];

            {
                for (id, (window_state, peer)) in self.peers.iter_mut() {
                    match window_state.ui(ctx, ui, peer) {
                        ShouldRemove::Yes => {
                            remove_peers.push(id.clone());
                        }
                        ShouldRemove::No => {}
                    }
                }
            }

            for peer in remove_peers {
                self.peers.remove(&peer);
            }

            if let Ok(event) = self.event_rx.try_recv() {
                match event {
                    AppEvent::Peer(peer) => {
                        self.peers
                            .insert(peer.our_id().clone(), (PeerWindowState::default(), peer));
                    }
                    AppEvent::ConnectionRequest(our_peer_id, (connection_request_id, peer_id)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_peer_id) {
                            peer_window_state
                                .connection_requests
                                .insert(connection_request_id, peer_id);
                        }
                    }
                    AppEvent::RemotePeerConnected(peer_id, (their_peer_id, control)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&peer_id) {
                            peer_window_state
                                .connected_peers
                                .insert(their_peer_id.clone(), control);
                        }

                        // If we had a connection request from this peer then we can get rid of it
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&peer_id) {
                            if let Some(connection_id) = (|| {
                                for (c_id, p_id) in &peer_window_state.connection_requests {
                                    if p_id == &their_peer_id {
                                        return Some(c_id.clone());
                                    }
                                }

                                None
                            })() {
                                peer_window_state.connection_requests.remove(&connection_id);
                            }
                        }
                    }
                    AppEvent::RemotePeerStreamRequest(our_id, (their_id, request, response_tx)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_id) {
                            peer_window_state
                                .stream_requests
                                .insert(their_id.clone(), (request, response_tx));
                        }
                    }

                    AppEvent::DecoderEvent(our_id, (their_id, decoder_event)) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_id) {
                            peer_window_state
                                .connected_peer_media
                                .insert(their_id, (None, decoder_event));
                        }
                    }
                    AppEvent::PeerClosed(our_id, their_id) => {
                        if let Some((peer_window_state, _)) = self.peers.get_mut(&our_id) {
                            peer_window_state
                                .connected_peers
                                .remove(&their_id)
                                .expect("Expect remote PeerControl to exist when it goes away");

                            peer_window_state.connected_peer_media.remove(&their_id);
                        }
                    }
                }
            }
        });
    }
}

pub async fn ui() -> Result<()> {
    let _system = crate::windows::System::new()?;

    let mut app = App::default();
    let config = Config::load();

    let (width, height) = (config.width, config.height);

    let event_loop = EventLoopBuilder::new().build()?; // .with_any_thread(true).build()?;
    let window = WindowBuilder::new()
        .with_title("remote")
        .with_inner_size(PhysicalSize::new(width, height))
        .build(&event_loop)?;

    let window_handle = if let RawWindowHandle::Win32(raw) = window.window_handle()?.as_raw() {
        HWND(raw.hwnd.get())
    } else {
        panic!("unexpected RawWindowHandle variant");
    };

    let (device, context, swap_chain) = create_device_and_swapchain(window_handle, width, height)?;

    let mut render_target = Some(create_render_target_for_swap_chain(&device, &swap_chain)?);

    let egui_ctx = egui::Context::default();
    let mut egui_renderer = egui_directx11::Renderer::new(&device)?;
    let mut egui_winit = egui_winit::State::new(
        egui_ctx.clone(),
        egui_ctx.viewport_id(),
        &window.display_handle()?,
        None,
        None,
    );

    let mut egui_demo = egui_demo_lib::DemoWindows::default();

    event_loop.run(move |event, _control_flow| match event {
        Event::AboutToWait => window.request_redraw(),
        Event::WindowEvent { window_id, event } => {
            if window_id != window.id() {
                return;
            }

            if egui_winit.on_window_event(&window, &event).consumed {
                return;
            }

            match event {
                WindowEvent::CloseRequested => {
                    std::process::exit(0);
                    // control_flow.exit()
                }
                WindowEvent::Resized(PhysicalSize {
                    width: new_width,
                    height: new_height,
                }) => {
                    if let Err(err) = resize_swap_chain_and_render_target(
                        &device,
                        &swap_chain,
                        &mut render_target,
                        new_width,
                        new_height,
                        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
                    ) {
                        panic!("fail to resize framebuffers: {err:?}");
                    }
                }
                WindowEvent::RedrawRequested => {
                    if let Some(render_target) = &render_target {
                        let egui_input = egui_winit.take_egui_input(&window);
                        let egui_output = egui_ctx.run(egui_input, |ctx| {
                            app.ui(ctx);
                            egui_demo.ui(ctx);
                        });
                        let (renderer_output, platform_output, _) =
                            egui_directx11::split_output(egui_output);
                        egui_winit.handle_platform_output(&window, platform_output);

                        unsafe {
                            context.ClearRenderTargetView(render_target, &[0.0, 0.0, 0.0, 1.0]);
                        }
                        let _ = egui_renderer.render(
                            &context,
                            render_target,
                            &egui_ctx,
                            renderer_output,
                            window.scale_factor() as _,
                        );
                        let _ = unsafe { swap_chain.Present(1, 0) };
                    } else {
                        unreachable!();
                    }
                }
                _ => (),
            }
        }
        _ => (),
    })?;

    // if *produce {
    //     tokio::task::spawn({

    //     });
    // }

    Ok(())
}
