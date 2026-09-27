//! A presented world: a world-less client's terrain and moment, built from
//! byte ranges of a mod's files.
//!
//! Everything that changes the presented world takes effect in ISSUE ORDER
//! ([`Op`]): an apply that has not landed holds back every later `Time` and
//! `Queue`. Applies are prepared one at a time, off the frame when they need
//! reads, and land at the top of a frame drive as a diff of pointer swaps.
//! An apply whose data is all in memory (provenance skips its pieces, the
//! piece cache holds the rest, its frames are decoded) is prepared inline
//! and lands in the frame it was issued in.
//!
//! The engine keeps no rewind history: the mod passes the pieces and events
//! that state where it wants to be, and provenance plus the piece cache make
//! a jump cost what differs.

mod io;
mod land;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;

use mod_api::capture::{ClientPresence, ClientStateKey};
use petramond_world::chunk::{ChunkPos, SectionPos};
use rustc_hash::{FxHashMap, FxHashSet};

use super::body::{CapturedCues, NameTables, ServerToClient, ViewCue};
use super::feed::{Moment, Queue};
pub use super::fold::Prepared;
use super::fold::{
    fold, resolve_state, resolve_stretch, touched_columns, BaseColumn, FoldJob, Need,
    PresenceIndex, Shape, StatePiece, TerrainBody,
};
use super::source::{FileChanges, FileRanges, SourceFile};
use super::unpack::{Frame, StateBody, Vocab};
use super::window::{Away, Stated, Window};
use crate::worker::JobPool;
use crate::world::{Cached, PieceRange, ReplicaWorld, Resident};
use io::{Done, Io, Read};

/// A call that changes the presented world, in issue order.
#[derive(Clone, Debug)]
pub enum Op {
    Apply {
        id: u64,
        state: Vec<FileRanges>,
        events: Vec<FileRanges>,
        at: f64,
    },
    Queue(Vec<FileRanges>),
    Time(f64),
}

/// What a frame's drive hands the client presenting the world.
#[derive(Debug, Default)]
pub struct DriveOut {
    /// An apply landed: the moment starts over from `messages` (reset what
    /// presents a moment before ingesting them).
    pub jumped: bool,
    /// World messages to ingest, in order; their terrain is already in the
    /// replica, so they carry none.
    pub messages: Vec<ServerToClient>,
    /// The captured player's predictions and dig cell, as released.
    pub cues: Vec<CapturedCues>,
    /// The captured view samples released (after a jump: all kept ones).
    pub views: Vec<ViewCue>,
    /// Applies that landed this frame, in order.
    pub landed: Vec<u64>,
    /// Applies that failed this frame, with why.
    pub failed: Vec<(u64, String)>,
}

/// An apply being prepared.
struct Preparing {
    id: u64,
    /// Every read it asked for; none = it was in memory when issued.
    waited: bool,
    /// A fold submitted to the job pool.
    folding: bool,
}

pub struct Presentation {
    tables: NameTables,
    files: FxHashMap<u64, SourceFile>,
    changes: FileChanges,
    io: Io,
    pool: Arc<JobPool>,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    requested: FxHashSet<Need>,
    shapes: FxHashMap<(u64, u64), Result<Shape, String>>,
    frames: FxHashMap<(u64, u64), Result<Arc<Frame>, String>>,
    decoded: FxHashMap<PieceRange, Result<StateBody, String>>,
    stated: Stated,
    moment: Moment,
    ops: VecDeque<Op>,
    cancelled: FxHashSet<u64>,
    preparing: Option<Preparing>,
    folded: Option<(u64, Result<Box<Prepared>, String>)>,
    queue: Queue,
    position: f64,
    released_through: Option<u64>,
    window: Option<Window>,
    window_dirty: bool,
    /// Away columns inside the window still to load, farthest first.
    entering: Vec<ChunkPos>,
    views: Vec<ViewCue>,
    applied: u64,
    error: Option<String>,
    /// Measured read latency (seconds) and bytes per released tick, for
    /// read-ahead: what the position will need before a new read lands.
    read_seconds: f64,
    bytes_per_tick: f64,
    ticks_per_second: f64,
}

