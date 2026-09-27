use serde::{Deserialize, Serialize};

use crate::ClientStorageScope;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureSource {
    Scene,
    World,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureWhen {
    Next,
    Settled,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureInto {
    File {
        scope: ClientStorageScope,
        path: String,
    },
    Media(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientCaptureStatus {
    Armed,
    Taken {
        time: f64,
    },
    Delivered {
        time: f64,
        width: u32,
        height: u32,
        range: Option<[u64; 2]>,
    },
    Failed {
        why: String,
    },
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientStep {
    pub seconds: f64,
    pub seed: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientAudioInto {
    File {
        scope: ClientStorageScope,
        path: String,
    },
    Media(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientAudioTapData {
    pub started_at: Option<f64>,
    pub frames: u64,
    pub ended: bool,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaCapabilities {
    pub encoder: Option<String>,
    pub containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaVideo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub fps: [u32; 2],
    pub options: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaAudio {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub options: Vec<(String, String)>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientMediaPhase {
    Queued,
    Open,
    Finishing,
    Finished,
    Failed,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientMediaFailure {
    EncoderFailed,
    DiskFull,
    Aborted,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaStateData {
    pub phase: ClientMediaPhase,
    pub frames_in: u64,
    pub frames_encoded: u64,
    pub audio_in: u64,
    pub bytes: u64,
    pub failure: Option<ClientMediaFailure>,
    pub error: Option<String>,
}
