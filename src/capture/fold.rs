//! Preparing an Apply: `pre ⊕ state ⊕ fold(events)`, computed off the
//! presented world and handed over as a diff.
//!
//! The state pieces are chosen last-wins per key, and PROVENANCE drops every
//! piece the presented world already holds from that very range, unread.
//! What remains is decoded (or found decoded in the piece cache). The
//! stretch of events then folds over copies of the columns it touches, in a
//! detached replica running the very write functions the presented one
//! runs, and every resulting key is compared with what is presented: equal
//! content drops out, so an unchanged key keeps its `Arc`, its mesh and its
//! light. What lands is only what differs.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mod_api::capture::{ClientPopulation, ClientPresence, ClientStateKey};
use petramond_world::chunk::{ChunkPos, SectionPos};
use rustc_hash::FxHashMap;

use super::feed::{Moment, Queue};
use super::source::FileRanges;
use super::unpack::{Frame, FrameItem, StateBody};
use super::view::ViewCue;
use super::window::{Away, AwayColumn};
use crate::net::protocol::ColumnPayload;
use crate::world::{DetachedFold, PieceRange, SectionContent, TerrainEdit};

/// One state piece an apply passes, after last-wins.
#[derive(Clone, Debug)]
pub struct StatePiece {
    pub key: ClientStateKey,
    pub range: PieceRange,
}

/// What the bytes at a state range's start turned out to be.
#[derive(Clone, Debug)]
pub enum Shape {
    /// One piece, stating this key.
    Piece(StatePiece),
    /// A state record: every piece its envelope lists.
    Record(Vec<StatePiece>),
}

/// Something an apply waits on before it can fold.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    /// The head (and a record's envelope) at `offset` of incarnation `file`.
    Shape {
        incarnation: u64,
        offset: u64,
        len: u64,
    },
    /// A piece decoded.
    Piece(PieceRange),
    /// The frame record at `offset`, and the ones after it up to `end`.
    Frames {
        incarnation: u64,
        offset: u64,
        end: u64,
    },
}

/// The state ranges as pieces, last occurrence of a key winning.
pub fn resolve_state(
    ranges: &[FileRanges],
    shape: impl Fn(u64, u64) -> Option<Result<Shape, String>>,
    needs: &mut Vec<Need>,
) -> Result<Vec<StatePiece>, String> {
    let mut all = Vec::new();
    for r in ranges {
        for &[offset, len] in &r.ranges {
            match shape(r.file.incarnation, offset) {
                None => needs.push(Need::Shape {
                    incarnation: r.file.incarnation,
                    offset,
                    len,
                }),
                Some(Err(why)) => return Err(why),
                Some(Ok(Shape::Piece(p))) => {
                    if p.range.len != len {
                        return Err(super::unpack::at(
                            &r.file.label,
                            offset,
                            format!("a range of {len} bytes over a piece of {}", p.range.len),
                        ));
                    }
                    all.push(p);
                }
                Some(Ok(Shape::Record(pieces))) => {
                    let whole = pieces
                        .iter()
                        .map(|p| p.range.offset + p.range.len)
                        .max()
                        .unwrap_or(offset);
                    if whole > offset + len {
                        return Err(super::unpack::at(
                            &r.file.label,
                            offset,
                            "a range that ends inside its record",
                        ));
                    }
                    all.extend(pieces);
                }
            }
        }
    }
    let mut last: BTreeMap<ClientStateKey, usize> = BTreeMap::new();
    for (i, p) in all.iter().enumerate() {
        last.insert(p.key, i);
    }
    Ok(all
        .into_iter()
        .enumerate()
        .filter(|(i, p)| last.get(&p.key) == Some(i))
        .map(|(_, p)| p)
        .collect())
}

/// The stretch an apply folds in as state: every frame of `ranges` up to
/// the one that releases batch `limit`, and the queue after it.
pub fn resolve_stretch(
    ranges: &[FileRanges],
    limit: u64,
    frame: impl Fn(u64, u64) -> Option<Result<Arc<Frame>, String>>,
    needs: &mut Vec<Need>,
) -> Result<(Vec<Arc<Frame>>, Queue), String> {
    let mut queue = Queue::default();
    queue.push(ranges);
    let mut stretch = Vec::new();
    while let Some(span) = queue.front() {
        let Some(found) = frame(span.file.incarnation, span.offset) else {
            needs.push(Need::Frames {
                incarnation: span.file.incarnation,
                offset: span.offset,
                end: span.end,
            });
            break;
        };
        let f = found?;
        if f.due > limit {
            break;
        }
        queue.advance(f.record.len);
        stretch.push(f);
    }
    Ok((stretch, queue))
}

/// The columns a stretch's terrain writes.
pub fn touched_columns(stretch: &[Arc<Frame>]) -> BTreeSet<ChunkPos> {
    let mut out = BTreeSet::new();
    for f in stretch {
        for item in &f.items {
            if let FrameItem::Terrain(edit) = item {
                edit.columns(|c| {
                    out.insert(c);
                });
            }
        }
    }
    out
}

