//! The app's half of frames, time and sound for client mods: it carries out
//! what the runtime's media desk holds (armed captures, the stepped clock,
//! taps, media files waiting for an encoder), routes each capture that comes
//! back from the GPU, and paces the frames the stepped clock holds.
//!
//! Stepped, presentation time moves only on an advance, and the world's
//! sound mixes offline on that timeline: exactly the seconds a frame moves
//! are pulled, at the start of the frame that moves them, after the sounds
//! of the frame before started. On the wall clock the world's sound stays on
//! the device and a tap copies it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use mod_api::{
    ClientCaptureSource, ClientCaptureStatus, ClientCaptureWhen, ClientMediaCapabilities,
    ClientMediaFailure, ClientMediaPhase,
};
use petramond::modding::client::files;
use petramond::modding::client::media::{
    ClockClaim, Destination, Media, MediaDesk, ProbeRequest, NO_ENCODER,
};
use petramond_audio::convert::FormatConverter;
use petramond_render::{CaptureRequest, CaptureSource, Captured, Renderer};

use crate::media::clock::PresentationClock;
use crate::media::encoder::{Encoder, EncoderSpec};
use crate::media::ffmpeg;

use super::{now_seconds, App};

/// The format the world mixes at offline when neither a device nor a tap
/// says one.
const FALLBACK_FORMAT: (u16, u32) = (2, 48_000);

/// A display refresh until the window says its own.
const DEFAULT_REFRESH: f64 = 1.0 / 60.0;

struct Probed {
    ffmpeg: Option<PathBuf>,
    capabilities: ClientMediaCapabilities,
}

/// A capture taken and on its way back from the GPU.
struct InFlight {
    desk: MediaDesk,
    into: Destination,
    time: f64,
}

/// A tap's conversion from the world's mix to what it asked for.
struct TapRoute {
    from: (u16, u32),
    converter: FormatConverter,
}

pub(super) struct MediaHost {
    pub(super) clock: PresentationClock,
    /// The claim the clock follows.
    claim: Option<ClockClaim>,
    probe: Option<Arc<OnceLock<Probed>>>,
    encoders: BTreeMap<u64, Encoder>,
    in_flight: BTreeMap<u64, InFlight>,
    taps: BTreeMap<u64, TapRoute>,
    /// The seed the world's sound mixes offline with, while it does.
    offline: Option<u64>,
    samples: Vec<f32>,
    converted: Vec<f32>,
    last_present: f64,
    /// Seconds per display refresh.
    refresh: f64,
    /// A capture was armed at the last look.
    capture_armed: bool,
}

impl Default for MediaHost {
    fn default() -> Self {
        Self {
            clock: PresentationClock::default(),
            claim: None,
            probe: None,
            encoders: BTreeMap::new(),
            in_flight: BTreeMap::new(),
            taps: BTreeMap::new(),
            offline: None,
            samples: Vec::new(),
            converted: Vec::new(),
            last_present: f64::NEG_INFINITY,
            refresh: DEFAULT_REFRESH,
            capture_armed: false,
        }
    }
}

impl MediaHost {
    /// What this machine can write, once asked. Tests never ask: the answer
    /// is this machine's, not the code's.
    fn probed(&self) -> Option<&Probed> {
        self.probe.as_ref()?.get()
    }

    /// Ask the machine again, unless a probe is already asking: one OS helper
    /// at a time.
    fn start_probe(&mut self) {
        if cfg!(test) || self.probe.as_ref().is_some_and(|cell| cell.get().is_none()) {
            return;
        }
        let cell = Arc::new(OnceLock::new());
        let fill = Arc::clone(&cell);
        let spawned = std::thread::Builder::new()
            .name("petramond-media-probe".into())
            .spawn(move || {
                let _turn = files::os_helper_turn();
                let ffmpeg = ffmpeg::find();
                let capabilities = ffmpeg::capabilities(ffmpeg.as_deref());
                let _ = fill.set(Probed {
                    ffmpeg,
                    capabilities,
                });
            });
        match spawned {
            Ok(_) => self.probe = Some(cell),
            Err(e) => log::error!("could not ask for a media encoder: {e}"),
        }
    }

