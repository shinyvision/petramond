use mod_api::{
    ClientAudioInto, ClientCaptureInto, ClientCaptureStatus, ClientMediaCall, ClientStorageScope,
    HostRet,
};

use super::files::{checked_path, write_refusal};
use crate::modding::client::argv;
use crate::modding::client::files::{self, FileRef, WriterKind};
use crate::modding::client::media::{Capture, Destination, Media, MediaInput, Tap};
use crate::modding::client::state::ClientStoreData;

fn file_destination(
    client: &ClientStoreData,
    call: &str,
    scope: ClientStorageScope,
    path: &str,
) -> Result<FileRef, HostRet> {
    checked_path(call, path)?;
    if let Some(refusal) = write_refusal(client, scope) {
        return Err(HostRet::refused(refusal));
    }
    files::locate(client, scope, path)
        .ok_or_else(|| HostRet::invalid(format!("{call}: scope World needs a world")))
}

fn checked_size(client: &ClientStoreData, size: [u32; 2]) -> Result<(), HostRet> {
    let limits = client.presented.lock().frame_limits;
    let max_side = limits.map_or(u32::MAX, |l| l.max_side);
    let max_bytes = limits.map_or(u64::MAX, |l| l.max_bytes);
    let [w, h] = size;
    let side = 1..=max_side;
    if !side.contains(&w) || !side.contains(&h) || u64::from(w) * u64::from(h) * 4 > max_bytes {
        return Err(HostRet::invalid(format!(
            "ClientFrameCapture: a {w}x{h} frame is outside this device's limits (sides \
             1..={max_side}, at most {max_bytes} bytes)"
        )));
    }
    Ok(())
}

fn checked_names(
    container: &str,
    video: Option<&mod_api::ClientMediaVideo>,
    audio: Option<&mod_api::ClientMediaAudio>,
    options: &[(String, String)],
) -> Result<(), String> {
    if video.is_none() && audio.is_none() {
        return Err("a media file needs a video or an audio track".into());
    }
    let codecs = video.map(|v| v.codec.as_str()).into_iter();
    let codecs = codecs.chain(audio.map(|a| a.codec.as_str()));
    if let Some(name) = std::iter::once(container)
        .chain(codecs)
        .find(|name| !argv::is_token(name))
    {
        return Err(format!(
            "'{name}' is not a plain name (lowercase letters, digits, '_', '-')"
        ));
    }
    if let Some(v) = video {
        if v.width == 0 || v.height == 0 || v.fps[0] == 0 || v.fps[1] == 0 {
            return Err("video width, height and fps must not be 0".into());
        }
    }
    if let Some(a) = audio {
        if a.sample_rate == 0 || a.channels == 0 {
            return Err("audio sample rate and channels must not be 0".into());
        }
    }
    let every_option = options
        .iter()
        .chain(video.into_iter().flat_map(|v| &v.options))
        .chain(audio.into_iter().flat_map(|a| &a.options));
    for (name, value) in every_option {
        if let Some(problem) = argv::option_problem(name, value) {
            return Err(problem);
        }
    }
    Ok(())
}

