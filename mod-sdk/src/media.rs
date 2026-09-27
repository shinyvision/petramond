//! Frames, time and sound out of the presented world, and media files made of
//! them: frame captures, the stepped presentation clock, taps on the world's
//! sound, and an encoder writing a media file into this mod's storage.
//!
//! The mod names every container, codec, option and path; the engine has no
//! presets, no quality dial and no progress UI.

use mod_api::{
    ClientAudioInto, ClientAudioTapData, ClientCaptureInto, ClientCaptureSource,
    ClientCaptureStatus, ClientCaptureWhen, ClientMediaAudio, ClientMediaCapabilities,
    ClientMediaStateData, ClientMediaVideo, ClientStep, ClientStorageScope, HostRet,
};

use crate::__rt::{host_fn, try_host_fn};

try_host_fn! {
    /// CLIENT: arm a capture of a rendered frame — `source` at `size` (`None`
    /// = the frame's own), taken when `when` first holds at a render, into a
    /// file (raw RGBA8 rows) or an open media file. `advance` steps the stepped
    /// clock on the frame after the one it is taken on. `Ok(capture)`; `Err` =
    /// refused.
    pub fn client_frame_capture(
        source: ClientCaptureSource,
        size: Option<[u32; 2]>,
        when: ClientCaptureWhen,
        advance: bool,
        into: ClientCaptureInto,
    ) -> u64 => ClientFrameCapture { source, size, when, advance, into } => Ticket
}

host_fn! {
    /// CLIENT: how a capture stands. It is forgotten once polled as
    /// `Delivered` or `Failed`.
    pub fn client_frame_capture_poll(capture: u64) -> ClientCaptureStatus
        => ClientFrameCapturePoll { capture } => ClientCaptureStatus
}

host_fn! {
    /// CLIENT: disarm an armed capture.
    pub fn client_frame_cancel(capture: u64) => ClientFrameCancel { capture }
}

host_fn! {
    /// CLIENT: `Some(step)` = presentation time moves only in steps of exactly
    /// `step.seconds`, one per advance; `None` = back to the wall clock.
    /// `false` = another mod holds the clock.
    pub fn client_clock_set(step: Option<ClientStep>) -> bool => ClientClockSet { step } => Bool
}

host_fn! {
    /// CLIENT: step the stepped clock on the next frame.
    pub fn client_clock_advance() => ClientClockAdvance
}

try_host_fn! {
    /// CLIENT: tap the world's sound at `sample_rate` and `channels` (f32 LE,
    /// interleaved) into a file or an open media file. `Ok(tap)`; `Err` =
    /// refused (this build has no mixer).
    pub fn client_audio_tap(sample_rate: u32, channels: u16, into: ClientAudioInto)
        -> u64
        => ClientAudioTap { sample_rate, channels, into } => Ticket
}

host_fn! {
    /// CLIENT: how a tap stands; `None` = no such tap.
    pub fn client_audio_tap_state(tap: u64) -> Option<ClientAudioTapData>
        => ClientAudioTapState { tap } => ClientAudioTapState
}

host_fn! {
    /// CLIENT: end a tap.
    pub fn client_audio_tap_end(tap: u64) => ClientAudioTapEnd { tap }
}

host_fn! {
    /// CLIENT: what this machine's encoder can write; `refresh` asks again.
    /// `None` = still asking.
    pub fn client_media_encoders(refresh: bool) -> Option<ClientMediaCapabilities>
        => ClientMediaEncoders { refresh } => ClientMediaEncoders
}

try_host_fn! {
    /// CLIENT: start writing a media file at `path` in `scope`, in the
    /// container and codecs named, with every option the mod names. The file
    /// appears only once complete. `Ok(media)` (it may start `Queued`); `Err` =
    /// refused (no encoder, a container or codec this machine lacks, or the
    /// path in use).
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
    /// CLIENT: push one RGBA8 frame at the video's size. `false` = not taken
    /// now (the encoder is behind, the file queued, failed or closed).
    pub fn client_media_push_frame(media: u64, rgba: Vec<u8>) -> bool
        => ClientMediaPushFrame { media, rgba } => Bool
}

host_fn! {
    /// CLIENT: push f32 LE interleaved samples at the audio's rate and
    /// channels. `false` as for [`client_media_push_frame`].
    pub fn client_media_push_audio(media: u64, pcm: Vec<u8>) -> bool
        => ClientMediaPushAudio { media, pcm } => Bool
}

host_fn! {
    /// CLIENT: no more input; the file finishes in the background.
    pub fn client_media_close(media: u64) => ClientMediaClose { media }
}

host_fn! {
    /// CLIENT: stop now and leave nothing.
    pub fn client_media_abort(media: u64) => ClientMediaAbort { media }
}

host_fn! {
    /// CLIENT: how a media file stands; `None` = no such media file.
    pub fn client_media_state(media: u64) -> Option<ClientMediaStateData>
        => ClientMediaState { media }
        => HostRet::ClientMediaState(state) => state.map(|state| *state)
}