    /// Answer every probe with `ffmpeg`, as though the machine had it and it
    /// could write `capabilities`.
    #[cfg(test)]
    pub(super) fn use_encoder_for_test(
        &mut self,
        ffmpeg: Option<PathBuf>,
        capabilities: ClientMediaCapabilities,
    ) {
        let cell = Arc::new(OnceLock::new());
        let _ = cell.set(Probed {
            ffmpeg,
            capabilities,
        });
        self.probe = Some(cell);
    }

    /// Stop what was aborted, and what nobody on a desk in `live` can close
    /// any more; forget the encoders that are done. A closed file whose desk
    /// is gone still finishes.
    fn tend_encoders(&mut self, live: &BTreeSet<u64>) {
        self.encoders.retain(|id, encoder| {
            if !live.contains(id) {
                encoder.abandon();
            }
            if encoder.aborted() {
                encoder.kill();
            }
            !encoder.done()
        });
    }

    #[cfg(test)]
    pub(super) fn last_present_for_test(&self) -> f64 {
        self.last_present
    }

    /// Whether any capture of this host is on its way into media file `id`.
    fn frames_on_their_way(&self, id: u64) -> bool {
        self.in_flight
            .values()
            .any(|f| matches!(f.into, Destination::Media(m) if m == id))
    }
}

fn f32_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}

impl App {
    /// "Now" for every presentation system: the wall, or the stepped
    /// timeline.
    pub(super) fn now(&self) -> f64 {
        self.media.clock.now(now_seconds())
    }

    /// The frame about to run moves a stepped clock: the host lifts its frame
    /// cap for it, since the next capture may be due at once.
    pub fn frame_uncapped(&self) -> bool {
        self.media
            .clock
            .stepped()
            .is_some_and(|clock| clock.pending() > 0)
    }

    /// The display's refresh rate, which paces a held frame's presents.
    pub fn set_display_refresh_hz(&mut self, hz: f64) {
        if hz.is_finite() && hz > 0.0 {
            self.media.refresh = 1.0 / hz;
        }
    }

    /// Whether a capture waits to be taken: a window-only screen then
    /// reaches the window alone (see `scene_screen`).
    pub(super) fn capture_armed(&self) -> bool {
        self.media.capture_armed
    }

    /// The desk this frame carries out: the session's runtime, or with no
    /// session the shell's.
    fn media_desk(&self) -> Option<MediaDesk> {
        self.client_mods_now()
            .map(|runtime| runtime.media_desk().clone())
    }

    /// Start a frame on the stepped clock: the seconds it moves (`None` on
    /// the wall clock). The world's sound for exactly those seconds is pulled
    /// here, after the previous frame's sounds started and before this one's.
    pub(super) fn step_media_clock(&mut self) -> Option<f64> {
        let desk = self.media_desk();
        let clock = self.media.clock.stepped_mut()?;
        if let Some(desk) = &desk {
            clock.advance(desk.take_advances());
        }
        let pending = clock.pending();
        if pending > 0 {
            let seconds = pending as f64 * clock.step_seconds();
            let from = clock.now();
            let mut samples = std::mem::take(&mut self.media.samples);
            samples.clear();
            self.sound.engine_mut().pull(seconds, &mut samples);
            if let (Some(desk), Some(format)) = (&desk, self.sound.engine().world_format()) {
                self.deliver_world_sound(desk, &samples, format, from);
            }
            self.media.samples = samples;
        }
        self.media
            .clock
            .stepped_mut()
            .map(|clock| clock.take_step())
    }

    /// Carry out what the desk holds, after the client mods' frame (where it
    /// was asked) and before the world ticks and plays its sounds.
    pub(super) fn drive_media(&mut self, renderer: &mut Renderer, wall_dt: f64) {
        let captured = renderer.take_captured();
        self.route_captures(captured);
        let Some(desk) = self.media_desk() else {
            self.media.tend_encoders(&BTreeSet::new());
            self.media.capture_armed = false;
            return;
        };
        desk.publish_mixes(self.sound.engine().mixes());
        self.drive_probe(&desk);
        if let Some(runtime) = self.client_mods_now() {
            for owner in desk.owners() {
                if !runtime.is_live(&owner) {
                    desk.forget_owner(&owner, "the mod stopped running");
                }
            }
        }
        self.follow_clock_claim(&desk);
        if let Some(clock) = self.media.clock.stepped_mut() {
            clock.advance(desk.take_advances());
        }
        self.route_world_sound(&desk, wall_dt);
        self.drive_media_files(&desk);
        self.media.capture_armed = desk.capture_armed();
    }

