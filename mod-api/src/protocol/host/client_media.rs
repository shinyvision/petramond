//! Rendered frames, the stepped presentation clock, taps on the world's sound,
//! and media files encoded into mod storage.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::client::ClientStorageScope;
use crate::legality::prelude::*;
use crate::media::{
    ClientAudioInto, ClientCaptureInto, ClientCaptureSource, ClientCaptureWhen, ClientMediaAudio,
    ClientMediaVideo, ClientStep,
};

host_domain! {
    /// Frames, the stepped clock, sound taps and media files. Arming a capture,
    /// holding the clock and tapping sound need a presented world; handles
    /// outlive the world they were made in, and a media file takes frames a
    /// mod made itself, on the shell too.
    ClientMediaCall {
        /// CLIENT: arm a capture of a rendered frame: `source`, at `size`
        /// (`None` = the frame's own), taken `when` the condition first holds at
        /// a render, into a file or a media file. `advance` steps the stepped
        /// clock on the frame after the one it is taken on.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket).
        ClientFrameCapture {
            source: ClientCaptureSource,
            size: Option<[u32; 2]>,
            when: ClientCaptureWhen,
            advance: bool,
            into: ClientCaptureInto,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: how a capture stands.
        /// → [`HostRet::ClientCaptureStatus`](crate::HostRet::ClientCaptureStatus).
        ClientFrameCapturePoll {
            capture: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: disarm an armed capture. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientFrameCancel {
            capture: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: `Some(step)` = presentation time moves only in steps of
        /// exactly `step.seconds`, one per advance; `None` = back to the wall
        /// clock, never running backwards. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` = another mod holds the clock.
        ClientClockSet {
            step: Option<ClientStep>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: step the stepped clock on the next frame.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientClockAdvance => legal(CLIENT, Any, Write),
        /// CLIENT: tap the WORLD's sound (never the interface's) at `sample_rate`
        /// and `channels`, as f32 LE interleaved samples into a file or a media
        /// file. On the wall clock the sound stays on the speakers and the tap
        /// copies it; on the stepped clock it is mixed on that timeline.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket).
        ClientAudioTap {
            sample_rate: u32,
            channels: u16,
            into: ClientAudioInto,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: how a tap stands.
        /// → [`HostRet::ClientAudioTapState`](crate::HostRet::ClientAudioTapState):
        /// `None` = no such tap of this instance.
        ClientAudioTapState {
            tap: u64,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: end a tap. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientAudioTapEnd {
            tap: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: what this machine's encoder can write; `refresh` asks it
        /// again. → [`HostRet::ClientMediaEncoders`](crate::HostRet::ClientMediaEncoders):
        /// `None` = still asking.
        ClientMediaEncoders {
            refresh: bool,
        } => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: start writing a media file at `path`: the mod names the
        /// container, each track's codec and every option. The file appears only
        /// once complete. → [`HostRet::Ticket`](crate::HostRet::Ticket): the media
        /// file's id; every open is accepted, and waits `Queued` while the
        /// encoders are all busy.
        ClientMediaOpen {
            scope: ClientStorageScope,
            path: String,
            container: String,
            video: Option<ClientMediaVideo>,
            audio: Option<ClientMediaAudio>,
            options: Vec<(String, String)>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: one RGBA8 frame at the video's size.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = not taken now (the
        /// encoder is behind, the file is queued, failed or closed); push again
        /// later.
        ClientMediaPushFrame {
            media: u64,
            #[serde(with = "serde_bytes")]
            rgba: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: f32 LE interleaved samples at the audio's rate and channels.
        /// → [`HostRet::Bool`](crate::HostRet::Bool), as
        /// [`ClientMediaPushFrame`](Self::ClientMediaPushFrame).
        ClientMediaPushAudio {
            media: u64,
            #[serde(with = "serde_bytes")]
            pcm: Vec<u8>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: no more input; the file finishes in the background.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientMediaClose {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: stop now and leave nothing. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientMediaAbort {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: how a media file stands.
        /// → [`HostRet::ClientMediaState`](crate::HostRet::ClientMediaState):
        /// `None` = no such media file of this instance.
        ClientMediaState {
            media: u64,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