pub(super) fn handle(client: &mut ClientStoreData, owner: &str, call: ClientMediaCall) -> HostRet {
    let desk = client.media.clone();
    match call {
        ClientMediaCall::ClientFrameCapture {
            source,
            size,
            when,
            advance,
            into,
        } => {
            if let Some(size) = size {
                if let Err(bug) = checked_size(client, size) {
                    return bug;
                }
            }
            let into = match into {
                ClientCaptureInto::Media(id) => {
                    let media = match desk.media_of(owner, id) {
                        Ok(media) => media,
                        Err(bug) => return HostRet::invalid(format!("ClientFrameCapture: {bug}")),
                    };
                    if media.video.is_none() {
                        return HostRet::invalid(
                            "ClientFrameCapture: that media file has no video track".into(),
                        );
                    }
                    if media.close_requested || media.input.done() || media.input.aborted() {
                        return HostRet::refused("that media file takes no more frames");
                    }
                    Destination::Media(id)
                }
                ClientCaptureInto::File { scope, path } => {
                    match file_destination(client, "ClientFrameCapture", scope, &path) {
                        Ok(file) => Destination::File(file),
                        Err(ret) => return ret,
                    }
                }
            };
            HostRet::Ticket(desk.arm_capture(Capture {
                owner: owner.to_owned(),
                source,
                size,
                when,
                advance,
                into,
                status: ClientCaptureStatus::Armed,
            }))
        }
        ClientMediaCall::ClientFrameCapturePoll { capture } => {
            match desk.poll_capture(owner, capture) {
                Ok(status) => HostRet::ClientCaptureStatus(status),
                Err(bug) => HostRet::invalid(format!("ClientFrameCapturePoll: {bug}")),
            }
        }
        ClientMediaCall::ClientFrameCancel { capture } => match desk.cancel_capture(owner, capture)
        {
            Ok(()) => HostRet::Unit,
            Err(bug) => HostRet::invalid(format!("ClientFrameCancel: {bug}")),
        },
        ClientMediaCall::ClientClockSet { step } => {
            if let Some(step) = step {
                if !(step.seconds.is_finite() && step.seconds > 0.0) {
                    return HostRet::invalid(
                        "ClientClockSet: a step must be finite and > 0".into(),
                    );
                }
            }
            HostRet::Bool(desk.set_clock(owner, step))
        }
        ClientMediaCall::ClientClockAdvance => {
            desk.advance_clock(owner);
            HostRet::Unit
        }
        ClientMediaCall::ClientAudioTap {
            sample_rate,
            channels,
            into,
        } => {
            if sample_rate == 0 || channels == 0 {
                return HostRet::invalid(
                    "ClientAudioTap: sample rate and channels must not be 0".into(),
                );
            }
            let into = match into {
                ClientAudioInto::Media(id) => {
                    let media = match desk.media_of(owner, id) {
                        Ok(media) => media,
                        Err(bug) => return HostRet::invalid(format!("ClientAudioTap: {bug}")),
                    };
                    match &media.audio {
                        Some(audio)
                            if audio.sample_rate == sample_rate && audio.channels == channels => {}
                        Some(audio) => {
                            return HostRet::invalid(format!(
                                "ClientAudioTap: a {sample_rate} Hz {channels}-channel tap \
                                 cannot feed a file declared at {} Hz and {} channels",
                                audio.sample_rate, audio.channels
                            ))
                        }
                        None => {
                            return HostRet::invalid(
                                "ClientAudioTap: that media file has no audio track".into(),
                            )
                        }
                    }
                    if media.close_requested || media.input.done() || media.input.aborted() {
                        return HostRet::refused("that media file takes no more sound");
                    }
                    Destination::Media(id)
                }
                ClientAudioInto::File { scope, path } => {
                    match file_destination(client, "ClientAudioTap", scope, &path) {
                        Ok(file) => Destination::File(file),
                        Err(ret) => return ret,
                    }
                }
            };
            if desk.mixes() == Some(false) {
                return HostRet::refused("this build has no sound mixer to tap");
            }
            HostRet::Ticket(desk.begin_tap(Tap {
                owner: owner.to_owned(),
                sample_rate,
                channels,
                into,
                data: mod_api::ClientAudioTapData {
                    started_at: None,
                    frames: 0,
                    ended: false,
                    error: None,
                },
                end_requested: false,
            }))
        }
        ClientMediaCall::ClientAudioTapState { tap } => {
            HostRet::ClientAudioTapState(desk.tap_state(owner, tap))
        }
        ClientMediaCall::ClientAudioTapEnd { tap } => match desk.end_tap(owner, tap) {
            Ok(()) => HostRet::Unit,
            Err(bug) => HostRet::invalid(format!("ClientAudioTapEnd: {bug}")),
        },
        ClientMediaCall::ClientMediaEncoders { refresh } => {
            HostRet::ClientMediaEncoders(desk.encoders(refresh))
        }
        ClientMediaCall::ClientMediaOpen {
            scope,
            path,
            container,
            video,
            audio,
            options,
        } => {
            if let Err(bug) = checked_path("ClientMediaOpen", &path) {
                return bug;
            }
            if let Err(why) = checked_names(&container, video.as_ref(), audio.as_ref(), &options) {
                return HostRet::invalid(format!("ClientMediaOpen: {why}"));
            }
            let file = match file_destination(client, "ClientMediaOpen", scope, &path) {
                Ok(file) => file,
                Err(ret) => return ret,
            };
            if let Some(refusal) = desk.open_refusal(&container, video.as_ref(), audio.as_ref()) {
                return HostRet::refused(refusal);
            }
            let claim = match files::claim(&file, WriterKind::Exclusive("a media file")) {
                Ok(claim) => claim,
                Err(in_use) => return HostRet::refused(in_use),
            };
            let input =
                std::sync::Arc::new(MediaInput::new(file, claim, video.as_ref(), audio.as_ref()));
            let media = Media {
                owner: owner.to_owned(),
                container,
                video,
                audio,
                options,
                input,
                close_requested: false,
            };
            HostRet::Ticket(desk.open_media(media))
        }
        ClientMediaCall::ClientMediaPushFrame { media, rgba } => {
            let media = match desk.media_of(owner, media) {
                Ok(media) => media,
                Err(bug) => return HostRet::invalid(format!("ClientMediaPushFrame: {bug}")),
            };
            match media.input.frame_bytes() {
                None => {
                    HostRet::invalid("ClientMediaPushFrame: the file has no video track".into())
                }
                Some(want) if want != rgba.len() => HostRet::invalid(format!(
                    "ClientMediaPushFrame: a frame of {} bytes; this file's frames are {want}",
                    rgba.len()
                )),
                Some(_) => HostRet::Bool(!media.close_requested && media.input.push_frame(rgba)),
            }
        }
        ClientMediaCall::ClientMediaPushAudio { media, pcm } => {
            let media = match desk.media_of(owner, media) {
                Ok(media) => media,
                Err(bug) => return HostRet::invalid(format!("ClientMediaPushAudio: {bug}")),
            };
            match media.input.audio_frame_bytes() {
                None => {
                    HostRet::invalid("ClientMediaPushAudio: the file has no audio track".into())
                }
                Some(frame) if !pcm.len().is_multiple_of(frame) => HostRet::invalid(format!(
                    "ClientMediaPushAudio: {} bytes are not whole sample frames of {frame} bytes",
                    pcm.len()
                )),
                Some(_) => HostRet::Bool(!media.close_requested && media.input.push_audio(pcm)),
            }
        }
        ClientMediaCall::ClientMediaClose { media } => match desk.close_media(owner, media) {
            Ok(()) => HostRet::Unit,
            Err(bug) => HostRet::invalid(format!("ClientMediaClose: {bug}")),
        },
        ClientMediaCall::ClientMediaAbort { media } => match desk.abort_media(owner, media) {
            Ok(()) => HostRet::Unit,
            Err(bug) => HostRet::invalid(format!("ClientMediaAbort: {bug}")),
        },
        ClientMediaCall::ClientMediaState { media } => HostRet::ClientMediaState(
            desk.media_of(owner, media)
                .ok()
                .map(|media| Box::new(media.input.state())),
        ),
    }
}