    fn drive_probe(&mut self, desk: &MediaDesk) {
        match desk.take_probe_request() {
            ProbeRequest::Refresh => self.media.start_probe(),
            ProbeRequest::Ask if self.media.probe.is_none() => self.media.start_probe(),
            _ => {}
        }
        if desk.lacks_encoders() {
            if let Some(probed) = self.media.probed() {
                desk.publish_encoders(probed.capabilities.clone());
            }
        }
    }

    /// Step the clock on the claim's step from the next frame, or give it
    /// back to the wall.
    fn follow_clock_claim(&mut self, desk: &MediaDesk) {
        let claim = desk.clock();
        if claim == self.media.claim {
            return;
        }
        let wall = now_seconds();
        match &claim {
            Some(claim) => self.media.clock.begin_stepped(wall, claim.step.seconds),
            None => self.media.clock.end_stepped(wall),
        }
        self.media.claim = claim;
    }

    /// Where the world's sound goes this frame, and the taps' share of it on
    /// the wall clock. Stepped, it mixes offline on the timeline (a device
    /// plays at the wall's pace). On the wall clock it stays on the device
    /// and a tap copies it; with no device it mixes offline at the wall's
    /// pace instead.
    fn route_world_sound(&mut self, desk: &MediaDesk, wall_dt: f64) {
        let taps = desk.taps();
        for (id, tap) in &taps {
            if tap.data.ended {
                self.media.taps.remove(id);
            } else if tap.end_requested {
                desk.update_tap(*id, |data| data.ended = true);
                self.media.taps.remove(id);
            } else if let Destination::Media(media) = tap.into {
                let taking = desk
                    .media()
                    .into_iter()
                    .any(|(m, file)| m == media && !file.close_requested && !file.input.done());
                if !taking {
                    desk.update_tap(*id, |data| {
                        data.ended = true;
                        data.error = Some("the media file stopped taking sound".into());
                    });
                }
            }
        }
        let tapped = taps.iter().any(|(_, t)| !t.data.ended && !t.end_requested);
        let stepped = self.media.claim.as_ref().map(|claim| claim.step.seed);
        let offline = match stepped {
            Some(seed) => Some(seed),
            None if tapped && !self.sound.engine().has_device() => {
                Some(self.media.offline.unwrap_or(0))
            }
            None => None,
        };
        if offline != self.media.offline {
            match offline {
                Some(seed) => {
                    let (channels, rate) = self
                        .sound
                        .engine()
                        .world_format()
                        .or_else(|| {
                            taps.iter()
                                .find(|(_, t)| !t.data.ended)
                                .map(|(_, t)| (t.channels, t.sample_rate))
                        })
                        .unwrap_or(FALLBACK_FORMAT);
                    self.sound.engine_mut().begin_offline(channels, rate, seed);
                }
                None => self.sound.engine_mut().end_offline(),
            }
            self.media.offline = offline;
        }
        let copied = stepped.is_none() && tapped && self.sound.engine().has_device();
        self.sound.engine_mut().set_world_copied(copied);
        if stepped.is_some() || !tapped {
            return;
        }
        let mut samples = std::mem::take(&mut self.media.samples);
        samples.clear();
        if self.media.offline.is_some() {
            self.sound.engine_mut().pull(wall_dt.max(0.0), &mut samples);
        } else {
            self.sound.engine_mut().drain_world_copy(&mut samples);
        }
        if let Some(format) = self.sound.engine().world_format() {
            let from = self.now() - wall_dt.max(0.0);
            self.deliver_world_sound(desk, &samples, format, from);
        }
        self.media.samples = samples;
    }

