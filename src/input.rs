use input::mouse::{ButtonAction, MouseButton};

pub(crate) enum Mouse {
    Absolute(i32, i32),
    Relative(i32, i32),
    Click(MouseButton, ButtonAction),
}

pub(crate) enum InputMessage {
    Mouse(Mouse),
}

// #[tracing::instrument(skip(peer_connection))]
// pub(crate) async fn input_channel(
//     peer_connection: &dyn PeerConnection,
//     controlling: bool,
// ) -> Result<(mpsc::Sender<InputMessage>, mpsc::Receiver<InputMessage>)> {
//     let (control_tx, mut control_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);
//     let (event_tx, event_rx) = mpsc::channel(ARBITRARY_CHANNEL_LIMIT);

//     let (tx, mut rx) = peer_connection.channel("input", controlling, None).await?;

//     (control_tx, event_rx)
// }
