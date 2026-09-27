//! The desk between a client mod's frame, clock, tap and media calls and the
//! client that carries them out.
//!
//! A host call runs inside a mod dispatch, where the renderer, the audio
//! engine and the encoder are out of reach. So a call validates, answers from
//! the desk, and leaves the rest here: an armed capture, a clock claim, a tap,
//! a media file waiting to start. The client carries each out at its next
//! frame and writes back where it stands. One desk per runtime, shared by
//! every mod in it; each entry remembers the mod that made it, so no mod
//! names another's.
//!
//! A media file's bytes cross from the frame to the encoder's writer thread
//! through its [`MediaInput`], which is also where the encoder publishes its
//! progress, so a push is answered from the encoder's own backpressure at the
//! call.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use mod_api::{
    ClientAudioTapData, ClientCaptureSource, ClientCaptureStatus, ClientCaptureWhen,
    ClientMediaAudio, ClientMediaCapabilities, ClientMediaFailure, ClientMediaPhase,
    ClientMediaStateData, ClientMediaVideo, ClientStep,
};

use super::files::{self, issue_id, FileRef, WriterClaim};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panic while holding the desk leaves plain data behind; keep going.
    m.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Where a capture's pixels or a tap's samples go.
#[derive(Clone, Debug)]
pub enum Destination {
    /// Appended to this file through its queue.
    File(FileRef),
    /// Handed to this open media file.
    Media(u64),
}

/// One armed or taken frame capture.
#[derive(Clone, Debug)]
pub struct Capture {
    pub owner: String,
    pub source: ClientCaptureSource,
    pub size: Option<[u32; 2]>,
    pub when: ClientCaptureWhen,
    pub advance: bool,
    pub into: Destination,
    pub status: ClientCaptureStatus,
}

/// One tap on the world's sound.
#[derive(Clone, Debug)]
pub struct Tap {
    pub owner: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub into: Destination,
    pub data: ClientAudioTapData,
    /// The mod ended it; the client delivers what it holds, then ends it.
    pub end_requested: bool,
}

/// One media file: what the mod asked for, and the input its encoder reads.
#[derive(Clone)]
pub struct Media {
    pub owner: String,
    pub container: String,
    pub video: Option<ClientMediaVideo>,
    pub audio: Option<ClientMediaAudio>,
    pub options: Vec<(String, String)>,
    pub input: Arc<MediaInput>,
    /// The mod closed it; the client hands it the captures still on their
    /// way back, then closes its input.
    pub close_requested: bool,
}

/// The stepped clock's holder and step.
#[derive(Clone, Debug, PartialEq)]
pub struct ClockClaim {
    pub holder: String,
    pub step: ClientStep,
}

#[derive(Default)]
struct Desk {
    captures: BTreeMap<u64, Capture>,
    taps: BTreeMap<u64, Tap>,
    media: BTreeMap<u64, Media>,
    clock: Option<ClockClaim>,
    /// `ClientClockAdvance` calls by the holder since the client last took
    /// them.
    advances: u64,
    encoders: Option<ClientMediaCapabilities>,
    /// What a mod asked of the machine since the client last looked.
    probe: ProbeRequest,
    /// Whether this build mixes sound a tap can take; `None` = not said yet.
    mixes: Option<bool>,
}

/// What a mod asked of the machine's encoder since the client last looked.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ProbeRequest {
    #[default]
    None,
    /// What can it write? (The answer the client already has will do.)
    Ask,
    /// Ask the machine again.
    Refresh,
}

/// Why a call naming `id` is the mod's bug.
fn never_issued(what: &str, id: u64) -> String {
    format!("{what} {id} was never issued to this instance")
}

#[derive(Clone, Default)]
pub struct MediaDesk(Arc<Mutex<Desk>>);