    /// Hand `samples` (interleaved at `format`, starting at presentation time
    /// `from`) to every running tap, each in its own format.
    fn deliver_world_sound(
        &mut self,
        desk: &MediaDesk,
        samples: &[f32],
        format: (u16, u32),
        from: f64,
    ) {
        if samples.is_empty() {
            return;
        }
        let media = desk.media();
        for (id, tap) in desk.taps() {
            if tap.data.ended || tap.end_requested {
                continue;
            }
            let route = self.media.taps.entry(id).or_insert_with(|| TapRoute {
                from: format,
                converter: FormatConverter::new(format, (tap.channels, tap.sample_rate)),
            });
            if route.from != format {
                *route = TapRoute {
                    from: format,
                    converter: FormatConverter::new(format, (tap.channels, tap.sample_rate)),
                };
            }
            let converted = &mut self.media.converted;
            converted.clear();
            let frames = route.converter.convert(samples, converted);
            if frames == 0 {
                continue;
            }
            let bytes = f32_bytes(converted);
            let refused = match &tap.into {
                Destination::Media(m) => match media.iter().find(|(id, _)| id == m) {
                    Some((_, file)) => {
                        file.input.deliver_audio(bytes);
                        None
                    }
                    None => Some("the media file is gone".to_owned()),
                },
                Destination::File(file) => {
                    let failed = desk.clone();
                    files::append(file, bytes, move |landed| {
                        if let Err(why) = landed {
                            failed.update_tap(id, |data| {
                                data.ended = true;
                                data.error = Some(why);
                            });
                        }
                    })
                    .err()
                }
            };
            desk.update_tap(id, |data| match refused {
                Some(why) => {
                    data.ended = true;
                    data.error = Some(why);
                }
                None => {
                    data.started_at.get_or_insert(from);
                    data.frames += frames as u64;
                }
            });
        }
    }

    /// Start queued media files in order while encoders are free, close the
    /// ones whose frames have all come back, and stop the aborted.
    fn drive_media_files(&mut self, desk: &MediaDesk) {
        let files = desk.media();
        let parallel = std::thread::available_parallelism().map_or(1, usize::from);
        let mut running = self.media.encoders.values().filter(|e| !e.done()).count();
        for (id, media) in &files {
            if media.input.phase() == ClientMediaPhase::Queued
                && !media.input.aborted()
                && running < parallel
            {
                self.start_encoder(*id, media);
                running += 1;
            }
            if media.close_requested
                && !media.input.closed()
                && !self.media.frames_on_their_way(*id)
            {
                media.input.close();
            }
        }
        for (id, capture) in desk.armed_captures() {
            let Destination::Media(media) = capture.into else {
                continue;
            };
            let open = files.iter().any(|(m, file)| {
                *m == media && !file.close_requested && !file.input.done() && !file.input.aborted()
            });
            if !open {
                desk.set_capture_status(
                    id,
                    ClientCaptureStatus::Failed {
                        why: "the media file closed before the frame was taken".into(),
                    },
                );
            }
        }
        let live: BTreeSet<u64> = files.iter().map(|(id, _)| *id).collect();
        self.media.tend_encoders(&live);
    }

    fn start_encoder(&mut self, id: u64, media: &Media) {
        let Some(ffmpeg) = self.media.probed().and_then(|p| p.ffmpeg.clone()) else {
            media
                .input
                .failed(ClientMediaFailure::EncoderFailed, NO_ENCODER.into());
            return;
        };
        let spec = EncoderSpec {
            ffmpeg,
            container: media.container.clone(),
            video: media.video.clone(),
            audio: media.audio.clone(),
            options: media.options.clone(),
        };
        log::info!("media file {} opens", media.input.target().display());
        self.media
            .encoders
            .insert(id, Encoder::start(spec, Arc::clone(&media.input)));
    }

