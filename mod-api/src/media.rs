//! Frames, time and sound out of the presented world, and media files made
//! of them: frame captures, the stepped presentation clock, taps on the
//! world's sound, and encoding a media file into a mod's storage.
//!
//! The mod names every container, codec, option and path. The engine runs
//! the encoder and nothing else: no presets, no quality dial, no naming.

use serde::{Deserialize, Serialize};

use crate::ClientStorageScope;

/// What a frame capture holds.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureSource {
    /// The frame as the scene layer holds it at the capture point: the world,
    /// hands, aim marks and the HUD the claims leave up. Never window UI, a
    /// mod's document, canvas or overlay, world marks, or a menu.
    Scene,
    /// The graded world image before the scene UI: the world, with the
    /// first-person hand when it shows, and no HUD or crosshair.
    World,
}

/// Which rendered frame a capture takes.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureWhen {
    /// The next rendered frame.
    Next,
    /// The next rendered frame whose world is settled: nothing waiting to
    /// mesh, relight, stream or upload.
    Settled,
}

/// Where a frame capture's pixels go.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientCaptureInto {
    /// Appended to this file as raw RGBA8 rows, top row first:
    /// `width × height × 4` bytes.
    File {
        scope: ClientStorageScope,
        path: String,
    },
    /// Handed to this open media file's encoder.
    Media(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientCaptureStatus {
    Armed,
    /// Taken on the frame presented at `time` (presentation time).
    Taken {
        time: f64,
    },
    /// File: the pixels' absolute range, once handed to the OS. Media: handed
    /// to the encoder (`range: None`).
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

/// One step of the stepped presentation clock (`ClientClockSet`).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientStep {
    /// Presentation time per step; finite and > 0.
    pub seconds: f64,
    /// Seeds the stepped timeline's own randomness (the world sound's variant
    /// picks and pitch jitter), so the same timeline sounds the same.
    pub seed: u64,
}

/// Where a tap's samples go: f32 LE, interleaved.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientAudioInto {
    /// Appended to this file.
    File {
        scope: ClientStorageScope,
        path: String,
    },
    /// Appended to this open media file's audio, which must declare the tap's
    /// rate and channels.
    Media(u64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientAudioTapData {
    /// Presentation time of the tap's first sample frame; `None` until one arrived.
    pub started_at: Option<f64>,
    /// Sample frames delivered.
    pub frames: u64,
    pub ended: bool,
    /// Why it ended, when it did not end on request.
    pub error: Option<String>,
}

/// What this machine's encoder can write (`ClientMediaEncoders`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaCapabilities {
    /// The encoder found (its version line); `None` = none on this machine.
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
    /// Frames per second as `[numerator, denominator]`.
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
    /// Accepted, waiting for a running encoder to finish.
    Queued,
    Open,
    Finishing,
    Finished,
    Failed,
}

/// What a mod cannot classify portably from the encoder's or the OS's words.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientMediaFailure {
    EncoderFailed,
    DiskFull,
    Aborted,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientMediaStateData {
    pub phase: ClientMediaPhase,
    /// Frames accepted (captures delivered + pushes).
    pub frames_in: u64,
    /// Frames the encoder took.
    pub frames_encoded: u64,
    /// Sample frames accepted.
    pub audio_in: u64,
    /// The finished file's size, once `Finished`.
    pub bytes: u64,
    pub failure: Option<ClientMediaFailure>,
    /// The encoder's or the OS's own words.
    pub error: Option<String>,
}