impl MediaDesk {
    fn desk(&self) -> MutexGuard<'_, Desk> {
        lock(&self.0)
    }

    // --- captures ---

    pub fn arm_capture(&self, capture: Capture) -> u64 {
        let id = issue_id();
        self.desk().captures.insert(id, capture);
        id
    }

    /// How capture `id` of `owner` stands; `Err` = never issued. A capture
    /// is forgotten once answered as `Delivered` or `Failed`.
    pub fn poll_capture(&self, owner: &str, id: u64) -> Result<ClientCaptureStatus, String> {
        let mut desk = self.desk();
        let status = match desk.captures.get(&id) {
            Some(capture) if capture.owner == owner => capture.status.clone(),
            _ => return Err(never_issued("capture", id)),
        };
        if matches!(
            status,
            ClientCaptureStatus::Delivered { .. } | ClientCaptureStatus::Failed { .. }
        ) {
            desk.captures.remove(&id);
        }
        Ok(status)
    }

    /// Disarm capture `id` while it is armed; a taken one goes on.
    pub fn cancel_capture(&self, owner: &str, id: u64) -> Result<(), String> {
        let mut desk = self.desk();
        match desk.captures.get(&id) {
            Some(capture) if capture.owner == owner => {
                if capture.status == ClientCaptureStatus::Armed {
                    desk.captures.remove(&id);
                }
                Ok(())
            }
            _ => Err(never_issued("capture", id)),
        }
    }

    /// Every armed capture, oldest first.
    pub fn armed_captures(&self) -> Vec<(u64, Capture)> {
        self.desk()
            .captures
            .iter()
            .filter(|(_, c)| c.status == ClientCaptureStatus::Armed)
            .map(|(&id, c)| (id, c.clone()))
            .collect()
    }

    /// Whether any capture is armed.
    pub fn capture_armed(&self) -> bool {
        self.desk()
            .captures
            .values()
            .any(|c| c.status == ClientCaptureStatus::Armed)
    }

    /// The client's news of capture `id`; inert once it was forgotten.
    pub fn set_capture_status(&self, id: u64, status: ClientCaptureStatus) {
        if let Some(capture) = self.desk().captures.get_mut(&id) {
            capture.status = status;
        }
    }

    // --- the stepped clock ---

    /// `Some(step)` claims the clock for `owner` (or changes its step);
    /// `None` releases it. `false` = another mod holds it.
    pub fn set_clock(&self, owner: &str, step: Option<ClientStep>) -> bool {
        let mut desk = self.desk();
        if desk.clock.as_ref().is_some_and(|c| c.holder != owner) {
            return false;
        }
        match step {
            Some(step) => {
                desk.clock = Some(ClockClaim {
                    holder: owner.to_owned(),
                    step,
                });
            }
            None => {
                desk.clock = None;
                desk.advances = 0;
            }
        }
        true
    }

    /// The holder steps the clock on the next frame; anyone else is ignored.
    pub fn advance_clock(&self, owner: &str) {
        let mut desk = self.desk();
        if desk.clock.as_ref().is_some_and(|c| c.holder == owner) {
            desk.advances += 1;
        }
    }

    pub fn clock(&self) -> Option<ClockClaim> {
        self.desk().clock.clone()
    }

    pub fn take_advances(&self) -> u64 {
        std::mem::take(&mut self.desk().advances)
    }

    // --- taps ---

    pub fn begin_tap(&self, tap: Tap) -> u64 {
        let id = issue_id();
        self.desk().taps.insert(id, tap);
        id
    }

    /// How tap `id` of `owner` stands; `None` = no such tap. An ended tap is
    /// forgotten once its state has been read.
    pub fn tap_state(&self, owner: &str, id: u64) -> Option<ClientAudioTapData> {
        let mut desk = self.desk();
        let data = match desk.taps.get(&id) {
            Some(tap) if tap.owner == owner => tap.data.clone(),
            _ => return None,
        };
        if data.ended {
            desk.taps.remove(&id);
        }
        Some(data)
    }

    pub fn end_tap(&self, owner: &str, id: u64) -> Result<(), String> {
        match self.desk().taps.get_mut(&id) {
            Some(tap) if tap.owner == owner => {
                tap.end_requested = true;
                Ok(())
            }
            _ => Err(never_issued("tap", id)),
        }
    }

    /// Every tap, by id.
    pub fn taps(&self) -> Vec<(u64, Tap)> {
        self.desk()
            .taps
            .iter()
            .map(|(&id, t)| (id, t.clone()))
            .collect()
    }

    /// The client's news of tap `id`.
    pub fn update_tap(&self, id: u64, update: impl FnOnce(&mut ClientAudioTapData)) {
        if let Some(tap) = self.desk().taps.get_mut(&id) {
            update(&mut tap.data);
        }
    }

    // --- media files ---

    /// What this machine's encoder can write; `None` while it is being asked.
    /// Asking starts the probe when nobody has yet; `refresh` asks again.
    pub fn encoders(&self, refresh: bool) -> Option<ClientMediaCapabilities> {
        let mut desk = self.desk();
        if refresh {
            desk.encoders = None;
            desk.probe = ProbeRequest::Refresh;
        } else if desk.encoders.is_none() && desk.probe == ProbeRequest::None {
            desk.probe = ProbeRequest::Ask;
        }
        desk.encoders.clone()
    }

    /// Why `open` is refused, if it is: no encoder, the probe has not
    /// answered, or a name this machine lacks. (The file store refuses a
    /// path in use.)
    pub fn open_refusal(
        &self,
        container: &str,
        video: Option<&ClientMediaVideo>,
        audio: Option<&ClientMediaAudio>,
    ) -> Option<String> {
        let mut desk = self.desk();
        let Some(encoders) = desk.encoders.clone() else {
            if desk.probe == ProbeRequest::None {
                desk.probe = ProbeRequest::Ask;
            }
            return Some(
                "this machine is still being asked what it can encode; ask again once \
                 ClientMediaEncoders answers"
                    .into(),
            );
        };
        if encoders.encoder.is_none() {
            return Some(NO_ENCODER.into());
        }
        let lacking: Vec<&str> = std::iter::once((&encoders.containers, container))
            .chain(video.map(|v| (&encoders.video_codecs, v.codec.as_str())))
            .chain(audio.map(|a| (&encoders.audio_codecs, a.codec.as_str())))
            .filter(|(have, name)| !have.iter().any(|h| h == name))
            .map(|(_, name)| name)
            .collect();
        if !lacking.is_empty() {
            return Some(format!(
                "this machine's encoder cannot write {}",
                lacking.join(", ")
            ));
        }
        None
    }

    pub fn open_media(&self, media: Media) -> u64 {
        let id = issue_id();
        self.desk().media.insert(id, media);
        id
    }

    fn owned_media(&self, owner: &str, id: u64) -> Result<Media, String> {
        match self.desk().media.get(&id) {
            Some(media) if media.owner == owner => Ok(media.clone()),
            _ => Err(never_issued("media file", id)),
        }
    }

    /// Media file `id` of `owner`: `Err` = never issued.
    pub fn media_of(&self, owner: &str, id: u64) -> Result<Media, String> {
        self.owned_media(owner, id)
    }

    pub fn close_media(&self, owner: &str, id: u64) -> Result<(), String> {
        let mut desk = self.desk();
        match desk.media.get_mut(&id) {
            Some(media) if media.owner == owner => {
                media.close_requested = true;
                Ok(())
            }
            _ => Err(never_issued("media file", id)),
        }
    }

    pub fn abort_media(&self, owner: &str, id: u64) -> Result<(), String> {
        self.owned_media(owner, id)?.input.abort();
        Ok(())
    }

    /// Every media file, by id.
    pub fn media(&self) -> Vec<(u64, Media)> {
        self.desk()
            .media
            .iter()
            .map(|(&id, m)| (id, m.clone()))
            .collect()
    }

    // --- the client's side ---

    /// What a mod asked of the machine since the last take.
    pub fn take_probe_request(&self) -> ProbeRequest {
        std::mem::take(&mut self.desk().probe)
    }

    /// Whether the desk has no answer from the machine yet.
    pub fn lacks_encoders(&self) -> bool {
        self.desk().encoders.is_none()
    }

    pub fn publish_encoders(&self, encoders: ClientMediaCapabilities) {
        self.desk().encoders = Some(encoders);
    }

    pub fn publish_mixes(&self, mixes: bool) {
        self.desk().mixes = Some(mixes);
    }

    pub fn mixes(&self) -> Option<bool> {
        self.desk().mixes
    }

    /// Drop everything `owner` made (it stopped running): armed captures are
    /// disarmed, its taps end, its media files abort and its clock claim goes.
    pub fn forget_owner(&self, owner: &str, why: &str) {
        let mut desk = self.desk();
        desk.captures.retain(|_, c| c.owner != owner);
        for tap in desk.taps.values_mut().filter(|t| t.owner == owner) {
            tap.data.ended = true;
            tap.data.error.get_or_insert_with(|| why.to_owned());
        }
        for media in desk.media.values().filter(|m| m.owner == owner) {
            media.input.abort();
        }
        if desk.clock.as_ref().is_some_and(|c| c.holder == owner) {
            desk.clock = None;
            desk.advances = 0;
        }
    }

    /// Every mod with anything on the desk.
    pub fn owners(&self) -> Vec<String> {
        let desk = self.desk();
        let mut owners: Vec<String> = desk
            .captures
            .values()
            .map(|c| c.owner.clone())
            .chain(
                desk.taps
                    .values()
                    .filter(|t| !t.data.ended)
                    .map(|t| t.owner.clone()),
            )
            .chain(
                desk.media
                    .values()
                    .filter(|m| !m.input.done())
                    .map(|m| m.owner.clone()),
            )
            .chain(desk.clock.iter().map(|c| c.holder.clone()))
            .collect();
        owners.sort();
        owners.dedup();
        owners
    }
}

