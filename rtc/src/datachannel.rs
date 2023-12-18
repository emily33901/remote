mod channel;
pub(crate) mod peer;

use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use super::{ChannelControl, ChannelEvent, ChannelOptions, DataChannel, PeerConnection};
use eyre::Result;
