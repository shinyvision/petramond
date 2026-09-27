use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, PoisonError};

use mod_api::capture::{
    ClientCapturedView, ClientEnvelope, ClientEventsPhase, ClientEventsReport, ClientFrameEnvelope,
    ClientPieceKind, ClientStateKey,
};

use super::body::CapturedCues;
use super::pieces::{self, Piece};
use super::state::{ColumnSnap, SectionSnap};
use super::view::ViewCue;
use super::writer::{frame_record, EnvelopeTarget, FrameSlot};
use crate::modding::client::files::{self, FileRef, WriterClaim, WriterKind};
use crate::net::protocol::{ServerToClient, TickUpdate};
use crate::worker::JobPool;
use crate::world::{column_key, section_key, FrameChanges};
use petramond_math::math::IVec3;
use petramond_world::chunk::SectionPos;

const FRAME_JOB_KEY: i64 = super::state::STATE_JOB_KEY / 2;

pub enum Applied {
    Section(SectionSnap),
    Column(ColumnSnap),
    Batch(Box<TickUpdate>),
    Message(ServerToClient),
}

pub struct FrameInput {
    pub world: u64,
    pub presented_tick: f64,
    pub applied: Vec<Applied>,
    pub cues: Option<CapturedCues>,
    pub view: Option<(ViewCue, ClientCapturedView)>,
    pub changes: FrameChanges,
    pub predicted: Arc<[IVec3]>,
}

impl FrameInput {
    fn happened(&self, last_view: Option<&ViewCue>) -> bool {
        let view = match (self.view.as_ref().map(|(cue, _)| cue), last_view) {
            (Some(cue), Some(last)) => !cue.events.is_empty() || !cue.presents_as(last),
            (None, None) => false,
            _ => true,
        };
        !self.applied.is_empty()
            || !self.changes.touched.is_empty()
            || !self.changes.removed.is_empty()
            || self.cues.is_some()
            || view
    }
}

#[derive(Default)]
struct Status {
    phase: Option<ClientEventsPhase>,
    error: Option<String>,
    frames: u64,
    written_through: u64,
    backlog_bytes: u64,
    in_flight: u64,
    claim: Option<WriterClaim>,
    file: Option<FileRef>,
    #[cfg(test)]
    finished: Vec<std::sync::mpsc::Sender<ClientEventsPhase>>,
}

impl Status {
    fn finish(&mut self, phase: ClientEventsPhase) {
        self.phase = Some(phase);
        self.claim = None;
        #[cfg(test)]
        for watcher in self.finished.drain(..) {
            let _ = watcher.send(phase);
        }
    }

    fn report(&self) -> ClientEventsReport {
        ClientEventsReport {
            phase: self.phase.unwrap_or(ClientEventsPhase::Running),
            error: self.error.clone(),
            frames: self.frames,
            written_through: self.written_through,
            backlog_bytes: self.backlog_bytes,
        }
    }

    fn finished(&self) -> bool {
        matches!(
            self.phase,
            Some(ClientEventsPhase::Ended | ClientEventsPhase::Failed)
        )
    }
}

type Shared = Arc<Mutex<Status>>;

fn lock(status: &Shared) -> std::sync::MutexGuard<'_, Status> {
    status.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Log {
    owner: String,
    file: FileRef,
    envelopes: Option<FileRef>,
    first_frame: u64,
    world: Option<u64>,
    seq: u64,
    status: Shared,
}

#[derive(Default)]
pub struct EventsLogs {
    logs: BTreeMap<u64, Log>,
    last_view: Option<ViewCue>,
}

impl EventsLogs {
    pub fn running(&self) -> bool {
        self.logs
            .values()
            .any(|log| lock(&log.status).phase.is_none())
    }

    pub fn begin(
        &mut self,
        id: u64,
        owner: &str,
        file: FileRef,
        envelopes: Option<FileRef>,
        first_frame: u64,
    ) -> Result<(), String> {
        let claim = files::claim(&file, WriterKind::Appends("an events log"))?;
        let status = Status {
            claim: Some(claim),
            file: Some(file.clone()),
            ..Default::default()
        };
        self.logs.insert(
            id,
            Log {
                owner: owner.to_owned(),
                file,
                envelopes,
                first_frame,
                world: None,
                seq: 0,
                status: Arc::new(Mutex::new(status)),
            },
        );
        self.last_view = None;
        Ok(())
    }