impl Presentation {
    /// A presentation of a world whose id vocabulary is `tables`. It holds
    /// nothing until the first apply lands.
    pub fn new(tables: NameTables, pool: Arc<JobPool>) -> Self {
        let vocab = Arc::new(Vocab::of(&tables));
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let io = Io::new(Arc::clone(&vocab), Arc::clone(&pool), done_tx.clone());
        Self {
            tables,
            files: FxHashMap::default(),
            changes: FileChanges::subscribe(),
            io,
            pool,
            done_tx,
            done_rx,
            requested: FxHashSet::default(),
            shapes: FxHashMap::default(),
            frames: FxHashMap::default(),
            decoded: FxHashMap::default(),
            stated: Stated::default(),
            moment: Moment::default(),
            ops: VecDeque::new(),
            cancelled: FxHashSet::default(),
            preparing: None,
            folded: None,
            queue: Queue::default(),
            position: 0.0,
            released_through: None,
            window: None,
            window_dirty: true,
            entering: Vec::new(),
            views: Vec::new(),
            applied: 0,
            error: None,
            read_seconds: 0.0,
            bytes_per_tick: 0.0,
            ticks_per_second: 0.0,
        }
    }

    pub fn tables(&self) -> &NameTables {
        &self.tables
    }

    /// Queue a call; it takes effect in issue order.
    pub fn push(&mut self, op: Op) {
        match &op {
            Op::Apply { state, events, .. } => {
                self.register(state);
                self.register(events);
            }
            Op::Queue(events) => self.register(events),
            Op::Time(_) => {}
        }
        self.ops.push_back(op);
    }

    fn register(&mut self, ranges: &[FileRanges]) {
        for r in ranges {
            self.files
                .entry(r.file.incarnation)
                .or_insert_with(|| r.file.clone());
        }
    }

    /// `true`: apply `id` will never land. `false`: it already did (or
    /// failed).
    pub fn cancel(&mut self, id: u64) -> bool {
        let queued = self
            .ops
            .iter()
            .position(|op| matches!(op, Op::Apply { id: i, .. } if *i == id));
        match queued {
            Some(i) => {
                self.ops.remove(i);
                if self.preparing.as_ref().is_some_and(|p| p.id == id) {
                    self.preparing = None;
                    self.cancelled.insert(id);
                }
                true
            }
            None => false,
        }
    }

