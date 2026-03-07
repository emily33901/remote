#[derive(Debug, Clone, PartialEq)]
pub enum RemotePeerState {
    Connecting,
    Connected,
    Disconnecting,
    Disconnected { reason: DisconnectReason },
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisconnectReason {
    UserInitiated,
    ConnectionFailed,
    RemoteClosed,
    Timeout,
}
