use crate::config::{self, Config};
use std::cell::RefCell;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::{Arc, Weak};
use std::{collections::HashMap, fmt::Display};

use derive_more::{Deref, DerefMut};
use tokio::runtime::Handle;
use windows::Win32::Media::MediaFoundation::OPM_CONFIGURE_PARAMETERS;

use crate::peer::PeerControl;
use clap::Parser;
use eyre::Result;
use media::dx::create_device_and_swapchain;
use rtc;
use signal::{ConnectionId, PeerId};
use signal::{SignallingControl, SignallingEvent};

use tokio::sync::{mpsc, MappedMutexGuard, Mutex, MutexGuard};
use tracing::level_filters::LevelFilter;
use uuid::Uuid;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11RenderTargetView, ID3D11Texture2D,
    D3D11_TRACE_INPUT_GS_INSTANCE_ID_REGISTER,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain;
use winit::dpi::PhysicalSize;
use winit::event::{Event, WindowEvent};
use winit::event_loop::EventLoopBuilder;
use winit::platform::windows::EventLoopBuilderExtWindows;
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

#[derive(Debug)]
struct RemotePeer {
    control: mpsc::Sender<PeerControl>,
    tasks: tokio::task::JoinSet<Result<()>>,
}

impl RemotePeer {
    async fn connected(
        controlling: bool,
        signalling_control: mpsc::Sender<SignallingControl>,
        our_peer_id: PeerId,
        their_peer_id: PeerId,
    ) -> Result<Self> {
        let config = config::Config::load();

        let (control, mut event) = crate::peer::peer(
            config.webrtc_api,
            our_peer_id.clone(),
            their_peer_id.clone(),
            signalling_control.clone(),
            controlling,
        )
        .await?;

        let mut tasks = tokio::task::JoinSet::new();

        tasks.spawn({
            let our_peer_id = our_peer_id.clone();

            async move {
                // NOTE(emily): Make sure to keep player alive
                // let _player = audio_player;

                // let file_sink = media::file_sink::file_sink(
                //     std::path::Path::new(&format!("test-{our_peer_id}.mp4")),
                //     width,
                //     height,
                //     framerate,
                //     bitrate,
                // )
                // .unwrap();

                // let mut i = 0;

                while let Some(event) = event.recv().await {
                    tracing::warn!(our_peer_id, ?event, "ignoring peer event");
                    // match event {
                    //     peer::PeerEvent::Audio(audio) => {
                    //         // tracing::debug!("peer event audio {}", audio.len());
                    //         audio_sink_tx.send(audio).await.unwrap();
                    //         // audio_sink_tx.send(audio).await.unwrap();
                    //     }
                    //     peer::PeerEvent::Video(video) => {
                    //         // tracing::debug!("peer event video {}", video.data.len());
                    //         h264_control
                    //             .send(media::decoder::DecoderControl::Data(video.clone()))
                    //             .await
                    //             .unwrap();

                    //         // match i {
                    //         //     0..=1000 => file_sink
                    //         //         .send(media::file_sink::FileSinkControl::Video(video))
                    //         //         .await
                    //         //         .unwrap(),
                    //         //     1001 => file_sink
                    //         //         .send(media::file_sink::FileSinkControl::Done)
                    //         //         .await
                    //         //         .unwrap(),
                    //         //     _ => {
                    //         //         tracing::info!("!! DONE")
                    //         //     }
                    //         // }
                    //         // i += 1;
                    //     }
                    //     peer::PeerEvent::Error(error) => {
                    //         tracing::warn!("peer event error {error:?}");
                    //         break;
                    //     }
                    // }
                }
                eyre::Ok(())
            }
        });

        Ok(Self {
            control,
            tasks: tasks,
        })
    }
}

impl Drop for RemotePeer {
    fn drop(&mut self) {
        tracing::info!("RemotePeer::drop");
        let control = self.control.clone();
        tokio::spawn(async move {
            control.send(PeerControl::Die).await.unwrap();
        });
    }
}

#[derive(Debug)]
struct _Peer {
    our_peer_id: PeerId,
    last_connection_request: Option<String>,
    remote_peers: HashMap<PeerId, RemotePeer>,
    connection_peer_id: HashMap<ConnectionId, PeerId>,
    signal_control: mpsc::Sender<SignallingControl>,
    app_event_tx: mpsc::Sender<AppEvent>,
    peer_tasks: tokio::task::JoinSet<Result<()>>,
}

