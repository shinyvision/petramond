//! Records reach a mod's file through that file's queue in the mod file store, in the same turn
//! as the call, while their bytes are still encoding off the frame.
//!
//! Frame records are small and encoded whole: one slot, one write with the final head.
//!
//! State records can be hundreds of MB, so they're streamed, never buffered whole. The call
//! reserves one slot per chunk plus one for the envelope. The first slot opens with the head,
//! incomplete. Chunks fill their slots as the job pool encodes them, and the queue writes them in
//! order as they land.
//!
//! After all chunks land, the envelope records where each piece landed relative to the record
//! start. After the envelope lands, the head is rewritten in place with the final length and CRC.
//! Only then does the ticket answer and the record get appended to the envelope file.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

use mod_api::capture::{
    crc32, write_envelope_entry, write_record_head, CaptureRecordHead, CaptureRecordKind,
    ClientEnvelope, ClientEnvelopeEntry, ClientPieceInfo, CAPTURE_RECORD_HEAD_LEN,
};
use mod_api::ClientFileAnswer;

use crate::modding::client::files::{self, FileRef, RecordSlot};

use super::pieces::Piece;

pub struct EnvelopeTarget {
    turn: Option<(FileRef, u64)>,
}

#[derive(Default)]
struct EnvelopeQueue {
    next_issue: u64,
    next_write: u64,
    ready: BTreeMap<u64, Option<Vec<u8>>>,
}

fn envelope_queues() -> MutexGuard<'static, HashMap<PathBuf, EnvelopeQueue>> {
    static QUEUES: OnceLock<Mutex<HashMap<PathBuf, EnvelopeQueue>>> = OnceLock::new();
    QUEUES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

impl EnvelopeTarget {
    pub fn issue(file: FileRef) -> Self {
        let mut queues = envelope_queues();
        let queue = queues.entry(file.path()).or_default();
        let seq = queue.next_issue;
        queue.next_issue += 1;
        Self {
            turn: Some((file, seq)),
        }
    }

    fn settle(mut self, entry: Option<Vec<u8>>) {
        self.settle_turn(entry);
    }

    fn settle_turn(&mut self, entry: Option<Vec<u8>>) {
        let Some((file, seq)) = self.turn.take() else {
            return;
        };
        let mut queues = envelope_queues();
        let path = file.path();
        let Some(queue) = queues.get_mut(&path) else {
            return;
        };
        queue.ready.insert(seq, entry);
        while let Some(entry) = queue.ready.remove(&queue.next_write) {
            queue.next_write += 1;
            let Some(bytes) = entry else {
                continue;
            };
            let target = file.clone();
            if let Err(why) = files::append(&file, bytes, move |landed| {
                if let Err(why) = landed {
                    log::warn!("capture envelope entry into {} failed: {why}", target.rel());
                }
            }) {
                log::warn!("capture envelope entry refused: {why}");
            }
        }
        if queue.next_write == queue.next_issue {
            queues.remove(&path);
        }
    }
}

impl Drop for EnvelopeTarget {
    fn drop(&mut self) {
        self.settle_turn(None);
    }
}

fn entry_bytes(record: [u64; 2], envelope: ClientEnvelope) -> Option<Vec<u8>> {
    write_envelope_entry(&ClientEnvelopeEntry { record, envelope }).ok()
}

pub fn frame_record(
    pieces: &[Piece],
    envelope_of: impl Fn(Vec<ClientPieceInfo>) -> ClientEnvelope,
) -> Result<(Vec<u8>, ClientEnvelope), String> {
    let mut envelope_len = 0usize;
    loop {
        let mut at = (CAPTURE_RECORD_HEAD_LEN + envelope_len) as u64;
        let infos: Vec<ClientPieceInfo> = pieces
            .iter()
            .map(|p| {
                let mut info = p.info.clone();
                info.range = [at, p.len()];
                at += p.len();
                info
            })
            .collect();
        let envelope = envelope_of(infos);
        let bytes = encode_envelope(&envelope)?;
        if bytes.len() != envelope_len {
            envelope_len = bytes.len();
            continue;
        }
        let head = CaptureRecordHead {
            kind: CaptureRecordKind::Frame,
            complete: true,
            len: at,
            envelope_at: CAPTURE_RECORD_HEAD_LEN as u64,
            envelope_len: bytes.len() as u64,
            envelope_crc: crc32(&bytes),
        };
        let mut out = Vec::with_capacity(at as usize);
        out.extend_from_slice(&write_record_head(&head));
        out.extend_from_slice(&bytes);
        for piece in pieces {
            out.extend_from_slice(&piece.bytes);
        }
        return Ok((out, envelope));
    }
}