/// What a player reads when the machine has no encoder.
pub const NO_ENCODER: &str = "no media encoder was found: install ffmpeg (on PATH, or name it \
                              with the PETRAMOND_FFMPEG environment variable)";

/// What a media file's input holds and how its encoder stands.
struct InputState {
    frames: VecDeque<Vec<u8>>,
    audio: VecDeque<Vec<u8>>,
    closed: bool,
    aborted: bool,
    state: ClientMediaStateData,
}

/// What the writer is to do next.
pub enum MediaWork {
    Frame(Vec<u8>),
    Audio(Vec<u8>),
    /// No more input: finish the file.
    Finish,
    /// Stop now and leave nothing.
    Abort,
}

/// A media file's input, between the frame (pushes, delivered captures and
/// tap samples) and its encoder's writer thread.
pub struct MediaInput {
    /// Where the finished file goes.
    target: PathBuf,
    /// The file in its bucket, and the claim that keeps every other writer
    /// off it until the file is finished or has failed; `None` = a file
    /// outside any bucket (a test's).
    stored: Mutex<Option<(FileRef, WriterClaim)>>,
    /// Makes this file's partial names its own.
    serial: u64,
    frame_bytes: Option<usize>,
    audio_frame_bytes: Option<usize>,
    inner: Mutex<InputState>,
    wake: Condvar,
}

