# Plan: UI Rewrite with Peer/RemotePeer Architecture

## Overview

Split the current `remote` crate into two distinct crates:
1. **`remote`** - Core library providing `Peer` and `RemotePeer` types for P2P streaming
2. **`remote-ui`** - Separate egui-based UI crate consuming the core library

---

## Part 1: Core `remote` Crate Refactoring

### Goal
Provide a clean, UI-agnostic API with two primary types:

#### 1.1 `Peer` Type (Local Peer)

**Location:** `remote/src/peer/mod.rs`

```rust
pub struct Peer {
    id: PeerId,
    signal_client: SignalClient,
    remote_peers: HashMap<PeerId, RemotePeer>,
    config: PeerConfig,
}

pub struct PeerConfig {
    pub signal_server: Url,
    pub webrtc_api: rtc::Api,
    pub encoder_config: media::EncoderConfig,
    pub decoder_config: media::DecoderConfig,
}

impl Peer {
    // Lifecycle
    pub async fn new(config: PeerConfig) -> Result<Self>;
    pub async fn connect(&mut self, peer_id: PeerId) -> Result<RemotePeer>;
    pub async fn disconnect(&mut self, peer_id: &PeerId) -> Result<()>;
    pub async fn shutdown(self) -> Result<()>;

    // Queries
    pub fn id(&self) -> &PeerId;
    pub fn connected_peers(&self) -> impl Iterator<&PeerId>;
    pub fn remote_peer(&self, id: &PeerId) -> Option<&RemotePeer>;

    // Events
    pub fn subscribe(&self) -> Receiver<PeerEvent>;
}

pub enum PeerEvent {
    ConnectionRequested { from: PeerId, connection_id: ConnectionId },
    PeerConnected { peer_id: PeerId },
    PeerDisconnected { peer_id: PeerId, reason: DisconnectReason },
    IncomingStream { peer_id: PeerId },
    Error { peer_id: Option<PeerId>, error: Error },
}
```

#### 1.2 `RemotePeer` Type (Connection to Remote)

**Location:** `remote/src/remote_peer/mod.rs`

```rust
pub struct RemotePeer {
    peer_id: PeerId,
    state: RemotePeerState,
    control: mpsc::Sender<PeerControl>,
    media_control: mpsc::Sender<MediaControl>,
}

pub enum RemotePeerState {
    Connecting,
    Connected { 
        channels: Channels,
        capabilities: PeerCapabilities,
    },
    Disconnecting,
    Disconnected { reason: DisconnectReason },
}

pub struct Channels {
    pub logic: Channel<LogicMessage>,
    pub audio: Channel<AudioData>,
    pub video: Channel<VideoData>,
}

impl RemotePeer {
    // Lifecycle
    pub async fn accept(self) -> Result<ConnectedRemotePeer>;
    pub async fn reject(self) -> Result<()>;
    pub async fn disconnect(self) -> Result<()>;

    // Streaming Control
    pub async fn request_stream(&self, request: StreamRequest) -> Result<StreamHandle>;
    pub async fn stop_stream(&self) -> Result<()>;

    // Queries
    pub fn id(&self) -> &PeerId;
    pub fn state(&self) -> &RemotePeerState;
    pub fn statistics(&self) -> PeerStatistics;

    // Events
    pub fn subscribe(&self) -> Receiver<RemotePeerEvent>;
}

pub enum RemotePeerEvent {
    StateChanged { new_state: RemotePeerState },
    VideoFrame { frame: VideoFrame },
    AudioData { data: AudioData },
    Statistics { stats: Statistics },
    StreamRequested { request: StreamRequest },
    Error { error: Error },
}
```

### 1.3 File Structure for `remote` Crate