fn encode_envelope(envelope: &ClientEnvelope) -> Result<Vec<u8>, String> {
    match envelope {
        ClientEnvelope::State(e) => postcard::to_allocvec(e),
        ClientEnvelope::Frame(e) => postcard::to_allocvec(e),
    }
    .map_err(|e| format!("envelope: {e}"))
}

pub struct FrameSlot {
    slot: RecordSlot,
}

impl FrameSlot {
    pub fn reserve(
        file: &FileRef,
        envelopes: Option<EnvelopeTarget>,
        landed: impl FnOnce(Result<[u64; 2], String>, Option<&ClientEnvelope>) + Send + 'static,
    ) -> Result<(Self, FrameFill), String> {
        let envelope: Arc<Mutex<Option<ClientEnvelope>>> = Arc::default();
        let kept = Arc::clone(&envelope);
        let slot = files::record(file, move |result| {
            let envelope = kept.lock().unwrap_or_else(PoisonError::into_inner).take();
            if let Some(target) = envelopes {
                let entry = match (&result, &envelope) {
                    (Ok(range), Some(envelope)) => entry_bytes(*range, envelope.clone()),
                    _ => None,
                };
                target.settle(entry);
            }
            landed(result, envelope.as_ref());
        })?;
        Ok((Self { slot }, FrameFill { envelope }))
    }

    pub fn fill(self, fill: FrameFill, bytes: Vec<u8>, envelope: ClientEnvelope) {
        *fill.envelope.lock().unwrap_or_else(PoisonError::into_inner) = Some(envelope);
        self.slot.fill(vec![bytes]);
    }

    pub fn fail(self, why: String) {
        self.slot.fail(why);
    }
}

pub struct FrameFill {
    envelope: Arc<Mutex<Option<ClientEnvelope>>>,
}

pub struct StateRecord {
    file: FileRef,
    shared: Arc<Mutex<StateProgress>>,
}

struct StateProgress {
    slots: Vec<Option<RecordSlot>>,
    chunks: Vec<Option<Vec<(ClientPieceInfo, u64)>>>,
    landed: Vec<Option<[u64; 2]>>,
    make_envelope: Option<Box<dyn FnOnce(Vec<ClientPieceInfo>) -> ClientEnvelope + Send>>,
    envelope: Option<(ClientEnvelope, u32)>,
    error: Option<String>,
    finish: Option<StateFinish>,
}

struct StateFinish {
    envelopes: Option<EnvelopeTarget>,
    done: Box<dyn FnOnce(Result<ClientFileAnswer, String>) + Send>,
}

fn lock(shared: &Mutex<StateProgress>) -> MutexGuard<'_, StateProgress> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

impl StateRecord {
    pub fn reserve(
        file: FileRef,
        chunks: usize,
        envelopes: Option<EnvelopeTarget>,
        done: impl FnOnce(Result<ClientFileAnswer, String>) + Send + 'static,
    ) -> Result<Self, String> {
        let chunks = chunks.max(1);
        let shared = Arc::new(Mutex::new(StateProgress {
            slots: Vec::with_capacity(chunks + 1),
            chunks: vec![None; chunks],
            landed: vec![None; chunks + 1],
            make_envelope: None,
            envelope: None,
            error: None,
            finish: Some(StateFinish {
                envelopes,
                done: Box::new(done),
            }),
        }));
        for index in 0..=chunks {
            let progress = Arc::clone(&shared);
            let target = file.clone();
            let slot = files::record(&file, move |result| {
                landed(&progress, &target, index, result);
            });
            match slot {
                Ok(slot) => lock(&shared).slots.push(Some(slot)),
                Err(why) => {
                    let slots: Vec<_> = lock(&shared).slots.drain(..).flatten().collect();
                    lock(&shared).finish = None;
                    for slot in slots {
                        slot.fail(why.clone());
                    }
                    return Err(why);
                }
            }
        }
        Ok(Self { file, shared })
    }

    pub fn set_envelope(
        &self,
        envelope: impl FnOnce(Vec<ClientPieceInfo>) -> ClientEnvelope + Send + 'static,
    ) {
        lock(&self.shared).make_envelope = Some(Box::new(envelope));
    }

    pub fn chunks(&self) -> usize {
        lock(&self.shared).chunks.len()
    }

