# Plan: UI Rewrite with Peer/RemotePeer Architecture

## ✅ COMPLETED

The UI has been completely rewritten. The codebase is now split into two crates:

### 1. `remote` (Core Library)

**Location:** `src/lib.rs` and submodules

Clean, UI-agnostic API with two primary types:

- **`Peer`** - Local peer with lifecycle management
  - `new(config) -> (Peer, Receiver<PeerEvent>)`
  - `connect(peer_id)` - Initiate connection
  - `accept_connection(connection_id)` - Accept incoming connection
  - `disconnect(peer_id)` - Disconnect from peer
  - `id()` - Get local peer ID

- **`RemotePeer`** - Connection to a remote peer
  - State machine: Connecting → Connected → Disconnected
  - Event stream for video/audio/data

**File Structure:**
```
src/
├── lib.rs              # Re-exports: Peer, RemotePeer, PeerEvent, etc.
├── types.rs            # Re-exports PeerId, ConnectionId from signal crate
├── error.rs            # Unified Error and Result types
├── peer/
│   ├── mod.rs          # Peer type
│   ├── config.rs       # PeerConfig
│   └── event.rs        # PeerEvent enum
├── remote_peer/
│   ├── mod.rs          # RemotePeer type
│   ├── state.rs        # RemotePeerState, DisconnectReason
│   └── event.rs        # RemotePeerEvent enum
├── connection/
│   ├── mod.rs          # create_peer_connection()
│   └── channels.rs     # Logic/Audio/Video channels
└── protocol/
    ├── mod.rs          # Mode type
    └── messages.rs     # LogicMessage, StreamRequest, etc.
```

### 2. `remote-ui` (UI Crate)

**Location:** `remote-ui/`

Separate egui-based UI crate consuming the core library:

```
remote-ui/
├── Cargo.toml
└── src/
    ├── main.rs         # Entry point with tracing setup
    ├── config.rs       # Config loading from environment
    └── app.rs          # RemoteApp implementing eframe::App
```

**Features:**
- Clock window (existing)
- Peer creation button
- Connect to peer input
- Event logging to console

## Commits

1. `9c79c48` - Create remote-ui crate with Peer and RemotePeer types
2. `6564696` - Remove old UI code and unused modules (54 files deleted, 11,265 lines removed)

## Remaining Work (Optional Enhancements)

- Video display panel in remote-ui
- Statistics overlay
- Settings panel for encoder/decoder selection
- Connection request dialogs
- Examples in `remote/examples/`

## Summary

| Component | Old Location | New Location |
|-----------|--------------|--------------|
| Peer type | `src/peer.rs` | `src/peer/mod.rs` |
| RemotePeer type | `src/ui/peer.rs` | `src/remote_peer/mod.rs` |
| UI App | `src/ui/app.rs` | `remote-ui/src/app.rs` |
| Protocol | Inline | `src/protocol/` |
| Error types | Scattered | `src/error.rs` |
| Binary entry | `src/main.rs` | `remote-ui/src/main.rs` |