    /// Applies not yet landed, in issue order.
    pub fn pending(&self) -> Vec<u64> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                Op::Apply { id, .. } => Some(*id),
                _ => None,
            })
            .collect()
    }

    pub fn position(&self) -> f64 {
        self.position
    }

    pub fn released_through(&self) -> Option<u64> {
        self.released_through
    }

    pub fn applied(&self) -> u64 {
        self.applied
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn moment(&self) -> &Moment {
        &self.moment
    }

    pub fn exhausted(&self) -> bool {
        self.queue.is_empty()
    }

    /// No apply pending, and every batch the position needs is released or
    /// the queue has nothing more.
    pub fn ready(&self) -> bool {
        self.ops.is_empty()
            && (self.exhausted()
                || self
                    .released_through
                    .is_some_and(|t| t > self.position.floor() as u64))
    }

    /// Every stated column outside the window.
    pub fn away(&self) -> &Stated {
        &self.stated
    }

    /// The load window, around the presented camera; `None` before there is
    /// a camera, and nothing is resident then.
    pub fn set_window(&mut self, window: Option<Window>) {
        if self.window != window {
            self.window = window;
            self.window_dirty = true;
        }
    }

    /// One frame's drive: land what is prepared, carry out what is due, and
    /// keep the window resident.
    pub fn drive(&mut self, replica: &mut ReplicaWorld, dt: f32) -> DriveOut {
        replica.keep_provenance();
        let mut out = DriveOut::default();
        self.take_file_changes(replica);
        let before = self.position;
        loop {
            self.drain(replica);
            if !self.step(replica, &mut out) {
                break;
            }
        }
        self.release(replica, &mut out);
        self.keep_window(replica);
        self.read_ahead(dt, before);
        out
    }

    fn drain(&mut self, replica: &mut ReplicaWorld) {
        while let Ok(done) = self.done_rx.try_recv() {
            self.absorb(replica, done);
        }
    }

    /// Block until every read and fold this presentation asked for has
    /// answered, taking each answer in. For a test's determinism.
    #[cfg(any(test, feature = "test-support"))]
    pub fn wait_for_reads(&mut self, replica: &mut ReplicaWorld) {
        while !self.requested.is_empty() || self.preparing.as_ref().is_some_and(|p| p.folding) {
            let Ok(done) = self.done_rx.recv() else {
                return;
            };
            let folded = matches!(done, Done::Folded { .. });
            self.absorb(replica, done);
            if folded {
                if let Some(p) = self.preparing.as_mut() {
                    p.folding = false;
                }
            }
        }
    }

    fn absorb(&mut self, replica: &mut ReplicaWorld, done: Done) {
        {
            match done {
                Done::Shape {
                    incarnation,
                    offset,
                    result,
                } => {
                    self.requested.retain(|n| {
                        !matches!(n, Need::Shape { incarnation: i, offset: o, .. }
                            if *i == incarnation && *o == offset)
                    });
                    // A record's pieces are each a range a mod may pass
                    // alone later: their shapes are known too.
                    if let Ok(Shape::Record(pieces)) = &result {
                        for piece in pieces {
                            self.shapes
                                .entry((incarnation, piece.range.offset))
                                .or_insert_with(|| Ok(Shape::Piece(piece.clone())));
                        }
                    }
                    self.shapes.insert((incarnation, offset), result);
                }
                Done::Frames {
                    incarnation,
                    frames,
                    from,
                    bytes,
                    seconds,
                } => {
                    self.requested.retain(|n| {
                        !matches!(n, Need::Frames { incarnation: i, offset: o, .. }
                            if *i == incarnation && *o == from)
                    });
                    self.read_seconds = ema(self.read_seconds, seconds);
                    let ticks: u64 = frames
                        .iter()
                        .filter_map(|(_, f)| f.as_ref().ok())
                        .map(|f| f.batches.len() as u64)
                        .sum();
                    if ticks > 0 {
                        self.bytes_per_tick = ema(self.bytes_per_tick, bytes as f64 / ticks as f64);
                    }
                    for (offset, frame) in frames {
                        if let Ok(f) = &frame {
                            self.cache_frame_pieces(replica, f);
                        }
                        self.frames.insert((incarnation, offset), frame);
                    }
                }
                Done::Piece { range, result } => {
                    self.requested.remove(&Need::Piece(range));
                    match result.map(|b| *b) {
                        Ok(StateBody::Section(content)) => {
                            replica.cache_piece(range, Cached::Section(content));
                        }
                        Ok(StateBody::Column(payload)) => {
                            replica.cache_piece(range, Cached::Column(payload));
                        }
                        other => {
                            self.decoded.insert(range, other);
                        }
                    }
                }
                Done::Folded { id, result } => {
                    if !self.cancelled.remove(&id) {
                        self.folded = Some((id, result));
                    }
                }
            }
        }
    }

    /// Frames' full arrivals, decoded, kept like any piece read.
    fn cache_frame_pieces(&self, replica: &mut ReplicaWorld, frame: &Frame) {
        for item in &frame.items {
            if let super::unpack::FrameItem::Terrain(crate::world::TerrainEdit::Column(
                p,
                Some(r),
            )) = item
            {
                replica.remember_piece(*r, || Cached::Column(Arc::clone(p)));
            }
        }
    }

    fn request(&mut self, needs: Vec<Need>, key: i64) {
        let mut pieces = Vec::new();
        for need in needs {
            if !self.requested.insert(need.clone()) {
                continue;
            }
            match need {
                Need::Shape {
                    incarnation,
                    offset,
                    len,
                } => match self.files.get(&incarnation) {
                    Some(file) => self.io.read(Read::Shape {
                        file: file.clone(),
                        offset,
                        len,
                    }),
                    None => {
                        self.shapes
                            .insert((incarnation, offset), Err(ended(incarnation)));
                    }
                },
                Need::Frames {
                    incarnation,
                    offset,
                    end,
                } => match self.files.get(&incarnation) {
                    Some(file) => {
                        let block = self.frame_block();
                        self.io.read(Read::Frames {
                            file: file.clone(),
                            offset,
                            end,
                            block,
                        })
                    }
                    None => {
                        self.frames
                            .insert((incarnation, offset), Err(ended(incarnation)));
                    }
                },
                Need::Piece(range) => match self.files.get(&range.incarnation) {
                    Some(file) => pieces.push((file.clone(), range)),
                    None => {
                        self.decoded.insert(range, Err(ended(range.incarnation)));
                    }
                },
            }
        }
        if !pieces.is_empty() {
            self.io.read(Read::Pieces { pieces, key });
        }
    }

    /// What one frame read should take: the ticks the position will need
    /// before another read could land, at the measured bytes per tick.
    fn frame_block(&self) -> u64 {
        let horizon = (self.ticks_per_second * self.read_seconds * 2.0).max(2.0);
        ((self.bytes_per_tick * horizon) as u64).max(64 * 1024)
    }

    /// Carry out the op at the front, if it can. `true` = progress.
    fn step(&mut self, replica: &mut ReplicaWorld, out: &mut DriveOut) -> bool {
        if let Some((id, result)) = self.folded.take() {
            self.preparing = None;
            self.pop_apply(id);
            match result {
                Ok(prepared) => self.land(replica, *prepared, out),
                Err(why) => self.fail(id, why, out),
            }
            return true;
        }
        match self.ops.front().cloned() {
            None => false,
            Some(Op::Time(at)) => {
                self.ops.pop_front();
                // Checked against the position the ops before it left: one
                // accepted against an apply that never landed goes back no
                // further than the pair.
                if crate::modding::client::present::time_reaches(self.position, at) {
                    self.position = at;
                }
                true
            }
            Some(Op::Queue(events)) => {
                self.ops.pop_front();
                self.queue.push(&events);
                true
            }
            Some(Op::Apply {
                id,
                state,
                events,
                at,
            }) => self.prepare(replica, id, &state, &events, at, out),
        }
    }

    fn pop_apply(&mut self, id: u64) {
        if let Some(i) = self
            .ops
            .iter()
            .position(|op| matches!(op, Op::Apply { id: x, .. } if *x == id))
        {
            self.ops.remove(i);
        }
    }

    fn fail(&mut self, id: u64, why: String, out: &mut DriveOut) {
        let why = format!("apply {id}: {why}");
        log::warn!("presentation: {why}");
        self.error = Some(why.clone());
        out.failed.push((id, why));
    }

    /// Plan the front apply. Answers whether anything moved.
    fn prepare(
        &mut self,
        replica: &mut ReplicaWorld,
        id: u64,
        state: &[FileRanges],
        events: &[FileRanges],
        at: f64,
        out: &mut DriveOut,
    ) -> bool {
        let first = self.preparing.as_ref().is_none_or(|p| p.id != id);
        if first {
            self.preparing = Some(Preparing {
                id,
                waited: false,
                folding: false,
            });
        }
        if self.preparing.as_ref().is_some_and(|p| p.folding) {
            return false;
        }
        match self.plan(replica, id, state, events, at) {
            Err(why) => {
                self.preparing = None;
                self.pop_apply(id);
                self.fail(id, why, out);
                true
            }
            Ok(Plan::Waiting(needs)) => {
                if let Some(p) = self.preparing.as_mut() {
                    p.waited = true;
                }
                self.request(needs, i64::MAX / 4);
                false
            }
            Ok(Plan::Ready(job)) => {
                let waited = self.preparing.as_ref().is_some_and(|p| p.waited);
                if waited && !self.pool.is_inline() {
                    if let Some(p) = self.preparing.as_mut() {
                        p.folding = true;
                    }
                    let done = self.done_tx.clone();
                    self.pool.submit(i64::MAX / 4, move || {
                        let _ = done.send(Done::Folded {
                            id,
                            result: fold(*job).map(Box::new),
                        });
                    });
                    false
                } else {
                    self.folded = Some((id, fold(*job).map(Box::new)));
                    true
                }
            }
        }
    }

    /// Everything the front apply needs, or what it still waits on.
    fn plan(
        &mut self,
        replica: &ReplicaWorld,
        id: u64,
        state: &[FileRanges],
        events: &[FileRanges],
        at: f64,
    ) -> Result<Plan, String> {
        let mut needs = Vec::new();
        let shapes = &self.shapes;
        let pieces = resolve_state(state, |i, o| shapes.get(&(i, o)).cloned(), &mut needs)?;
        let frames = &self.frames;
        let limit = at.max(0.0).floor() as u64 + 1;
        let (stretch, rest) = resolve_stretch(
            events,
            limit,
            |i, o| frames.get(&(i, o)).cloned(),
            &mut needs,
        )?;
        if !needs.is_empty() {
            return Ok(Plan::Waiting(needs));
        }
        let touched = touched_columns(&stretch);
        let resident = |c: ChunkPos| replica.data().columns.contains_key(&c);
        let window = self.window;
        let in_window = |c: ChunkPos| window.is_some_and(|w| w.contains(c));

        // Which terrain pieces fold, which only re-index, which change nothing.
        let mut folded_cols: BTreeSet<ChunkPos> = touched.clone();
        let mut terrain = Vec::new();
        let mut index_only = Vec::new();
        let mut moment_pieces = Vec::new();
        let mut presence = None;
        let mut population = None;
        for p in &pieces {
            let (column, resident_key) = match p.key {
                ClientStateKey::Section([x, y, z]) => {
                    let sp = SectionPos::new(x, y, z);
                    (sp.chunk_pos(), Some(Resident::Section(sp)))
                }
                ClientStateKey::Column([x, z]) => {
                    let c = ChunkPos::new(x, z);
                    (c, Some(Resident::Column(c)))
                }
                _ => {
                    match self.decoded.get(&p.range) {
                        None => needs.push(Need::Piece(p.range)),
                        Some(Err(why)) => return Err(why.clone()),
                        Some(Ok(StateBody::Presence(pr))) => {
                            presence = Some(Arc::new(PresenceIndex::new(pr.clone())))
                        }
                        Some(Ok(StateBody::Population(pop))) => population = Some(pop.clone()),
                        Some(Ok(body)) => moment_pieces.push(body.clone()),
                    }
                    continue;
                }
            };
            let key = resident_key.expect("terrain keys only");
            let away = self.stated.away.get(&column);
            let origin = if resident(column) {
                replica.origin_of(key)
            } else {
                away.filter(|a| a.pending.is_empty())
                    .and_then(|a| match key {
                        Resident::Section(sp) => a.sections.get(&sp.cy).and_then(Away::range),
                        Resident::Column(_) => a.column.as_ref().and_then(Away::range),
                    })
            };
            if origin == Some(p.range) && !touched.contains(&column) {
                continue;
            }
            let folds = touched.contains(&column)
                || resident(column)
                || away.is_some_and(|a| !a.pending.is_empty())
                || (away.is_none() && in_window(column));
            if !folds {
                index_only.push(p.clone());
                continue;
            }
            folded_cols.insert(column);
            match replica.cached_piece(&p.range) {
                Some(Cached::Section(c)) => {
                    terrain.push((p.key, p.range, TerrainBody::Section(c.clone())))
                }
                Some(Cached::Column(c)) => {
                    terrain.push((p.key, p.range, TerrainBody::Column(Arc::clone(c))))
                }
                None => match self.decoded.get(&p.range) {
                    Some(Err(why)) => return Err(why.clone()),
                    _ => needs.push(Need::Piece(p.range)),
                },
            }
        }

        // A whole-world statement removes what it does not list AFTER what
        // waits on an away column applies: those columns fold.
        if presence.is_some() {
            folded_cols.extend(
                self.stated
                    .away
                    .iter()
                    .filter(|(_, a)| !a.pending.is_empty())
                    .map(|(&c, _)| c),
            );
        }
        // The folded columns as presented.
        let mut base = std::collections::BTreeMap::new();
        for &c in &folded_cols {
            let col = if resident(c) {
                BaseColumn {
                    column: replica
                        .column_content(c)
                        .map(|p| (Arc::new(p), replica.origin_of(Resident::Column(c)))),
                    sections: replica
                        .column_sections(c)
                        .into_iter()
                        .filter_map(|sp| replica.section_content(sp).map(|s| (sp.cy, s)))
                        .collect(),
                    pending: Vec::new(),
                }
            } else if let Some(away) = self.stated.away.get(&c) {
                let mut col = BaseColumn {
                    pending: away.pending.clone(),
                    ..Default::default()
                };
                match &away.column {
                    None => {}
                    Some(Away::Held(p)) => col.column = Some((Arc::clone(p), None)),
                    Some(Away::Range(r)) => match replica.cached_piece(r) {
                        Some(Cached::Column(p)) => col.column = Some((Arc::clone(p), Some(*r))),
                        _ => needs.push(Need::Piece(*r)),
                    },
                }
                for (&cy, s) in &away.sections {
                    match s {
                        Away::Held(content) => {
                            col.sections.insert(cy, (content.clone(), None));
                        }
                        Away::Range(r) => match replica.cached_piece(r) {
                            Some(Cached::Section(content)) => {
                                col.sections.insert(cy, (content.clone(), Some(*r)));
                            }
                            _ => {
                                if let Some(Err(why)) = self.decoded.get(r) {
                                    return Err(why.clone());
                                }
                                needs.push(Need::Piece(*r));
                            }
                        },
                    }
                }
                col
            } else {
                BaseColumn::default()
            };
            base.insert(c, col);
        }
        if !needs.is_empty() {
            return Ok(Plan::Waiting(needs));
        }

        // A whole-world statement may name only what is presented or stated.
        if let Some(pr) = &presence {
            check_presence(pr.presence(), replica, &self.stated, &pieces)?;
        }
        let mut moment = self.moment.clone();
        moment.previous = None;
        Ok(Plan::Ready(Box::new(FoldJob {
            id,
            at,
            terrain,
            base,
            presence,
            population,
            moment_pieces,
            moment,
            stretch,
            rest,
            released_through: self.released_through,
            index_only,
        })))
    }
}