impl MediaInput {
    /// The input of `file`, held by `claim`, with `video` and `audio`
    /// tracks: `Queued` until an encoder takes it.
    pub fn new(
        file: FileRef,
        claim: WriterClaim,
        video: Option<&ClientMediaVideo>,
        audio: Option<&ClientMediaAudio>,
    ) -> Self {
        let mut input = Self::at(file.path(), video, audio);
        input.stored = Mutex::new(Some((file, claim)));
        input
    }

    /// The input of a file at `target`, outside any bucket.
    pub fn at(
        target: PathBuf,
        video: Option<&ClientMediaVideo>,
        audio: Option<&ClientMediaAudio>,
    ) -> Self {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        Self {
            target,
            stored: Mutex::new(None),
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            frame_bytes: video.map(|v| v.width as usize * v.height as usize * 4),
            audio_frame_bytes: audio.map(|a| usize::from(a.channels) * 4),
            inner: Mutex::new(InputState {
                frames: VecDeque::new(),
                audio: VecDeque::new(),
                closed: false,
                aborted: false,
                state: ClientMediaStateData {
                    phase: ClientMediaPhase::Queued,
                    frames_in: 0,
                    frames_encoded: 0,
                    audio_in: 0,
                    bytes: 0,
                    failure: None,
                    error: None,
                },
            }),
            wake: Condvar::new(),
        }
    }