    /// Hand every capture that came back to where it was going.
    fn route_captures(&mut self, captured: Vec<Captured>) {
        for Captured { id, frame } in captured {
            let Some(flight) = self.media.in_flight.remove(&id) else {
                continue;
            };
            let desk = flight.desk;
            let frame = match frame {
                Ok(frame) => frame,
                Err(why) => {
                    desk.set_capture_status(id, ClientCaptureStatus::Failed { why });
                    continue;
                }
            };
            let (time, width, height) = (flight.time, frame.width, frame.height);
            match flight.into {
                Destination::Media(m) => {
                    let Some((_, media)) = desk.media().into_iter().find(|(id, _)| *id == m) else {
                        desk.set_capture_status(
                            id,
                            ClientCaptureStatus::Failed {
                                why: "the media file is gone".into(),
                            },
                        );
                        continue;
                    };
                    let size = media.video.as_ref().map(|v| (v.width, v.height));
                    let status = if size != Some((width, height)) {
                        let (w, h) = size.unwrap_or_default();
                        let why = format!("a {width}x{height} frame reached a {w}x{h} file");
                        media
                            .input
                            .failed(ClientMediaFailure::EncoderFailed, why.clone());
                        ClientCaptureStatus::Failed { why }
                    } else {
                        match media.input.deliver_frame(frame.rgba) {
                            Ok(()) => ClientCaptureStatus::Delivered {
                                time,
                                width,
                                height,
                                range: None,
                            },
                            Err(why) => ClientCaptureStatus::Failed { why },
                        }
                    };
                    desk.set_capture_status(id, status);
                }
                Destination::File(file) => {
                    let landed = desk.clone();
                    let appended = files::append(&file, frame.rgba, move |result| {
                        landed.set_capture_status(
                            id,
                            match result {
                                Ok(range) => ClientCaptureStatus::Delivered {
                                    time,
                                    width,
                                    height,
                                    range: Some(range),
                                },
                                Err(why) => ClientCaptureStatus::Failed { why },
                            },
                        );
                    });
                    if let Err(why) = appended {
                        desk.set_capture_status(id, ClientCaptureStatus::Failed { why });
                    }
                }
            }
        }
    }

    /// Draw the frame `render` built, taking every armed capture whose
    /// moment has come. Stepped, a frame presents only once a display refresh
    /// has passed since the last present, and a frame with nothing to
    /// present or capture draws nothing.
    ///
    /// `Settled` is this frame's `world_settled`: after its mesh upload, the
    /// replica has no section waiting to be (re)meshed or relit, no mesh job
    /// or light bake in flight and no stream work pending, and the renderer
    /// has no column queued for upload.
    pub(super) fn present_frame(&mut self, renderer: &mut Renderer) {
        let wall = now_seconds();
        let present = self.media.clock.stepped().is_none()
            || wall - self.media.last_present >= self.media.refresh;
        let desk = self.media_desk();
        let mut requests = Vec::new();
        let mut armed = Vec::new();
        if let Some(desk) = &desk {
            let settled = self.presented_view.world_settled;
            let files = desk.media();
            let mut fed = BTreeSet::new();
            for (id, capture) in desk.armed_captures() {
                if capture.when == ClientCaptureWhen::Settled && !settled {
                    continue;
                }
                if let Destination::Media(m) = capture.into {
                    let takes = files
                        .iter()
                        .any(|(id, file)| *id == m && file.input.takes_frame());
                    // One frame per file per draw: the next waits on the
                    // encoder's own pipe.
                    if !takes || !fed.insert(m) {
                        continue;
                    }
                }
                requests.push(CaptureRequest {
                    id,
                    source: match capture.source {
                        ClientCaptureSource::Scene => CaptureSource::Scene,
                        ClientCaptureSource::World => CaptureSource::World,
                    },
                    size: capture.size.map(|[w, h]| (w, h)),
                });
                armed.push((id, capture));
            }
        }
        if requests.is_empty() && !present {
            return;
        }
        let taken = renderer.draw_frame(&requests, present);
        if present {
            self.media.last_present = wall;
        }
        if let Some(desk) = desk {
            let now = self.now();
            for (id, capture) in armed.into_iter().filter(|(id, _)| taken.contains(id)) {
                desk.set_capture_status(id, ClientCaptureStatus::Taken { time: now });
                let holds = self
                    .media
                    .claim
                    .as_ref()
                    .is_some_and(|claim| claim.holder == capture.owner);
                if capture.advance && holds {
                    if let Some(clock) = self.media.clock.stepped_mut() {
                        clock.advance(1);
                    }
                }
                self.media.in_flight.insert(
                    id,
                    InFlight {
                        desk: desk.clone(),
                        into: capture.into,
                        time: now,
                    },
                );
            }
            self.media.capture_armed = desk.capture_armed();
        }
        let captured = renderer.take_captured();
        self.route_captures(captured);
    }
}