/// Read the `Tables` piece a presentation opens with, and check it: one
/// whole piece of this format and protocol, of the vocabulary its own
/// tables hash to. The answer arrives on the returned channel.
pub fn open_tables(tables: &FileRanges) -> Receiver<Result<NameTables, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let Some(&[offset, len]) = tables.ranges.first() else {
        let _ = tx.send(Err("no tables range".into()));
        return rx;
    };
    let label = tables.file.label.clone();
    super::source::read(&tables.file, offset, len, move |bytes| {
        let at = |why: String| super::unpack::at(&label, offset, why);
        let result = bytes.map_err(at).and_then(|bytes| {
            let (head, body) =
                mod_api::capture::check_piece(&bytes).map_err(|e| at(e.to_string()))?;
            if head.piece_len() != len {
                return Err(at("a range that is not one whole piece".into()));
            }
            if head.kind != mod_api::capture::ClientPieceKind::Tables {
                return Err(at(format!(
                    "a {:?} piece where the tables belong",
                    head.kind
                )));
            }
            if head.protocol != crate::net::PROTOCOL_VERSION {
                return Err(at(format!(
                    "captured under protocol v{}; this build reads v{}",
                    head.protocol,
                    crate::net::PROTOCOL_VERSION
                )));
            }
            let tables: NameTables = super::body::decode_body(body).map_err(|e| at(e.0))?;
            if super::body::vocabulary_of(&tables) != head.vocabulary {
                return Err(at(
                    "tables that are not the vocabulary their piece names".into()
                ));
            }
            Ok(tables)
        });
        let _ = tx.send(result);
    });
    rx
}