```
remote/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Re-exports: Peer, RemotePeer, PeerEvent, etc.
│   ├── peer/
│   │   ├── mod.rs                # Peer type
│   │   ├── config.rs             # PeerConfig
│   │   └── event.rs              # PeerEvent
│   ├── remote_peer/
│   │   ├── mod.rs                # RemotePeer type
│   │   ├── state.rs              # RemotePeerState, DisconnectReason
│   │   ├── event.rs              # RemotePeerEvent
│   │   └── statistics.rs         # PeerStatistics
│   ├── connection/
│   │   ├── mod.rs                # Connection management
│   │   ├── channels.rs           # Logic/Audio/Video channels
│   │   └── negotiation.rs        # SDP/ICE handling
│   ├── protocol/
│   │   ├── mod.rs                # Wire protocol types
│   │   ├── logic.rs              # LogicMessage (StreamRequest, Ping, etc.)
│   │   ├── audio.rs              # AudioData
│   │   └── video.rs              # VideoData, VideoBuffer, chunking
│   ├── error.rs                  # Error types
│   └── types.rs                  # PeerId, ConnectionId, etc.
```

### 1.4 Dependencies for `remote` Crate

```toml
[dependencies]
remote-signal = { path = "../signal" }
remote-rtc = { path = "../rtc" }
remote-media = { path = "../media" }
tokio = { version = "1", features = ["full"] }
thiserror = "1"
tracing = "0.1"

# Optional for WASM/async-agnostic
[features]
default = ["tokio-runtime"]
tokio-runtime = ["tokio/rt-multi-thread"]
```

---

## Part 2: New `remote-ui` Crate

### Goal
Clean egui-based UI that consumes `remote::Peer` and `remote::RemotePeer`.

### 2.1 File Structure

```
remote-ui/
├── Cargo.toml
├── src/
│   ├── main.rs                   # Entry point
│   ├── app.rs                    # eframe::App implementation
│   ├── panels/
│   │   ├── mod.rs
│   │   ├── connect.rs            # Connection panel (enter peer ID)
│   │   ├── peers_list.rs         # List of connected peers
│   │   ├── video_viewer.rs       # Video display for incoming streams
│   │   ├── statistics.rs         # Stats overlay/panel
│   │   └── settings.rs           # Settings/configuration
│   ├── state/
│   │   ├── mod.rs
│   │   └── app_state.rs          # UI-specific state
│   └── renderer/
│       ├── mod.rs
│       └── texture_renderer.rs   # D3D11/egui texture integration
```

### 2.2 App Architecture

```rust
// remote-ui/src/app.rs
pub struct RemoteApp {
    // Core
    peer: Option<Arc<Mutex<Peer>>>,
    peer_events: Option<Receiver<PeerEvent>>,
    
    // UI State
    connect_input: String,
    pending_requests: Vec<PendingConnection>,
    connected_peers: HashMap<PeerId, ConnectedPeerUi>,
    
    // Media
    video_renderer: VideoRenderer,
}

struct ConnectedPeerUi {
    peer: RemotePeer,
    events: Receiver<RemotePeerEvent>,
    video_texture: Option<egui::TextureHandle>,
    statistics: Statistics,
    show_stats: bool,
}

impl eframe::App for RemoteApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll events from Peer
        self.poll_peer_events();
        
        // Poll events from each RemotePeer
        self.poll_remote_peer_events();
        
        // Draw UI
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Peer ID:");
                if let Some(peer) = &self.peer {
                    ui.label(peer.lock().id().to_string());
                }
            });
        });
        
        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_peers(ui);
            self.show_video(ui);
        });
    }
}
```

### 2.3 Dependencies for `remote-ui` Crate

```toml
[package]
name = "remote-ui"
version = "0.1.0"

[dependencies]
remote = { path = "../remote" }
remote-media = { path = "../media" }
eframe = { version = "0.27", features = ["wgpu"] }
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
tracing-subscriber = "0.3"
```

---

## Part 3: Migration Steps

### Phase 1: Extract Core Types (Week 1)
1. Create `remote/src/peer/mod.rs` with `Peer` struct
2. Create `remote/src/remote_peer/mod.rs` with `RemotePeer` struct
3. Move `PeerId`, `ConnectionId` to `remote/src/types.rs`
4. Create `remote/src/error.rs` with unified error types
5. Move protocol messages to `remote/src/protocol/`
6. **Test:** Unit tests for new types compile

