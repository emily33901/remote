use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EncodeStatistics {
    pub media_queue_len: usize,
    pub time: Duration,
    pub end_time: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DecodeStatistics {
    pub media_queue_len: usize,
    pub time: Duration,
    pub start_time: SystemTime,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionStatistics {
    pub media_queue_len: usize,
    pub time: Duration,
}

#[derive(Clone, Default, Debug, Serialize, Deserialize)]
pub struct Statistics {
    pub encode: Option<EncodeStatistics>,
    pub decode: Option<DecodeStatistics>,
    pub convert: Option<ConversionStatistics>,
}