    fn inner(&self) -> MutexGuard<'_, InputState> {
        lock(&self.inner)
    }

    /// Where the finished file goes.
    pub fn target(&self) -> &Path {
        &self.target
    }

    /// A hidden intermediate beside the target, unique to this file:
    /// `.<leaf>.<pid>-<n>.<kind>.partial`, or `.<leaf>.<pid>-<n>.partial`
    /// for the finished file before it moves into place.
    pub fn partial(&self, kind: Option<&str>) -> PathBuf {
        let leaf = self
            .target
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let kind = kind.map(|k| format!("{k}.")).unwrap_or_default();
        self.target.with_file_name(format!(
            ".{leaf}.{}-{}.{kind}partial",
            std::process::id(),
            self.serial
        ))
    }

    /// Move the finished partial into place, replacing whatever file was
    /// there: through the file store (after every write queued to the path,
    /// synced first) for a bucket's file. `Ok` = the finished file's size.
    pub fn place(&self) -> Result<u64, String> {
        let partial = self.partial(None);
        let stored = lock(&self.stored).take();
        match stored {
            Some((file, claim)) => {
                drop(claim);
                let name = partial.file_name().unwrap_or_default().to_string_lossy();
                let from = file.sibling(&match file.rel().rsplit_once('/') {
                    Some((dir, _)) => format!("{dir}/{name}"),
                    None => name.into_owned(),
                });
                let (tx, rx) = std::sync::mpsc::channel();
                files::rename(&from, &file, move |done| {
                    let _ = tx.send(done);
                })?;
                rx.recv()
                    .map_err(|_| "the file store stopped".to_owned())??;
            }
            None => std::fs::rename(&partial, &self.target).map_err(|e| {
                format!(
                    "the finished file could not be moved to {}: {e}",
                    self.target.display()
                )
            })?,
        }
        Ok(std::fs::metadata(&self.target).map_or(0, |m| m.len()))
    }

    /// Let other writers at the path again (the file failed).
    fn release(&self) {
        lock(&self.stored).take();
    }

    pub fn state(&self) -> ClientMediaStateData {
        self.inner().state.clone()
    }

    pub fn phase(&self) -> ClientMediaPhase {
        self.inner().state.phase
    }

    /// Finished or failed.
    pub fn done(&self) -> bool {
        matches!(
            self.phase(),
            ClientMediaPhase::Finished | ClientMediaPhase::Failed
        )
    }

    /// The bytes one frame must have; `None` = the file has no video.
    pub fn frame_bytes(&self) -> Option<usize> {
        self.frame_bytes
    }

    /// Whether a frame offered now would be taken: the file is open and no
    /// frame waits beyond the one its writer is handing to the pipe.
    pub fn takes_frame(&self) -> bool {
        let inner = self.inner();
        inner.state.phase == ClientMediaPhase::Open
            && !inner.closed
            && !inner.aborted
            && inner.frames.is_empty()
    }

    /// A mod's push: taken only when [`takes_frame`](Self::takes_frame).
    pub fn push_frame(&self, rgba: Vec<u8>) -> bool {
        if !self.takes_frame() {
            return false;
        }
        self.deliver_frame(rgba).is_ok()
    }

    /// A capture routed into the file: always queued while the file is
    /// open. `Err` = the frame is not the file's size (the file then fails).
    pub fn deliver_frame(&self, rgba: Vec<u8>) -> Result<(), String> {
        let mut inner = self.inner();
        if inner.aborted || matches!(inner.state.phase, ClientMediaPhase::Failed) {
            return Ok(());
        }
        if Some(rgba.len()) != self.frame_bytes {
            return Err(format!(
                "a frame of {} bytes reached a file whose frames are {} bytes",
                rgba.len(),
                self.frame_bytes.unwrap_or(0)
            ));
        }
        inner.state.frames_in += 1;
        inner.frames.push_back(rgba);
        drop(inner);
        self.wake.notify_all();
        Ok(())
    }

    /// A mod's push of samples: taken while the writer holds none it has not
    /// written, the file is open and no close was asked for.
    pub fn push_audio(&self, pcm: Vec<u8>) -> bool {
        {
            let inner = self.inner();
            if inner.state.phase != ClientMediaPhase::Open
                || inner.closed
                || inner.aborted
                || !inner.audio.is_empty()
            {
                return false;
            }
        }
        self.deliver_audio(pcm);
        true
    }

    /// Samples a tap routed into the file: always queued.
    pub fn deliver_audio(&self, pcm: Vec<u8>) {
        let Some(frame) = self.audio_frame_bytes else {
            return;
        };
        let mut inner = self.inner();
        if inner.aborted || matches!(inner.state.phase, ClientMediaPhase::Failed) {
            return;
        }
        inner.state.audio_in += (pcm.len() / frame) as u64;
        inner.audio.push_back(pcm);
        drop(inner);
        self.wake.notify_all();
    }

    /// The bytes of one sample frame; `None` = the file has no audio.
    pub fn audio_frame_bytes(&self) -> Option<usize> {
        self.audio_frame_bytes
    }

    /// No more input: the writer finishes the file once it has written what
    /// it holds.
    pub fn close(&self) {
        self.inner().closed = true;
        self.wake.notify_all();
    }

    pub fn closed(&self) -> bool {
        self.inner().closed
    }

    /// Stop now and leave nothing. A file that has not started fails at once.
    pub fn abort(&self) {
        let mut inner = self.inner();
        if matches!(
            inner.state.phase,
            ClientMediaPhase::Finished | ClientMediaPhase::Failed
        ) {
            return;
        }
        inner.aborted = true;
        inner.frames.clear();
        inner.audio.clear();
        let never_started = inner.state.phase == ClientMediaPhase::Queued;
        if never_started {
            inner.state.phase = ClientMediaPhase::Failed;
            inner.state.failure = Some(ClientMediaFailure::Aborted);
            inner.state.error = Some(ABORTED.into());
        }
        drop(inner);
        if never_started {
            self.release();
        }
        self.wake.notify_all();
    }

    pub fn aborted(&self) -> bool {
        self.inner().aborted
    }

    // --- the encoder's side ---

    /// The writer's next piece of work, waiting for one. Samples go first,
    /// so a frame blocked on the pipe never holds up the audio behind it.
    pub fn next_work(&self) -> MediaWork {
        let mut inner = self.inner();
        loop {
            if inner.aborted {
                return MediaWork::Abort;
            }
            if let Some(pcm) = inner.audio.pop_front() {
                return MediaWork::Audio(pcm);
            }
            if let Some(frame) = inner.frames.pop_front() {
                return MediaWork::Frame(frame);
            }
            if inner.closed {
                return MediaWork::Finish;
            }
            inner = self
                .wake
                .wait(inner)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }

    /// The encoder took a frame.
    pub fn frame_encoded(&self) {
        self.inner().state.frames_encoded += 1;
    }

    pub fn set_phase(&self, phase: ClientMediaPhase) {
        self.inner().state.phase = phase;
    }

    pub fn finished(&self, bytes: u64) {
        let mut inner = self.inner();
        inner.state.phase = ClientMediaPhase::Finished;
        inner.state.bytes = bytes;
    }

    pub fn failed(&self, failure: ClientMediaFailure, error: String) {
        let mut inner = self.inner();
        inner.state.phase = ClientMediaPhase::Failed;
        inner.state.failure = Some(failure);
        inner.state.error = Some(error);
        inner.frames.clear();
        inner.audio.clear();
        drop(inner);
        self.release();
        self.wake.notify_all();
    }
}