### Phase 2: Refactor Peer Logic (Week 2)
1. Extract connection logic from `src/ui/peer.rs` into `Peer::new()`, `Peer::connect()`
2. Implement event subscription pattern (`Peer::subscribe()`)
3. Move WebRTC setup to internal `connection/` module
4. Remove UI dependencies from core types
5. **Test:** Integration test: connect two peers via signaling

### Phase 3: Refactor RemotePeer Logic (Week 3)
1. Implement `RemotePeer` with proper state machine
2. Add streaming methods (`request_stream`, `stop_stream`)
3. Implement video/audio event emission
4. Create `Channels` abstraction for logic/audio/video
5. **Test:** Stream video between two peers

### Phase 4: Create remote-ui Crate (Week 4)
1. Create new crate with `eframe` dependency
2. Implement basic `RemoteApp` with peer initialization
3. Add connection panel UI
4. Add video display with `egui::PaintCallback`
5. **Test:** UI can display incoming video

### Phase 5: Complete UI Panels (Week 5)
1. Implement peers list panel
2. Implement statistics overlay
3. Implement settings panel (encoder/decoder selection)
4. Add connection request dialogs
5. **Test:** Full end-to-end UI test

### Phase 6: Cleanup (Week 6)
1. Remove old `src/ui/` code from `remote`
2. Update `remote/src/lib.rs` to export clean API
3. Update documentation
4. Add examples in `remote/examples/`
5. **Test:** All tests pass, CI green

---

## Part 4: API Design Principles

### 4.1 Async-First
All I/O operations return `Future` or take async callbacks:
```rust
impl Peer {
    pub async fn connect(&mut self, peer_id: PeerId) -> Result<RemotePeer>;
}
```

### 4.2 Event-Driven
State changes communicated via channels, not callbacks:
```rust
let events = peer.subscribe();
while let Some(event) = events.recv().await {
    match event {
        PeerEvent::PeerConnected { peer_id } => { /* ... */ }
        _ => {}
    }
}
```

### 4.3 Thread-Safe
`Peer` and `RemotePeer` are `Send + Sync`:
```rust
let peer = Arc::new(Mutex::new(Peer::new(config).await?));
```

### 4.4 Error Handling
Typed errors with `thiserror`:
```rust
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Signaling error: {0}")]
    Signaling(#[from] signal::Error),
    #[error("WebRTC error: {0}")]
    WebRTC(#[from] rtc::Error),
    #[error("Peer not found: {0}")]
    PeerNotFound(PeerId),
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
}
```

---

## Part 5: Open Questions

1. **Should `RemotePeer` be cloneable?**
   - Pro: Multiple UI components can subscribe
   - Con: Lifecycle becomes unclear
   - **Recommendation:** Use `Arc<RemotePeer>` for sharing

2. **How to handle video textures in UI?**
   - Current: D3D11 textures passed directly
   - New: `VideoFrame` trait with backend-specific implementations
   - egui integration via `wgpu` texture sharing

3. **Configuration persistence?**
   - Move `.env` handling to `remote-ui` or separate `remote-config` crate?
   - **Recommendation:** Keep in `remote-ui` for now

4. **Testing strategy?**
   - Mock signaling server for integration tests
   - Loopback video device for media tests

---

## Summary

| Component | Current Location | New Location |
|-----------|------------------|--------------|
| Peer type | `src/ui/peer.rs::_Peer` | `remote/src/peer/mod.rs` |
| RemotePeer type | `src/ui/peer.rs::RemotePeer` | `remote/src/remote_peer/mod.rs` |
| UI App | `src/ui/app.rs` | `remote-ui/src/app.rs` |
| Peer events | Scattered | `remote/src/peer/event.rs` |
| Protocol | Inline | `remote/src/protocol/` |
| Error types | Scattered | `remote/src/error.rs` |