    pub fn owns(&self, id: u64, owner: &str) -> bool {
        self.logs.get(&id).is_some_and(|log| log.owner == owner)
    }

    pub fn end(&mut self, id: u64, why: Option<String>) {
        if let Some(log) = self.logs.get(&id) {
            let mut status = lock(&log.status);
            if status.phase.is_none() {
                status.phase = Some(ClientEventsPhase::Ending);
                if status.error.is_none() {
                    status.error = why;
                }
                settle(&log.status, status);
            }
        }
    }

    pub fn owners(&self) -> Vec<String> {
        let mut owners: Vec<String> = self
            .logs
            .values()
            .filter(|log| lock(&log.status).phase.is_none())
            .map(|log| log.owner.clone())
            .collect();
        owners.dedup();
        owners
    }

    pub fn end_all_of(&mut self, owner: &str, why: &str) {
        let ids: Vec<u64> = self
            .logs
            .iter()
            .filter(|(_, log)| log.owner == owner)
            .map(|(&id, _)| id)
            .collect();
        for id in ids {
            self.end(id, Some(why.to_owned()));
        }
    }

    #[cfg(test)]
    pub fn watch(&self, id: u64) -> std::sync::mpsc::Receiver<ClientEventsPhase> {
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(log) = self.logs.get(&id) {
            let mut status = lock(&log.status);
            match status.phase {
                Some(phase @ (ClientEventsPhase::Ended | ClientEventsPhase::Failed)) => {
                    let _ = tx.send(phase);
                }
                _ => status.finished.push(tx),
            }
        }
        rx
    }

    pub fn poll(&mut self, id: u64, owner: &str) -> Option<ClientEventsReport> {
        let log = self.logs.get(&id).filter(|log| log.owner == owner)?;
        let (report, finished) = {
            let status = lock(&log.status);
            (status.report(), status.finished())
        };
        if finished {
            self.logs.remove(&id);
        }
        Some(report)
    }

    pub fn submit(&mut self, frame: u64, input: FrameInput, jobs: &Arc<JobPool>) {
        let mut takers = Vec::new();
        for log in self.logs.values_mut() {
            if frame < log.first_frame || lock(&log.status).phase.is_some() {
                continue;
            }
            match log.world {
                None => log.world = Some(input.world),
                Some(world) if world != input.world => {
                    let mut status = lock(&log.status);
                    status.phase = Some(ClientEventsPhase::Ending);
                    status
                        .error
                        .get_or_insert_with(|| "the presented world changed".into());
                    settle(&log.status, status);
                    continue;
                }
                Some(_) => {}
            }
            takers.push(log);
        }
        if takers.is_empty() || !input.happened(self.last_view.as_ref()) {
            return;
        }
        self.last_view = input.view.as_ref().map(|(cue, _)| cue.clone());
        let mut slots = Vec::with_capacity(takers.len());
        for log in takers {
            let seq = log.seq;
            let status = Arc::clone(&log.status);
            let envelopes = log.envelopes.clone().map(EnvelopeTarget::issue);
            let reserved = FrameSlot::reserve(&log.file, envelopes, move |landed, _| {
                frame_landed(&status, landed)
            });
            match reserved {
                Ok((slot, fill)) => {
                    log.seq += 1;
                    let mut status = lock(&log.status);
                    status.frames += 1;
                    status.in_flight += 1;
                    slots.push((slot, fill, seq, Arc::clone(&log.status)));
                }
                Err(why) => fail(&log.status, why),
            }
        }
        if slots.is_empty() {
            return;
        }
        jobs.submit(FRAME_JOB_KEY, move || encode_frame(input, slots));
    }
}

fn frame_landed(status: &Shared, landed: Result<[u64; 2], String>) {
    let mut s = lock(status);
    s.in_flight = s.in_flight.saturating_sub(1);
    match landed {
        Ok([offset, len]) => {
            s.written_through = s.written_through.max(offset + len);
            s.backlog_bytes = s.backlog_bytes.saturating_sub(len);
        }
        Err(why) => {
            if !s.finished() {
                s.error.get_or_insert(why);
                s.finish(ClientEventsPhase::Failed);
            }
        }
    }
    settle(status, s);
}