/// A column as presented before the apply, as the fold seeds it.
#[derive(Clone, Debug, Default)]
pub struct BaseColumn {
    pub column: Option<(Arc<ColumnPayload>, Option<PieceRange>)>,
    pub sections: BTreeMap<i32, (SectionContent, Option<PieceRange>)>,
    /// Terrain released while it was away, still to apply over the above.
    pub pending: Vec<TerrainEdit>,
}

/// A decoded terrain state piece.
#[derive(Clone, Debug)]
pub enum TerrainBody {
    Section(SectionContent),
    Column(Arc<ColumnPayload>),
}

/// What changed for one key.
#[derive(Clone, Debug)]
pub enum KeyChange<T> {
    /// Content unchanged; it now holds exactly this piece (or none).
    Origin(Option<PieceRange>),
    /// New content, holding exactly this piece (or none).
    Set(T, Option<PieceRange>),
    Removed,
}

/// One folded column: what changed, and the whole column as it now stands
/// (for a column that lands away).
#[derive(Clone, Debug)]
pub struct ColumnOutcome {
    pub pos: ChunkPos,
    pub column: Option<KeyChange<Arc<ColumnPayload>>>,
    pub sections: BTreeMap<i32, KeyChange<SectionContent>>,
    pub whole: Folded,
}

/// Everything an apply's fold needs, owned, so it can run on a worker.
pub struct FoldJob {
    pub id: u64,
    pub at: f64,
    /// Terrain pieces for folded columns, decoded, in last-wins order.
    pub terrain: Vec<(ClientStateKey, PieceRange, TerrainBody)>,
    /// The columns folded, as presented.
    pub base: BTreeMap<ChunkPos, BaseColumn>,
    pub presence: Option<Arc<PresenceIndex>>,
    pub population: Option<ClientPopulation>,
    /// Moment pieces, decoded, in last-wins order.
    pub moment_pieces: Vec<StateBody>,
    pub moment: Moment,
    pub stretch: Vec<Arc<Frame>>,
    pub rest: Queue,
    pub released_through: Option<u64>,
    /// Terrain pieces for away columns nothing else folds: only re-indexed.
    pub index_only: Vec<StatePiece>,
}

/// A fold's result, ready to land.
pub struct Prepared {
    pub id: u64,
    pub at: f64,
    pub columns: Vec<ColumnOutcome>,
    pub presence: Option<Arc<PresenceIndex>>,
    pub moment: Moment,
    pub views: Vec<ViewCue>,
    pub rest: Queue,
    pub released_through: Option<u64>,
    pub index_only: Vec<StatePiece>,
}

/// A whole-world terrain statement, indexed by column once, so asking it
/// about every resident key costs O(resident keys).
pub struct PresenceIndex {
    presence: ClientPresence,
    columns: FxHashMap<[i32; 2], usize>,
}

impl PresenceIndex {
    pub fn new(presence: ClientPresence) -> Self {
        let columns = presence
            .columns
            .iter()
            .enumerate()
            .map(|(i, c)| (c.pos, i))
            .collect();
        Self { presence, columns }
    }

    pub fn presence(&self) -> &ClientPresence {
        &self.presence
    }

    pub fn lists_column(&self, pos: ChunkPos) -> bool {
        self.columns.contains_key(&[pos.cx, pos.cz])
    }

    pub fn lists_section(&self, pos: SectionPos) -> bool {
        self.columns
            .get(&[pos.cx, pos.cz])
            .is_some_and(|&i| self.presence.columns[i].has_section(self.presence.cy_min, pos.cy))
    }
}

fn same_column(a: &ColumnPayload, b: &ColumnPayload) -> bool {
    a == b
}

