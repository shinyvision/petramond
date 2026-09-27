use crate::client::ClientStorageScope;
use crate::legality::prelude::*;
use crate::media::{
    ClientAudioInto, ClientCaptureInto, ClientCaptureSource, ClientCaptureWhen, ClientMediaAudio,
    ClientMediaVideo, ClientStep,
};

host_domain! {
    ClientMediaCall {
        ClientFrameCapture {
            source: ClientCaptureSource,
            size: Option<[u32; 2]>,
            when: ClientCaptureWhen,
            advance: bool,
            into: ClientCaptureInto,
        } => legal(CLIENT, Any, Write),
        ClientFrameCapturePoll {
            capture: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientFrameCancel {
            capture: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientClockSet {
            step: Option<ClientStep>,
        } => legal(CLIENT, Any, Write),
        ClientClockAdvance => legal(CLIENT, Any, Write),
        ClientAudioTap {
            sample_rate: u32,
            channels: u16,
            into: ClientAudioInto,
        } => legal(CLIENT, Any, Write),
        ClientAudioTapState {
            tap: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientAudioTapEnd {
            tap: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaEncoders {
            refresh: bool,
        } => legal(CLIENT_SHELL, Any, Read),
        ClientMediaOpen {
            scope: ClientStorageScope,
            path: String,
            container: String,
            video: Option<ClientMediaVideo>,
            audio: Option<ClientMediaAudio>,
            options: Vec<(String, String)>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaPushFrame {
            media: u64,
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaPushAudio {
            media: u64,
            #[serde(with = "serde_bytes")]
            pcm: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaClose {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaAbort {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        ClientMediaState {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