enum Plan {
    Waiting(Vec<Need>),
    Ready(Box<FoldJob>),
}

fn ended(incarnation: u64) -> String {
    format!("file incarnation {incarnation} was deleted or replaced")
}

fn ema(old: f64, new: f64) -> f64 {
    if old == 0.0 {
        new
    } else {
        old * 0.8 + new * 0.2
    }
}

/// `Err` when `presence` lists a key nothing presents or states.
fn check_presence(
    presence: &ClientPresence,
    replica: &ReplicaWorld,
    stated: &Stated,
    pieces: &[StatePiece],
) -> Result<(), String> {
    let stated_here: FxHashSet<ClientStateKey> = pieces.iter().map(|p| p.key).collect();
    for col in &presence.columns {
        let c = ChunkPos::new(col.pos[0], col.pos[1]);
        let away = stated.away.get(&c);
        let has_column = replica.column_content(c).is_some()
            || away.is_some_and(|a| a.presented_keys().0)
            || stated_here.contains(&ClientStateKey::Column(col.pos));
        if !has_column {
            return Err(format!(
                "the presence lists column {:?}, which nothing states",
                col.pos
            ));
        }
        for (w, word) in col.sections.iter().enumerate() {
            let mut bits = *word;
            while bits != 0 {
                let b = bits.trailing_zeros() as i32;
                bits &= bits - 1;
                let cy = presence.cy_min + w as i32 * 64 + b;
                let sp = SectionPos::new(c.cx, cy, c.cz);
                let known = replica.data().sections.contains_key(&sp)
                    || stated.has_section(sp)
                    || stated_here.contains(&ClientStateKey::Section([c.cx, cy, c.cz]));
                if !known {
                    return Err(format!(
                        "the presence lists section {:?}, which nothing states",
                        [c.cx, cy, c.cz]
                    ));
                }
            }
        }
    }
    Ok(())
}
