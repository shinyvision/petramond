//! Building an Apply: `pre ⊕ state ⊕ fold(events)`, diffed against the presented world.
//!
//! State pieces are picked last-wins per key. PROVENANCE skips any piece the presented world
//! already has from that range, unread. The rest gets decoded (or pulled from the piece cache).
//!
//! Events fold over copies of the touched columns, in a detached replica using the same write
//! functions as the presented world. Each resulting key is compared against what's presented;
//! equal content drops out, so an unchanged key keeps its `Arc`, mesh and light. Only the diffs
//! make it out.

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

#[derive(Clone, Debug)]
pub struct StatePiece {
    pub key: ClientStateKey,
    pub range: PieceRange,
}

#[derive(Clone, Debug)]
pub enum Shape {
    Piece(StatePiece),
    Record(Vec<StatePiece>),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    Shape {
        incarnation: u64,
        offset: u64,
        len: u64,
    },
    Piece(PieceRange),
    Frames {
        incarnation: u64,
        offset: u64,
        end: u64,
    },
}

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

#[derive(Clone, Debug, Default)]
pub struct BaseColumn {
    pub column: Option<(Arc<ColumnPayload>, Option<PieceRange>)>,
    pub sections: BTreeMap<i32, (SectionContent, Option<PieceRange>)>,
    pub pending: Vec<TerrainEdit>,
}

#[derive(Clone, Debug)]
pub enum TerrainBody {
    Section(SectionContent),
    Column(Arc<ColumnPayload>),
}

#[derive(Clone, Debug)]
pub enum KeyChange<T> {
    Origin(Option<PieceRange>),
    Set(T, Option<PieceRange>),
    Removed,
}

#[derive(Clone, Debug)]
pub struct ColumnOutcome {
    pub pos: ChunkPos,
    pub column: Option<KeyChange<Arc<ColumnPayload>>>,
    pub sections: BTreeMap<i32, KeyChange<SectionContent>>,
    pub whole: Folded,
}

pub struct FoldJob {
    pub id: u64,
    pub at: f64,
    pub terrain: Vec<(ClientStateKey, PieceRange, TerrainBody)>,
    pub base: BTreeMap<ChunkPos, BaseColumn>,
    pub presence: Option<Arc<PresenceIndex>>,
    pub population: Option<ClientPopulation>,
    pub moment_pieces: Vec<StateBody>,
    pub moment: Moment,
    pub stretch: Vec<Arc<Frame>>,
    pub rest: Queue,
    pub released_through: Option<u64>,
    pub index_only: Vec<StatePiece>,
}

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
    let released = moment.clock.map(|(tick, _)| tick).or(released_through);
    let keep_from = views.iter().rposition(|v| v.at <= at).unwrap_or(0);
    views.drain(..keep_from);

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

fn away_of<T>(content: T, origin: Option<PieceRange>) -> Away<T> {
    match origin {
        Some(r) => Away::Range(r),
        None => Away::Held(content),
    }
}