impl Drop for _Peer {
    fn drop(&mut self) {
        tracing::info!(self.our_peer_id, "_Peer::drop");
    }
}

impl _Peer {}

#[derive(Clone, Deref, DerefMut)]
struct UIPeer(
    PeerId,
    #[deref]
    #[deref_mut]
    Arc<Mutex<_Peer>>,
);

impl Drop for UIPeer {
    fn drop(&mut self) {
        tracing::info!(our_peer_id = self.0, "UIPeer::drop");
    }
}

impl UIPeer {
    fn our_id(&self) -> &PeerId {
        &self.0
    }

    /// Create a new peer, blocking waiting for a connection to the signalling server and the id of this peer.
    async fn new<S: AsRef<str>>(
        signalling_address: S,
        app_event_tx: mpsc::Sender<AppEvent>,
    ) -> Result<Self> {
        let (control, mut event_rx) = signal::client(signalling_address.as_ref()).await?;

        // TODO(emily): Hold onto other events that might turn up here, right now we just THROW them away, not
        // very nice.

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
        });

        Ok(zelf)
    }

    fn weak(&self) -> Weak<Mutex<_Peer>> {
        Arc::downgrade(&self.1)
    }

    async fn inner(&self) -> tokio::sync::MappedMutexGuard<_Peer> {
        MutexGuard::map(self.lock().await, |peer| peer)
    }

    async fn signalling(
        zelf: Weak<Mutex<_Peer>>,
        mut signal_rx: mpsc::Receiver<SignallingEvent>,
        signal_tx: mpsc::Sender<SignallingControl>,
    ) -> Result<()> {
        while let Some(event) = signal_rx.recv().await {
            let strong_zelf = zelf.upgrade().ok_or(eyre::eyre!("no peer"))?;
            let mut zelf = strong_zelf.lock().await;

            match event {
                signal::SignallingEvent::Id(id) => {
                    unreachable!("We should only ever get our peer_id once");
                }
                signal::SignallingEvent::ConectionRequest(peer_id, connection_id) => {
                    tracing::info!(
                        zelf.our_peer_id,
                        peer_id,
                        ?connection_id,
                        "connection request"
                    );
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
                    let our_peer_id = zelf.our_peer_id.clone();
                    tracing::info!(our_peer_id, peer_id, offer, "offer");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(peer_data) = remote_peers.get(&peer_id) {
                        peer_data
                            .control
                            .send(PeerControl::Offer(offer))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(
                            our_peer_id,
                            peer_id,
                            ?remote_peers,
                            "got offer for unknown peer"
                        );
                    }
                }
                signal::SignallingEvent::Answer(peer_id, answer) => {
                    let our_peer_id = zelf.our_peer_id.clone();

                    tracing::info!(zelf.our_peer_id, peer_id, answer, "answer");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(remote_peer) = remote_peers.get(&peer_id) {
                        remote_peer
                            .control
                            .send(PeerControl::Answer(answer))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(
                            our_peer_id,
                            peer_id,
                            ?remote_peers,
                            "got answer for unknown peer {peer_id}"
                        );
                    }
                }
                signal::SignallingEvent::IceCandidate(peer_id, ice_candidate) => {
                    let our_peer_id = zelf.our_peer_id.clone();

                    tracing::info!(our_peer_id, peer_id, ?ice_candidate, "ice candidate");

                    let remote_peers = &mut zelf.remote_peers;
                    if let Some(remote_peer) = remote_peers.get(&peer_id) {
                        remote_peer
                            .control
                            .send(PeerControl::IceCandidate(ice_candidate))
                            .await
                            .unwrap();
                    } else {
                        tracing::debug!(
                            our_peer_id,
                            peer_id,
                            ?remote_peers,
                            "got ice candidate for unknown peer"
                        );
                    }
                }
                signal::SignallingEvent::ConnectionAccepted(peer_id, connection_id) => {
                    // NOTE(emily): We sent the request so we are controlling
                    let our_peer_id = zelf.our_peer_id.clone();

                    assert!(peer_id != our_peer_id);

                    tracing::info!(our_peer_id, peer_id, ?connection_id, "connection accepted");

                    zelf.peer_tasks.spawn({
                        let our_peer_id = our_peer_id;
                        let their_peer_id = peer_id.clone();
                        let tx = signal_tx.clone();
                        let zelf = Arc::downgrade(&strong_zelf);

                        async move {
                            let remote_peer =
                                RemotePeer::connected(true, tx, our_peer_id, their_peer_id.clone())
                                    .await?;

                            let control = remote_peer.control.clone();

                            if let Some(zelf) = zelf.upgrade() {
                                let mut zelf = zelf.lock().await;

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

    async fn connect(&self, peer_id: String) -> Result<()> {
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
                zelf.our_peer_id,
                peer_id,
                ?zelf.connection_peer_id,
                "no incoming connection request from peer"
            );
            Err(eyre::eyre!("no incoming connection request from peer"))
        }
    }
}

#[derive(PartialEq, Default)]
enum Visible {
    No,
    #[default]
    Yes,
}

#[derive(Default)]
struct PeerWindowState {
    visible: Visible,
    connect_peer_id: String,
    connection_requests: HashMap<ConnectionId, PeerId>,
    connected_peers: HashMap<PeerId, mpsc::Sender<PeerControl>>,
}

enum AppEvent {
    Peer(UIPeer),
    ConnectionRequest(PeerId, (ConnectionId, PeerId)),
    RemotePeerConnected(PeerId, (PeerId, mpsc::Sender<PeerControl>)),
}

struct App {
    peers: HashMap<PeerId, (PeerWindowState, UIPeer)>,
    event_rx: mpsc::Receiver<AppEvent>,
    event_tx: mpsc::Sender<AppEvent>,
}

impl Default for App {
    fn default() -> Self {
        let (event_tx, event_rx) = mpsc::channel(10);

        Self {
            peers: Default::default(),
            event_rx,
            event_tx,
        }
    }
}

impl App {
    fn ui(&mut self, ctx: &egui::Context) {
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
                    ui.horizontal(|ui| {
                        ui.label(id);

                        ui.selectable_value(&mut window_state.visible, Visible::No, "hide");
                        ui.selectable_value(&mut window_state.visible, Visible::Yes, "show");

                        if ui.button("disconnect").clicked() {
                            remove_peers.push(id.clone());
                        }
                    });

                    egui::Window::new(format!("Peer {id}")).show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.text_edit_singleline(&mut window_state.connect_peer_id);
                            if ui.button("connect").clicked() {
                                tokio::spawn({
                                    let connect_peer_id = window_state.connect_peer_id.clone();
                                    let peer = peer.clone();
                                    async move {
                                        peer.connect(connect_peer_id.clone()).await.unwrap();
                                    }
                                });
                            }
                        });

                        ui.heading("Connected Peers");

                        for (peer, control) in &window_state.connected_peers {
                            ui.horizontal(|ui| {
                                ui.label(format!("{}", peer));
                            });
                        }

                        ui.heading("Connection Requests");

                        for (c_id, p_id) in &window_state.connection_requests {
                            ui.horizontal(|ui| {
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
                            });
                        }
                    });
                }
            }

            for peer in remove_peers {
                self.peers.remove(&peer);
            }

            if let Ok(event) = self.event_rx.try_recv() {
                match event {
                    AppEvent::Peer(peer) => {
                        self.peers.insert(
                            format!("{}", peer.our_id()),
                            (PeerWindowState::default(), peer),
                        );
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
                }
            }
        });
    }
}

