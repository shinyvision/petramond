use std::collections::BTreeMap;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;

use mod_api::capture::{
    parse_piece_head, parse_record_head, records, CaptureRecordKind, ClientEnvelope,
    ClientStateEnvelope, CAPTURE_PIECE_MAGIC, CAPTURE_RECORD_HEAD_LEN, CAPTURE_RECORD_MAGIC,
};

use super::super::fold::{Shape, StatePiece};
use super::super::source::{read, SourceFile};
use super::super::unpack::{at, checked, decode_frame, decode_state, Frame, StateBody, Vocab};
use super::Prepared;
use crate::worker::JobPool;
use crate::world::PieceRange;

pub enum Done {
    Shape {
        incarnation: u64,
        offset: u64,
        result: Result<Shape, String>,
    },
    Frames {
        incarnation: u64,
        frames: Vec<(u64, Result<Arc<Frame>, String>)>,
        from: u64,
        bytes: u64,
        seconds: f64,
    },
    Piece {
        range: PieceRange,
        result: Result<Box<StateBody>, String>,
    },
    Folded {
        id: u64,
        result: Result<Box<Prepared>, String>,
    },
}

pub enum Read {
    Shape {
        file: SourceFile,
        offset: u64,
        len: u64,
    },
    Frames {
        file: SourceFile,
        offset: u64,
        end: u64,
        block: u64,
    },
    Pieces {
        pieces: Vec<(SourceFile, PieceRange)>,
        key: i64,
    },
}

#[derive(Clone)]
pub struct Io {
    vocab: Arc<Vocab>,
    pool: Arc<JobPool>,
    done: Sender<Done>,
}

impl Io {
    pub fn new(vocab: Arc<Vocab>, pool: Arc<JobPool>, done: Sender<Done>) -> Self {
        Self { vocab, pool, done }
    }

    pub fn read(&self, request: Read) {
        match request {
            Read::Shape { file, offset, len } => self.shape(file, offset, len),
            Read::Frames {
                file,
                offset,
                end,
                block,
            } => {
                let want = block.max(CAPTURE_RECORD_HEAD_LEN as u64).min(end - offset);
                self.frames(file, offset, end, want, Instant::now());
            }
            Read::Pieces { pieces, key } => self.pieces(pieces, key),
        }
    }

    fn shape(&self, file: SourceFile, offset: u64, len: u64) {
        let io = self.clone();
        let head_len = CAPTURE_RECORD_HEAD_LEN as u64;
        let send = move |io: &Io, file: &SourceFile, result: Result<Shape, String>| {
            let _ = io.done.send(Done::Shape {
                incarnation: file.incarnation,
                offset,
                result,
            });
        };
        if len < head_len {
            let why = at(
                &file.label,
                offset,
                format!("a range of {len} bytes holds no piece or record"),
            );
            return send(self, &file, Err(why));
        }
        let reader = file.clone();
        read(&reader, offset, head_len, move |head| {
            let fail = |why: String| at(&file.label, offset, why);
            let head = match head {
                Ok(h) => h,
                Err(e) => return send(&io, &file, Err(fail(e))),
            };
            if head[0..4] == CAPTURE_PIECE_MAGIC {
                let result = parse_piece_head(&head)
                    .map_err(|e| fail(e.to_string()))
                    .and_then(|piece| {
                        let key = piece.state_key().ok_or_else(|| {
                            fail(format!(
                                "a {:?} piece where a state piece belongs",
                                piece.kind
                            ))
                        })?;
                        Ok(Shape::Piece(StatePiece {
                            key,
                            range: PieceRange {
                                incarnation: file.incarnation,
                                offset,
                                len: piece.piece_len(),
                            },
                        }))
                    });
                return send(&io, &file, result);
            }
            if head[0..4] != CAPTURE_RECORD_MAGIC {
                return send(
                    &io,
                    &file,
                    Err(fail("not a capture piece or record".into())),
                );
            }
            let record = match parse_record_head(&head) {
                Ok(r) => r,
                Err(e) => return send(&io, &file, Err(fail(e.to_string()))),
            };
            let problem = if record.kind != CaptureRecordKind::State {
                Some("a frame record where state belongs".to_string())
            } else if !record.complete {
                Some("a torn record (never completed)".into())
            } else if record.len > len {
                Some(format!(
                    "a record of {} bytes in a range of {len}",
                    record.len
                ))
            } else if record.envelope_at < CAPTURE_RECORD_HEAD_LEN as u64
                || record
                    .envelope_at
                    .checked_add(record.envelope_len)
                    .is_none_or(|e| e > record.len)
            {
                Some("an envelope outside its record".into())
            } else {
                None
            };
            if let Some(why) = problem {
                return send(&io, &file, Err(fail(why)));
            }
            let (io2, file2) = (io.clone(), file.clone());
            read(
                &file,
                offset + record.envelope_at,
                record.envelope_len,
                move |bytes| {
                    let fail = |why: String| at(&file2.label, offset, why);
                    let result = bytes.map_err(fail).and_then(|bytes| {
                        let envelope: ClientStateEnvelope =
                            match mod_api::capture::decode_envelope(&record, &bytes)
                                .map_err(|e| fail(e.to_string()))?
                            {
                                ClientEnvelope::State(s) => s,
                                ClientEnvelope::Frame(_) => {
                                    return Err(fail("a frame envelope in a state record".into()))
                                }
                            };
                        envelope
                            .pieces
                            .iter()
                            .map(|p| {
                                let key = p.key.ok_or_else(|| {
                                    fail(format!("a {:?} piece listed without its key", p.kind))
                                })?;
                                let [rel, plen] = p.range;
                                if rel.checked_add(plen).is_none_or(|e| e > record.len) {
                                    return Err(fail(
                                        "a listed piece reaches past its record".into(),
                                    ));
                                }
                                Ok(StatePiece {
                                    key,
                                    range: PieceRange {
                                        incarnation: file2.incarnation,
                                        offset: offset + rel,
                                        len: plen,
                                    },
                                })
                            })
                            .collect::<Result<_, _>>()
                            .map(Shape::Record)
                    });
                    send(&io2, &file2, result);
                },
            );
        });
    }