/// What an aborted media file reports.
pub const ABORTED: &str = "the media file was aborted";

#[cfg(test)]
mod tests {
    use super::*;

    fn video() -> ClientMediaVideo {
        ClientMediaVideo {
            codec: "libx264".into(),
            width: 2,
            height: 2,
            fps: [30, 1],
            options: Vec::new(),
        }
    }

    #[test]
    fn a_push_waits_on_the_writer_and_a_delivered_capture_never_does() {
        let input = MediaInput::at(PathBuf::from("/nowhere/x.mp4"), Some(&video()), None);
        assert!(
            !input.push_frame(vec![0; 16]),
            "a queued file takes nothing"
        );
        input.set_phase(ClientMediaPhase::Open);
        assert!(input.push_frame(vec![0; 16]));
        assert!(!input.push_frame(vec![0; 16]), "one frame waits, no more");
        input.deliver_frame(vec![0; 16]).unwrap();
        assert!(matches!(input.next_work(), MediaWork::Frame(_)));
        assert!(!input.takes_frame(), "a delivered capture waits behind it");
        assert!(matches!(input.next_work(), MediaWork::Frame(_)));
        assert!(input.takes_frame(), "nothing waits beyond the pipe");
        assert!(
            input.deliver_frame(vec![0; 12]).is_err(),
            "a wrong size fails"
        );
        input.close();
        assert!(matches!(input.next_work(), MediaWork::Finish));
        assert_eq!(input.state().frames_in, 2);
    }

    #[test]
    fn the_clock_has_one_holder_and_only_its_advances_count() {
        let desk = MediaDesk::default();
        let step = ClientStep {
            seconds: 0.5,
            seed: 1,
        };
        assert!(desk.set_clock("a", Some(step)));
        assert!(!desk.set_clock("b", Some(step)));
        assert!(!desk.set_clock("b", None), "a non-holder releases nothing");
        desk.advance_clock("b");
        desk.advance_clock("a");
        desk.advance_clock("a");
        assert_eq!(desk.take_advances(), 2);
        desk.forget_owner("a", "stopped");
        assert!(desk.clock().is_none() && desk.set_clock("b", Some(step)));
    }
}
