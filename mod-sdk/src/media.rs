use mod_api::{
    ClientAudioInto, ClientAudioTapData, ClientCaptureInto, ClientCaptureSource,
    ClientCaptureStatus, ClientCaptureWhen, ClientMediaAudio, ClientMediaCapabilities,
    ClientMediaStateData, ClientMediaVideo, ClientStep, ClientStorageScope, HostRet,
};

use crate::__rt::{host_fn, try_host_fn};

try_host_fn! {
    pub fn client_frame_capture(
        source: ClientCaptureSource,
        size: Option<[u32; 2]>,
        when: ClientCaptureWhen,
        advance: bool,
        into: ClientCaptureInto,
    ) -> u64 => ClientFrameCapture { source, size, when, advance, into } => Ticket
}

host_fn! {
    pub fn client_frame_capture_poll(capture: u64) -> ClientCaptureStatus
        => ClientFrameCapturePoll { capture } => ClientCaptureStatus
}

host_fn! {
    pub fn client_frame_cancel(capture: u64) => ClientFrameCancel { capture }
}

host_fn! {
    pub fn client_clock_set(step: Option<ClientStep>) -> bool => ClientClockSet { step } => Bool
}

host_fn! {
    pub fn client_clock_advance() => ClientClockAdvance
}

try_host_fn! {
    pub fn client_audio_tap(sample_rate: u32, channels: u16, into: ClientAudioInto)
        -> u64
        => ClientAudioTap { sample_rate, channels, into } => Ticket
}

host_fn! {
    pub fn client_audio_tap_state(tap: u64) -> Option<ClientAudioTapData>
        => ClientAudioTapState { tap } => ClientAudioTapState
}

host_fn! {
    pub fn client_audio_tap_end(tap: u64) => ClientAudioTapEnd { tap }
}

host_fn! {
    pub fn client_media_encoders(refresh: bool) -> Option<ClientMediaCapabilities>
        => ClientMediaEncoders { refresh } => ClientMediaEncoders
}

try_host_fn! {
    pub fn client_media_open(
        scope: ClientStorageScope,
        path: &str,
        container: &str,
        video: Option<ClientMediaVideo>,
        audio: Option<ClientMediaAudio>,
        options: Vec<(String, String)>,
    ) -> u64 => ClientMediaOpen {
        scope,
        path: path.into(),
        container: container.into(),
        video,
        audio,
        options,
    } => Ticket
}

host_fn! {
    pub fn client_media_push_frame(media: u64, rgba: Vec<u8>) -> bool
        => ClientMediaPushFrame { media, rgba } => Bool
}

host_fn! {
    pub fn client_media_push_audio(media: u64, pcm: Vec<u8>) -> bool
        => ClientMediaPushAudio { media, pcm } => Bool
}

host_fn! {
    pub fn client_media_close(media: u64) => ClientMediaClose { media }
}

host_fn! {
    pub fn client_media_abort(media: u64) => ClientMediaAbort { media }
}

host_fn! {
    pub fn client_media_state(media: u64) -> Option<ClientMediaStateData>
        => ClientMediaState { media }
        => HostRet::ClientMediaState(state) => state.map(|state| *state)
}
