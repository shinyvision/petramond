use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::{codec, colgen, LoadedColumnGen, LoadedSection, SectionStore};

/// Cores kept out of decoding: the save reader that feeds the pool and the
/// game thread that consumes what it publishes.
const RESERVED_CORES: usize = 2;
/// Decoder ceiling. Records are read serially, so past a few decoders the
/// reader cannot keep them fed and the extra threads only contend with the
/// generation and mesh pools for cores.
const MAX_DECODERS: usize = 4;
/// Jobs the reader may queue ahead of the decoders, per decoder: two keeps a
/// worker busy across a job boundary without letting the reader's read-ahead
/// memory grow with the file.
const QUEUE_DEPTH_PER_DECODER: usize = 2;
/// Records in flight per decoder — queued, decoding, or decoded and waiting
/// in the publisher's reorder window behind a slower predecessor. Bounds how
/// far fast records can pile up behind one slow record N.
const IN_FLIGHT_PER_DECODER: usize = 3;

pub(super) enum DecodeJob {
    Section {
        pos: SectionPos,
        store: SectionStore,
        bytes: Option<Vec<u8>>,
    },
    Column {
        pos: ChunkPos,
        seed: u32,
        bytes: Option<Vec<u8>>,
    },
}

enum Decoded {
    Section(LoadedSection),
    Column(LoadedColumnGen),
}

/// The reader captures records after their write barrier; only those immutable
/// bytes cross to decoders. Permits cap read-ahead and out-of-order results.
/// Publication preserves acquisition order, including successive loads of the
/// same section across an intervening save or unload.
pub(super) struct Decoders {
    sender: Option<mpsc::SyncSender<(u64, DecodeJob)>>,
    permits: mpsc::Receiver<()>,
    next: u64,
    workers: Vec<JoinHandle<()>>,
}

impl Decoders {
    pub(super) fn new(
        sections: mpsc::Sender<LoadedSection>,
        columns: mpsc::Sender<LoadedColumnGen>,
    ) -> Self {
        let count = std::thread::available_parallelism().map_or(1, |n| {
            n.get()
                .saturating_sub(RESERVED_CORES)
                .clamp(1, MAX_DECODERS)
        });
        let (tx, rx) = mpsc::sync_channel::<(u64, DecodeJob)>(count * QUEUE_DEPTH_PER_DECODER);
        let rx = Arc::new(Mutex::new(rx));
        let (completed, results) = mpsc::channel::<(u64, Decoded)>();
        let in_flight = count * IN_FLIGHT_PER_DECODER;
        let (return_permit, permits) = mpsc::sync_channel(in_flight);
        for _ in 0..in_flight {
            return_permit
                .send(())
                .expect("permit channel sized for every permit");
        }
        let publisher = std::thread::Builder::new()
            .name("petramond-load-publish".into())
            .spawn(move || {
                crate::worker::lower_current_thread_priority();
                let mut ready = std::collections::BTreeMap::new();
                let mut next = 0;
                while let Ok((seq, value)) = results.recv() {
                    ready.insert(seq, value);
                    while let Some(value) = ready.remove(&next) {
                        match value {
                            Decoded::Section(s) => {
                                let _ = sections.send(s);
                            }
                            Decoded::Column(c) => {
                                let _ = columns.send(c);
                            }
                        }
                        next += 1;
                        let _ = return_permit.send(());
                    }
                }
            })
            .expect("spawn load publisher");
        let mut workers: Vec<_> = (0..count)
            .map(|i| {
                let (rx, completed) = (rx.clone(), completed.clone());
                std::thread::Builder::new()
                    .name(format!("petramond-decode-{i}"))
                    .spawn(move || {
                        crate::worker::lower_current_thread_priority();
                        loop {
                            let job = rx.lock().unwrap().recv();
                            let Ok((seq, job)) = job else { break };
                            let value = match job {
                                DecodeJob::Section { pos, store, bytes } => {
                                    let decoded =
                                        bytes.and_then(|b| codec::decode_section(pos, &b));
                                    let (section, entities, mobs) = match decoded {
                                        Some((s, e, m)) => (Some(s), e, m),
                                        None => (None, Vec::new(), Vec::new()),
                                    };
                                    Decoded::Section(LoadedSection {
                                        pos,
                                        store,
                                        section,
                                        entities,
                                        mobs,
                                    })
                                }
                                DecodeJob::Column { pos, seed, bytes } => {
                                    let record =
                                        bytes.and_then(|b| colgen::decode_record(pos, seed, &b));
                                    Decoded::Column(LoadedColumnGen { pos, record })
                                }
                            };
                            let _ = completed.send((seq, value));
                        }
                    })
                    .expect("spawn save decoder")
            })
            .collect();
        drop(completed);
        workers.push(publisher);
        Self {
            sender: Some(tx),
            permits,
            next: 0,
            workers,
        }
    }

    /// Hand one record to the pool, waiting for a permit if every in-flight
    /// slot is taken. The pool threads outlive this value (joined on drop), so
    /// a closed channel here means one of them panicked; propagating that
    /// beats silently dropping the record.
    pub(super) fn submit(&mut self, job: DecodeJob) {
        self.permits.recv().expect("save publisher alive");
        self.sender
            .as_ref()
            .expect("sender lives until drop")
            .send((self.next, job))
            .expect("save decoders alive");
        self.next += 1;
    }
}

impl Drop for Decoders {
    fn drop(&mut self) {
        self.sender.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests;