fn fail(status: &Shared, why: String) {
    let mut s = lock(status);
    s.error.get_or_insert(why);
    s.finish(ClientEventsPhase::Failed);
}

fn settle(status: &Shared, mut s: std::sync::MutexGuard<'_, Status>) {
    if s.phase != Some(ClientEventsPhase::Ending) || s.in_flight > 0 {
        return;
    }
    let Some(file) = s.file.take() else {
        return;
    };
    drop(s);
    let status = Arc::clone(status);
    files::sync(&file, move |synced| {
        let mut s = lock(&status);
        match synced {
            Ok(()) => s.finish(ClientEventsPhase::Ended),
            Err(why) => {
                s.error.get_or_insert(why);
                s.finish(ClientEventsPhase::Failed);
            }
        }
    });
}

fn frame_pieces(input: &FrameInput) -> Result<(Vec<Piece>, Vec<u64>), String> {
    let predicted: rustc_hash::FxHashSet<SectionPos> = input
        .predicted
        .iter()
        .filter_map(|c| SectionPos::from_world(c.x, c.y, c.z))
        .collect();
    let mut out = Vec::new();
    let mut batches = Vec::new();
    for applied in &input.applied {
        match applied {
            Applied::Section(s) => {
                let payload = crate::world::detached_section_payload(&s.section, s.draws.clone());
                out.push(pieces::state(
                    section_key(s.pos),
                    predicted.contains(&s.pos),
                    &payload,
                )?);
            }
            Applied::Column(c) => {
                let payload = crate::world::detached_column_payload(
                    c.pos,
                    &c.column,
                    c.summaries.as_deref(),
                    c.halo.clone(),
                    c.deep_band_lo,
                );
                out.push(pieces::state(
                    column_key(c.pos),
                    predicted.iter().any(|s| s.chunk_pos() == c.pos),
                    &payload,
                )?);
            }
            Applied::Batch(t) => {
                batches.push(t.tick);
                out.extend(pieces::split_batch(t)?);
            }
            Applied::Message(msg) => {
                out.push(pieces::piece(
                    ClientPieceKind::Message,
                    [0; 12],
                    false,
                    msg,
                )?);
            }
        }
    }
    if let Some(cues) = &input.cues {
        out.push(pieces::piece(ClientPieceKind::Cues, [0; 12], false, cues)?);
    }
    if let Some((view, _)) = &input.view {
        out.push(pieces::piece(ClientPieceKind::View, [0; 12], false, view)?);
    }
    Ok((out, batches))
}

fn encode_frame(input: FrameInput, slots: Vec<(FrameSlot, super::writer::FrameFill, u64, Shared)>) {
    let (pieces, batches) = match frame_pieces(&input) {
        Ok(done) => done,
        Err(why) => {
            for (slot, _, _, _) in slots {
                slot.fail(why.clone());
            }
            return;
        }
    };
    let stated: rustc_hash::FxHashSet<ClientStateKey> =
        pieces.iter().filter_map(|p| p.info.key).collect();
    let touched: Vec<ClientStateKey> = input
        .changes
        .touched
        .iter()
        .copied()
        .filter(|k| {
            !stated.contains(k)
                && !matches!(
                    k,
                    ClientStateKey::Mob(_) | ClientStateKey::Item(_) | ClientStateKey::Player(_)
                )
        })
        .collect();
    for (slot, fill, seq, status) in slots {
        let envelope_of = |pieces| {
            ClientEnvelope::Frame(ClientFrameEnvelope {
                seq,
                revision: input.changes.revision,
                presented_tick: input.presented_tick,
                batches: batches.clone(),
                view: input.view.as_ref().map(|(_, v)| *v),
                pieces,
                touched: touched.clone(),
                removed: input.changes.removed.clone(),
            })
        };
        match frame_record(&pieces, envelope_of) {
            Ok((bytes, envelope)) => {
                lock(&status).backlog_bytes += bytes.len() as u64;
                slot.fill(fill, bytes, envelope);
            }
            Err(why) => slot.fail(why),
        }
    }
}