pub async fn ui(produce: &bool) -> Result<()> {
    // telemetry::client::sink().await;

    let mut app = App::default();

    let config = Config::load();

    let (width, height) = (config.width, config.height);

    let event_loop = EventLoopBuilder::new().build()?; // .with_any_thread(true).build()?;
    let window = WindowBuilder::new()
        .with_title("remote")
        .with_inner_size(PhysicalSize::new(width / 2, height / 2))
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

    event_loop.run(move |event, control_flow| match event {
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
                    control_flow.exit()
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
    //         let _tx = signal_tx.clone();
    //         let _our_peer_id = our_peer_id.clone();
    //         let _last_connection_request = last_connection_request.clone();
    //         let peer_controls = peer_controls.clone();
    //         async move {
    //             match async move {
    //                 // let maybe_file = std::env::var("media_filename").ok();

    //                 let (_tx, mut rx) = if let Some(file) = config.media_filename.as_ref() {
    //                     media::produce::produce(
    //                         config.encoder_api,
    //                         file,
    //                         config.width,
    //                         config.height,
    //                         config.framerate,
    //                         config.bitrate,
    //                     )
    //                     .await?
    //                 } else {
    //                     media::desktop_duplication::duplicate_desktop(
    //                         config.encoder_api,
    //                         config.width,
    //                         config.height,
    //                         config.framerate,
    //                         config.bitrate,
    //                     )
    //                     .await?
    //                 };

    //                 while let Some(event) = rx.recv().await {
    //                     match event {
    //                         media::produce::MediaEvent::Audio(audio) => {
    //                             tracing::trace!("produce audio {}", audio.len());
    //                             let peer_controls = peer_controls.lock().await;
    //                             for (_, control) in peer_controls.iter() {
    //                                 control.send(PeerControl::Audio(audio.clone())).await?;
    //                             }
    //                         }
    //                         media::produce::MediaEvent::Video(video) => {
    //                             // tracing::debug!("throwing video");
    //                             tracing::trace!("produce video {}", video.data.len());
    //                             let peer_controls = peer_controls.lock().await;
    //                             for (_, control) in peer_controls.iter() {
    //                                 control.send(PeerControl::Video(video.clone())).await?;
    //                             }
    //                         }
    //                     }
    //                 }

    //                 eyre::Ok(())
    //             }
    //             .await
    //             {
    //                 Ok(_) => {
    //                     tracing::info!("produce down ok")
    //                 }
    //                 Err(err) => {
    //                     tracing::error!("produce down err {err}")
    //                 }
    //             }

    //             eyre::Ok(())
    //         }
    //     });
    // }

    // tokio::task::spawn_blocking({
    //     let tx = signal_tx.clone();
    //     let our_peer_id = our_peer_id.clone();
    //     let last_connection_request = last_connection_request.clone();
    //     let peer_controls = peer_controls.clone();

    //     move || {
    //         for line in std::io::stdin().lines() {
    //             if let Ok(line) = line {
    //                 let (command, arg) = {
    //                     let mut split = line.split(" ");
    //                     (
    //                         split.next().unwrap_or_default(),
    //                         split.next().unwrap_or_default(),
    //                     )
    //                 };

    //                 let tx = tx.clone();
    //                 let our_peer_id = our_peer_id.clone();
    //                 let peer_controls = peer_controls.clone();
    //                 let connection_peer_id = connection_peer_id.clone();

    //                 match command {
    //                     "connect" => {
    //                         // let peer_id = Uuid::from_str(arg)?;
    //                         let peer_id = arg.into();
    //                         tx.blocking_send(signal::SignallingControl::RequestConnection(
    //                             peer_id,
    //                         ))?;
    //                     }
    //                     "accept" => {
    //                         tracing::debug!("accept '{arg}'");
    //                         let connection_id = if arg == "" {
    //                             last_connection_request.try_lock().unwrap().unwrap()
    //                         } else {
    //                             Uuid::from_str(arg)?
    //                         };

    //                         tokio::spawn({
    //                             let tx = tx.clone();

    //                             async move {
    //                                 if let Some(peer_id) =
    //                                     connection_peer_id.lock().await.remove(&connection_id)
    //                                 {
    //                                     let our_peer_id =
    //                                         our_peer_id.lock().await.as_ref().unwrap().clone();

    //                                     assert!(peer_id != our_peer_id);

    //                                     peer_connected(
    //                                         our_peer_id,
    //                                         peer_id,
    //                                         tx,
    //                                         peer_controls,
    //                                         false,
    //                                     )
    //                                     .await
    //                                     .unwrap();
    //                                 } else {
    //                                     tracing::debug!("Unknown connection id {connection_id}");
    //                                 }
    //                             }
    //                         });

    //                         tx.blocking_send(signal::SignallingControl::AcceptConnection(
    //                             connection_id,
    //                         ))?;
    //                     }
    //                     "die" => {
    //                         tokio::spawn(async move {
    //                             for (_, control) in peer_controls.lock().await.drain() {
    //                                 control.send(PeerControl::Die).await.unwrap();
    //                             }
    //                         });
    //                     }
    //                     "quit" | "exit" | "q" => {
    //                         std::process::exit(0);
    //                     }

    //                     command => tracing::info!("Unknown command {command}"),
    //                 }
    //             }
    //         }

    //         tracing::warn!("stdin is done");

    //         eyre::Ok(())
    //     }
    // })
    // .await??;

    Ok(())
}
