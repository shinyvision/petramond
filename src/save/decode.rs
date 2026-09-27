use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;

use petramond_world::chunk::{ChunkPos, SectionPos};

use super::codec::KeptContent;
use super::format::{self, RecordError};
use super::palette::Palette;
use super::{
    codec, colgen, region, DecodedLoad, LoadedColumnGen, LoadedSection, SectionRecord,
    SectionStore, Unreadable,
};

const RESERVED_CORES: usize = 2;
const MAX_DECODERS: usize = 8;
const QUEUE_DEPTH_PER_DECODER: usize = 2;
const IN_FLIGHT_PER_DECODER: usize = 3;

pub(super) enum DecodeJob {
    Section {
        pos: SectionPos,
        store: SectionStore,
        bytes: std::io::Result<Option<Vec<u8>>>,
    },
    Column {
        pos: ChunkPos,
        seed: u32,
        bytes: Option<Vec<u8>>,
    },
}

enum Decoded {
    Section(DecodedLoad),
    Column(LoadedColumnGen),
}

pub(super) struct Decoders {
    sender: Option<mpsc::SyncSender<(u64, DecodeJob)>>,
    permits: mpsc::Receiver<()>,
    next: u64,
    workers: Vec<JoinHandle<()>>,
}

impl Decoders {
    pub(super) fn new(
        dir: PathBuf,
        palette: Arc<Palette>,
        sections: mpsc::Sender<DecodedLoad>,
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
                let (rx, completed, dir, palette) =
                    (rx.clone(), completed.clone(), dir.clone(), palette.clone());
                std::thread::Builder::new()
                    .name(format!("petramond-decode-{i}"))
                    .spawn(move || {
                        crate::worker::lower_current_thread_priority();
                        loop {
                            let job = rx.lock().unwrap().recv();
                            let Ok((seq, job)) = job else { break };
                            let value = match job {
                                DecodeJob::Section { pos, store, bytes } => {
                                    let (record, kept) =
                                        decode_record(&dir, pos, store, bytes, &palette);
                                    Decoded::Section(DecodedLoad {
                                        loaded: LoadedSection { pos, store, record },
                                        kept,
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

fn decode_record(
    dir: &Path,
    pos: SectionPos,
    store: SectionStore,
    bytes: std::io::Result<Option<Vec<u8>>>,
    pal: &Palette,
) -> (SectionRecord, KeptContent) {
    let (error, bytes) = match bytes {
        Ok(None) => return (SectionRecord::Absent, KeptContent::default()),
        Ok(Some(bytes)) => match codec::decode_section(pos, &bytes, pal) {
            Ok(decoded) => {
                let record = SectionRecord::Decoded {
                    section: Box::new(decoded.section),
                    entities: decoded.entities,
                    mobs: decoded.mobs,
                };
                return (record, decoded.kept);
            }
            Err(error) => (error, Some(bytes)),
        },
        Err(e) => (
            RecordError::Io {
                format: codec::SECTION.name,
                kind: e.kind(),
            },
            None,
        ),
    };
    if store == SectionStore::ExploredCache {
        log::warn!("explored-cache section {pos:?} is unreadable ({error}); regenerating");
        let record = SectionRecord::Unreadable(Unreadable {
            error,
            quarantined: None,
        });
        return (record, KeptContent::default());
    }
    let quarantined = bytes.and_then(|bytes| {
        let (rx, rz) = region::region_of(pos);
        let name = format!("r.{rx}.{rz}.{}.bin", region::local_index(pos));
        format::quarantine(dir, &Path::new("region").join(name), &bytes)
            .inspect_err(|e| log::error!("could not quarantine section {pos:?}: {e}"))
            .ok()
    });
    log::error!("saved section {pos:?} is unreadable ({error}); kept at {quarantined:?}");
    (
        SectionRecord::Unreadable(Unreadable { error, quarantined }),
        KeptContent::default(),
    )
}

#[cfg(test)]
mod tests;