/// Run the fold. `Err` fails the whole apply.
pub fn fold(job: FoldJob) -> Result<Prepared, String> {
    let FoldJob {
        id,
        at,
        terrain,
        base,
        presence,
        population,
        moment_pieces,
        mut moment,
        stretch,
        rest,
        released_through,
        index_only,
    } = job;

    // The moment: pieces, then the whole-world statement, then the stretch.
    moment.previous = None;
    for body in &moment_pieces {
        moment.restate(body);
    }
    if let Some(population) = &population {
        moment.restrict(population)?;
    }
    let mut views = Vec::new();
    for f in &stretch {
        moment.frame(f);
        for item in &f.items {
            if let FrameItem::View(v) = item {
                views.push((**v).clone());
            }
        }
    }
    // The newest batch the world now holds is the moment's, wherever the
    // apply moved it.
    let released = moment.clock.map(|(tick, _)| tick).or(released_through);
    let keep_from = views.iter().rposition(|v| v.at <= at).unwrap_or(0);
    views.drain(..keep_from);

    // The terrain: seed what is presented, restate, remove, fold the stretch.
    let mut detached = DetachedFold::new();
    for (&pos, col) in &base {
        if let Some((payload, origin)) = &col.column {
            detached.seed_column((**payload).clone(), *origin);
        }
        for (&cy, (content, origin)) in &col.sections {
            detached.seed_section(
                SectionPos::new(pos.cx, cy, pos.cz),
                content.clone(),
                *origin,
            );
        }
        for edit in &col.pending {
            detached.apply(edit.clone());
        }
    }
    // What the presented columns hold before this apply, pending included:
    // the base every key is compared against.
    let pre: BTreeMap<ChunkPos, Folded> = base
        .keys()
        .map(|&pos| (pos, Folded::of(&detached, pos)))
        .collect();
    for (key, range, body) in &terrain {
        match (key, body) {
            (ClientStateKey::Section([x, y, z]), TerrainBody::Section(content)) => {
                detached.seed_section(SectionPos::new(*x, *y, *z), content.clone(), Some(*range));
            }
            (ClientStateKey::Column(_), TerrainBody::Column(payload)) => {
                detached.seed_column((**payload).clone(), Some(*range));
            }
            _ => return Err("a terrain piece whose body states another kind".into()),
        }
    }
    if let Some(presence) = &presence {
        for &pos in base.keys() {
            if !presence.lists_column(pos) && detached.has_column(pos) {
                detached.apply(TerrainEdit::ColumnUnload(pos));
                continue;
            }
            for sp in detached.column_sections(pos) {
                if !presence.lists_section(sp) {
                    detached.apply(TerrainEdit::SectionUnload(sp));
                }
            }
        }
    }
    for f in &stretch {
        for item in &f.items {
            if let FrameItem::Terrain(edit) = item {
                detached.apply(edit.clone());
            }
        }
    }

    // Compare with the base: equal content drops out.
    let columns = pre
        .into_iter()
        .map(|(pos, before)| {
            let after = Folded::of(&detached, pos);
            let column = change(before.column.as_ref(), after.column.as_ref(), |a, b| {
                Arc::ptr_eq(a, b) || same_column(a, b)
            });
            let cys: BTreeSet<i32> = before
                .sections
                .keys()
                .chain(after.sections.keys())
                .copied()
                .collect();
            let sections = cys
                .into_iter()
                .filter_map(|cy| {
                    change(before.sections.get(&cy), after.sections.get(&cy), |a, b| {
                        a.same(b)
                    })
                    .map(|c| (cy, c))
                })
                .collect();
            ColumnOutcome {
                pos,
                column,
                sections,
                whole: after,
            }
        })
        .collect();

    Ok(Prepared {
        id,
        at,
        columns,
        presence,
        moment,
        views,
        rest,
        released_through: released,
        index_only,
    })
}

/// A column's keys as a fold left them, each with the piece it holds exactly.
#[derive(Clone, Debug, Default)]
pub struct Folded {
    pub column: Option<(Arc<ColumnPayload>, Option<PieceRange>)>,
    pub sections: BTreeMap<i32, (SectionContent, Option<PieceRange>)>,
}

impl Folded {
    fn of(d: &DetachedFold, pos: ChunkPos) -> Self {
        Self {
            column: d.column(pos).map(|(p, o)| (Arc::new(p), o)),
            sections: d
                .column_sections(pos)
                .into_iter()
                .filter_map(|sp| d.section(sp).map(|c| (sp.cy, c)))
                .collect(),
        }
    }

    /// The away index's form: a key holding a piece exactly is that range.
    pub fn into_away(self) -> AwayColumn {
        AwayColumn {
            column: self.column.map(|(p, o)| away_of(p, o)),
            sections: self
                .sections
                .into_iter()
                .map(|(cy, (c, o))| (cy, away_of(c, o)))
                .collect(),
            pending: Vec::new(),
        }
    }
}

/// How one key moved from `before` to `after`; `None` = not at all.
fn change<T: Clone>(
    before: Option<&(T, Option<PieceRange>)>,
    after: Option<&(T, Option<PieceRange>)>,
    same: impl Fn(&T, &T) -> bool,
) -> Option<KeyChange<T>> {
    match (before, after) {
        (None, None) => None,
        (Some(_), None) => Some(KeyChange::Removed),
        (None, Some((c, o))) => Some(KeyChange::Set(c.clone(), *o)),
        (Some((b, bo)), Some((a, ao))) if same(b, a) => {
            (bo != ao).then_some(KeyChange::Origin(*ao))
        }
        (Some(_), Some((a, ao))) => Some(KeyChange::Set(a.clone(), *ao)),
    }
}

/// A key's content in the form the away index keeps it: its range when it
/// holds one exactly, else the content.
fn away_of<T>(content: T, origin: Option<PieceRange>) -> Away<T> {
    match origin {
        Some(r) => Away::Range(r),
        None => Away::Held(content),
    }
}
