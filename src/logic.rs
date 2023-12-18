use eyre::Result;
use std::sync::Arc;

use rtc::PeerConnection;

pub(crate) async fn logic_channel(
    _peer_connection: Arc<dyn PeerConnection>,
    _controlling: bool,
) -> Result<()> {
    Ok(())
}