    pub fn fill(&self, index: usize, pieces: Vec<Piece>) {
        let mut bytes = Vec::with_capacity(pieces.len() + 1);
        let mut at = 0u64;
        if index == 0 {
            let head = write_record_head(&CaptureRecordHead {
                kind: CaptureRecordKind::State,
                complete: false,
                len: 0,
                envelope_at: 0,
                envelope_len: 0,
                envelope_crc: 0,
            });
            at += head.len() as u64;
            bytes.push(head.to_vec());
        }
        let mut infos = Vec::with_capacity(pieces.len());
        for piece in pieces {
            infos.push((piece.info, at));
            at += piece.bytes.len() as u64;
            bytes.push(piece.bytes);
        }
        let slot = {
            let mut progress = lock(&self.shared);
            progress.chunks[index] = Some(infos);
            progress.slots.get_mut(index).and_then(Option::take)
        };
        if let Some(slot) = slot {
            slot.fill(bytes);
        }
    }

    pub fn fail(&self, why: String) {
        let slots: Vec<_> = {
            let mut progress = lock(&self.shared);
            progress.error.get_or_insert(why.clone());
            progress.slots.iter_mut().filter_map(Option::take).collect()
        };
        for slot in slots {
            slot.fail(why.clone());
        }
    }

    pub fn file(&self) -> &FileRef {
        &self.file
    }
}

fn landed(
    shared: &Arc<Mutex<StateProgress>>,
    file: &FileRef,
    index: usize,
    result: Result<[u64; 2], String>,
) {
    let mut progress = lock(shared);
    let chunk_count = progress.chunks.len();
    match result {
        Ok(range) => {
            progress.landed[index] = Some(range);
        }
        Err(why) => {
            progress.error.get_or_insert(why);
        }
    }
    if index < chunk_count {
        let all_chunks = progress.landed[..chunk_count].iter().all(Option::is_some);
        if !all_chunks {
            if progress.error.is_some() {
                fail_envelope_slot(progress);
            }
            return;
        }
        let Some(envelope_slot) = progress.slots.get_mut(chunk_count).and_then(Option::take) else {
            return;
        };
        let start = progress.landed[0].expect("all chunks landed")[0];
        let mut infos = Vec::new();
        for (chunk, landed) in progress.chunks.iter().zip(&progress.landed) {
            let at = landed.expect("all chunks landed")[0] - start;
            for (info, offset) in chunk.iter().flatten() {
                let mut info = info.clone();
                let len = info.range[1];
                info.range = [at + offset, len];
                infos.push(info);
            }
        }
        let Some(make) = progress.make_envelope.take() else {
            drop(progress);
            envelope_slot.fail("the capture's envelope was never made".into());
            return;
        };
        let envelope = make(infos);
        match encode_envelope(&envelope) {
            Ok(bytes) => {
                progress.envelope = Some((envelope, crc32(&bytes)));
                drop(progress);
                envelope_slot.fill(vec![bytes]);
            }
            Err(why) => {
                drop(progress);
                envelope_slot.fail(why);
            }
        }
        return;
    }
    let finish = progress.finish.take();
    let error = progress.error.clone();
    let start = progress.landed[0].map(|r| r[0]);
    let envelope_range = progress.landed[chunk_count];
    let envelope = progress.envelope.take();
    drop(progress);
    let Some(finish) = finish else {
        return;
    };
    let (Some(start), Some(env), None) = (start, envelope_range, error.clone()) else {
        if let Some(target) = finish.envelopes {
            target.settle(None);
        }
        (finish.done)(Err(
            error.unwrap_or_else(|| "the capture did not land".into())
        ));
        return;
    };
    let Some((envelope, envelope_crc)) = envelope else {
        (finish.done)(Err("the capture's envelope was lost".into()));
        return;
    };
    let len = env[0] + env[1] - start;
    let head = write_record_head(&CaptureRecordHead {
        kind: CaptureRecordKind::State,
        complete: true,
        len,
        envelope_at: env[0] - start,
        envelope_len: env[1],
        envelope_crc,
    });
    files::patch(file, start, head.to_vec(), move |patched| match patched {
        Ok(_) => {
            if let Some(target) = finish.envelopes {
                target.settle(entry_bytes([start, len], envelope));
            }
            (finish.done)(Ok(ClientFileAnswer::Done {
                range: Some([start, len]),
                envelope: Some(env),
            }));
        }
        Err(why) => {
            if let Some(target) = finish.envelopes {
                target.settle(None);
            }
            (finish.done)(Err(why));
        }
    });
}

fn fail_envelope_slot(mut progress: MutexGuard<'_, StateProgress>) {
    let index = progress.chunks.len();
    let why = progress
        .error
        .clone()
        .unwrap_or_else(|| "the capture failed".into());
    if let Some(slot) = progress.slots.get_mut(index).and_then(Option::take) {
        drop(progress);
        slot.fail(why);
    }
}