    fn frames(&self, file: SourceFile, offset: u64, end: u64, want: u64, started: Instant) {
        let io = self.clone();
        let reader = file.clone();
        read(&reader, offset, want, move |bytes| {
            let bytes = match bytes {
                Ok(b) => b,
                Err(e) => {
                    let why = at(&file.label, offset, format!("the events break off: {e}"));
                    return io.send_frames(&file, offset, vec![(offset, Err(why))], 0, started);
                }
            };
            if let Ok(head) = parse_record_head(&bytes) {
                if head.complete && head.len > want && offset + head.len <= end {
                    return io.frames(file, offset, end, head.len, started);
                }
            }
            let io2 = io.clone();
            io.pool.submit(i64::MAX / 8, move || {
                let frames = walk(&io2.vocab, &file, offset, end, &bytes);
                io2.send_frames(&file, offset, frames, bytes.len() as u64, started);
            });
        });
    }

    fn send_frames(
        &self,
        file: &SourceFile,
        from: u64,
        frames: Vec<(u64, Result<Arc<Frame>, String>)>,
        bytes: u64,
        started: Instant,
    ) {
        let _ = self.done.send(Done::Frames {
            incarnation: file.incarnation,
            frames,
            from,
            bytes,
            seconds: started.elapsed().as_secs_f64(),
        });
    }

    fn pieces(&self, pieces: Vec<(SourceFile, PieceRange)>, key: i64) {
        let mut by_file: BTreeMap<u64, (SourceFile, Vec<PieceRange>)> = BTreeMap::new();
        for (file, range) in pieces {
            by_file
                .entry(file.incarnation)
                .or_insert_with(|| (file, Vec::new()))
                .1
                .push(range);
        }
        for (_, (file, mut ranges)) in by_file {
            ranges.sort_by_key(|r| r.offset);
            ranges.dedup();
            let mut i = 0;
            while i < ranges.len() {
                let start = ranges[i].offset;
                let mut end = start + ranges[i].len;
                let mut j = i + 1;
                while j < ranges.len() && ranges[j].offset <= end {
                    end = end.max(ranges[j].offset + ranges[j].len);
                    j += 1;
                }
                let run: Vec<PieceRange> = ranges[i..j].to_vec();
                i = j;
                let (io, label) = (self.clone(), file.label.clone());
                read(&file, start, end - start, move |bytes| match bytes {
                    Ok(bytes) => {
                        let bytes: Arc<[u8]> = bytes.into();
                        for range in run {
                            let (io2, bytes, label) =
                                (io.clone(), Arc::clone(&bytes), label.clone());
                            io.pool.submit(key, move || {
                                let rel = (range.offset - start) as usize;
                                let piece = &bytes[rel..rel + range.len as usize];
                                let result = checked(piece, range, &label, &io2.vocab).and_then(
                                    |(head, body)| {
                                        decode_state(&head, body, &io2.vocab)
                                            .map(|(_, b)| Box::new(b))
                                            .map_err(|why| at(&label, range.offset, why))
                                    },
                                );
                                let _ = io2.done.send(Done::Piece { range, result });
                            });
                        }
                    }
                    Err(e) => {
                        for range in run {
                            let _ = io.done.send(Done::Piece {
                                range,
                                result: Err(at(&label, range.offset, &e)),
                            });
                        }
                    }
                });
            }
        }
    }
}

fn walk(
    vocab: &Vocab,
    file: &SourceFile,
    offset: u64,
    end: u64,
    bytes: &[u8],
) -> Vec<(u64, Result<Arc<Frame>, String>)> {
    let mut out = Vec::new();
    let mut walked = 0u64;
    for step in records(bytes, offset) {
        match step {
            Ok((record_at, head)) => {
                let rel = (record_at - offset) as usize;
                let range = PieceRange {
                    incarnation: file.incarnation,
                    offset: record_at,
                    len: head.len,
                };
                let frame = decode_frame(
                    &bytes[rel..rel + head.len as usize],
                    range,
                    &file.label,
                    vocab,
                )
                .map(Arc::new);
                walked = record_at - offset + head.len;
                let failed = frame.is_err();
                out.push((record_at, frame));
                if failed {
                    break;
                }
            }
            Err(e) => {
                let stopped = offset + walked;
                if out.is_empty() || offset + bytes.len() as u64 >= end {
                    out.push((stopped, Err(at(&file.label, stopped, e))));
                }
                break;
            }
        }
    }
    out
}
