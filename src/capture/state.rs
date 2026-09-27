//! `ClientWorldStateWrite`: the presented world's state into a mod file.
//!
//! On the frame, inside the call, the selection is taken as reference-counted
//! handles only ([`take`]): the sections' own `Arc`s, the columns' `Arc`s and
//! the few bytes of column facts beside them, the draw sets of the sections
//! that have any, and the moment's parts. No byte of a piece is made there.
//! [`submit`] reserves the record's turn in its file and hands the snapshot
//! to the job pool, where the pieces are converted, encoded, compressed and
//! checked one chunk per job, and streamed into the file as they finish.

use std::sync::Arc;

use mod_api::capture::{
    ClientColumnPresence, ClientPieceKind, ClientPresence, ClientStateEnvelope, ClientStateKey,
    ClientStateSelect,
};
use mod_api::ClientFileAnswer;
use rustc_hash::{FxHashMap, FxHashSet};

use super::moment::Moment;
use super::pieces::{self, Piece};
use super::writer::{EnvelopeTarget, StateRecord};
use crate::modding::client::files::FileRef;
use crate::net::protocol::BlockDrawEntry;
use crate::worker::JobPool;
use crate::world::ReplicaWorld;
use crate::world::{column_key, section_key};
use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY};
use petramond_world::column::Column;
use petramond_world::section::{Section, SectionSummary};

/// Pieces per streamed chunk: one encoding job, and one queued write of the
/// record — its grain, not a bound on it.
const CHUNK_PIECES: usize = 256;

/// Where a state record's encoding runs in the job pool, whose keys are
/// terrain's squared section distances: behind the terrain within four
/// sections of the camera, ahead of the rest, so a capture never starves
/// while the world streams and never delays what the player sees next.
pub(super) const STATE_JOB_KEY: i64 = 4 * 4;

pub struct SectionSnap {
    pub pos: SectionPos,
    pub section: Arc<Section>,
    pub draws: Vec<BlockDrawEntry>,
}

pub struct ColumnSnap {
    pub pos: ChunkPos,
    pub column: Arc<Column>,
    pub summaries: Option<Box<[SectionSummary]>>,
    pub halo: Option<Arc<[u8]>>,
    pub deep_band_lo: Option<i32>,
}

/// Which piece kinds a write may state.
#[derive(Clone, Copy)]
struct Kinds(u32);

impl Kinds {
    fn of(kinds: Option<&[ClientPieceKind]>) -> Self {
        match kinds {
            None => Self(u32::MAX),
            Some(kinds) => Self(kinds.iter().fold(0, |m, k| m | 1 << (*k as u8))),
        }
    }

    fn has(self, kind: ClientPieceKind) -> bool {
        self.0 & 1 << (kind as u8) != 0
    }
}

enum Presence {
    Taken(Vec<(ChunkPos, u32)>),
    OfSnapshot,
}

/// Entities a write states: all of them, or these ids.
enum Ids {
    All,
    Some(Vec<u64>),
}

impl Ids {
    fn none() -> Self {
        Self::Some(Vec::new())
    }

    fn is_empty(&self) -> bool {
        matches!(self, Self::Some(ids) if ids.is_empty())
    }
}

/// A selection as the call took it: handles, no bytes.
pub struct StateSnapshot {
    pub revision: u64,
    pub since: Option<u64>,
    moment: Moment,
    /// `None` where the loaded-section index named a section the table did
    /// not hold: dropped off the frame.
    sections: Vec<Option<SectionSnap>>,
    /// The draw sets of the sections `sections` holds without them: taken
    /// apart so the walk over every section touches only its handle.
    draws: Vec<(SectionPos, Vec<BlockDrawEntry>)>,
    columns: Vec<ColumnSnap>,
    /// Every present column's loaded-section bits, when `Presence` is
    /// stated: taken, or (for `All`) read off the snapshot's own terrain.
    presence: Option<Presence>,
    population: bool,
    session: bool,
    tables: bool,
    clock: bool,
    roster: bool,
    environment: bool,
    activity: bool,
    viewer: bool,
    mobs: Ids,
    items: Ids,
    players: Ids,
    absent: Vec<ClientStateKey>,
}

impl StateSnapshot {
    fn empty(revision: u64, since: Option<u64>, moment: &Moment) -> Self {
        Self {
            revision,
            since,
            moment: moment.clone(),
            sections: Vec::new(),
            draws: Vec::new(),
            columns: Vec::new(),
            presence: None,
            population: false,
            session: false,
            tables: false,
            clock: false,
            roster: false,
            environment: false,
            activity: false,
            viewer: false,
            mobs: Ids::none(),
            items: Ids::none(),
            players: Ids::none(),
            absent: Vec::new(),
        }
    }

    /// Whether the write states nothing at all: it then writes nothing.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty()
            && self.columns.is_empty()
            && self.presence.is_none()
            && !(self.population
                || self.session
                || self.tables
                || self.clock
                || self.roster
                || self.environment
                || self.activity
                || self.viewer)
            && self.mobs.is_empty()
            && self.items.is_empty()
            && self.players.is_empty()
            && self.absent.is_empty()
    }

    /// Pieces the record will hold at most: what sizes its chunks.
    fn units(&self) -> usize {
        let ids = |ids: &Ids, all: usize| match ids {
            Ids::All => all,
            Ids::Some(ids) => ids.len(),
        };
        let flags = [
            self.presence.is_some(),
            self.population,
            self.session,
            self.tables,
            self.clock,
            self.roster,
            self.environment,
            self.activity,
            self.viewer,
        ];
        self.sections.len()
            + self.columns.len()
            + flags.iter().filter(|f| **f).count()
            + ids(&self.mobs, self.moment.rows.mobs.len())
            + ids(&self.items, self.moment.rows.items.len())
            + ids(&self.players, self.moment.rows.players.len())
    }
}

impl SectionSnap {
    /// Section `pos` as `world` holds it now.
    pub fn of(world: &ReplicaWorld, pos: SectionPos) -> Option<Self> {
        let section = Arc::clone(world.data().sections.get(&pos)?);
        Some(Self {
            pos,
            section,
            draws: world.section_block_draws(pos),
        })
    }
}

impl ColumnSnap {
    /// Column `pos` as `world` holds it now.
    pub fn of(world: &ReplicaWorld, pos: ChunkPos) -> Option<Self> {
        Some(Self::with(world, pos, world.data().columns.get(&pos)?))
    }

    fn with(world: &ReplicaWorld, pos: ChunkPos, column: &Arc<Column>) -> Self {
        ColumnFacts::of(world).snap(pos, column)
    }
}

/// The column facts beside the columns, borrowed apart from the world so a
/// parallel walk can read them.
struct ColumnFacts<'a> {
    summaries: &'a rustc_hash::FxHashMap<ChunkPos, Box<[SectionSummary]>>,
    halos: &'a rustc_hash::FxHashMap<ChunkPos, Arc<[u8]>>,
    bands: &'a rustc_hash::FxHashMap<ChunkPos, i32>,
}

impl<'a> ColumnFacts<'a> {
    fn of(world: &'a ReplicaWorld) -> Self {
        Self {
            summaries: &world.data().column_summaries,
            halos: &world.data().column_biome_halos,
            bands: &world.data().column_deep_band_los,
        }
    }

    fn snap(&self, pos: ChunkPos, column: &Arc<Column>) -> ColumnSnap {
        ColumnSnap {
            pos,
            column: Arc::clone(column),
            summaries: self.summaries.get(&pos).cloned(),
            halo: self.halos.get(&pos).cloned(),
            deep_band_lo: self.bands.get(&pos).copied(),
        }
    }
}

/// The `cy` of every set bit of a column's loaded-section mask.
fn cys(mut bits: u32) -> impl Iterator<Item = i32> {
    std::iter::from_fn(move || {
        (bits != 0).then(|| {
            let cy = SECTION_MIN_CY + bits.trailing_zeros() as i32;
            bits &= bits - 1;
            cy
        })
    })
}

fn presence_of(world: &ReplicaWorld) -> Vec<(ChunkPos, u32)> {
    world
        .data()
        .columns
        .keys()
        .map(|&pos| {
            (
                pos,
                world
                    .data()
                    .section_column_cys
                    .get(&pos)
                    .copied()
                    .unwrap_or(0),
            )
        })
        .collect()
}

/// Take the selection from `world` and `moment` as they stand: the world as
/// the last presented frame left it.
pub fn take(
    world: &ReplicaWorld,
    moment: &Moment,
    select: &ClientStateSelect,
    kinds: Option<&[ClientPieceKind]>,
) -> StateSnapshot {
    let kinds = Kinds::of(kinds);
    let changes = world.changes();
    let revision = changes.revision();
    match select {
        ClientStateSelect::ChangedSince(since) if changes.issued(*since) => {
            let keys: Vec<ClientStateKey> = changes.order().since(*since).collect();
            let mut snap = StateSnapshot::empty(revision, Some(*since), moment);
            for key in keys {
                add_key(&mut snap, world, moment, kinds, key, false);
            }
            snap
        }
        ClientStateSelect::Keys(keys) => {
            let mut snap = StateSnapshot::empty(revision, None, moment);
            let mut seen = FxHashSet::default();
            for &key in keys {
                if seen.insert(key) {
                    add_key(&mut snap, world, moment, kinds, key, true);
                }
            }
            snap
        }
        ClientStateSelect::All | ClientStateSelect::ChangedSince(_) => {
            take_all(world, moment, kinds, revision)
        }
    }
}

/// Every present key. The walk over the terrain is the whole cost of the
/// call, one handle per section and column, so it runs across cores: a
/// handle is an atomic increment on memory the frame has not touched, and
/// one core stalls on each in turn.
fn take_all(world: &ReplicaWorld, moment: &Moment, kinds: Kinds, revision: u64) -> StateSnapshot {
    use rayon::prelude::*;

    let mut snap = StateSnapshot::empty(revision, None, moment);
    let mut positions = Vec::new();
    if kinds.has(ClientPieceKind::Section) {
        positions.reserve(world.data().sections.len());
        for (&column, &bits) in world.data().section_column_cys.iter() {
            positions.extend(cys(bits).map(|cy| SectionPos::new(column.cx, cy, column.cz)));
        }
    }
    let columns: Vec<(ChunkPos, &Arc<Column>)> = if kinds.has(ClientPieceKind::Column) {
        world
            .data()
            .columns
            .iter()
            .map(|(&pos, c)| (pos, c))
            .collect()
    } else {
        Vec::new()
    };
    let all_sections = &world.data().sections;
    let facts = ColumnFacts::of(world);
    // Both walks are indexed, so each lands straight in its one allocation.
    (snap.sections, snap.columns) = rayon::join(
        || {
            positions
                .par_iter()
                .map(|&pos| {
                    all_sections.get(&pos).map(|section| SectionSnap {
                        pos,
                        section: Arc::clone(section),
                        draws: Vec::new(),
                    })
                })
                .collect()
        },
        || {
            columns
                .par_iter()
                .map(|&(pos, column)| facts.snap(pos, column))
                .collect()
        },
    );
    if kinds.has(ClientPieceKind::Section) {
        snap.draws = world
            .draw_sections()
            .map(|pos| (pos, world.section_block_draws(pos)))
            .collect();
    }
    if kinds.has(ClientPieceKind::Presence) {
        snap.presence = Some(
            if kinds.has(ClientPieceKind::Section) && kinds.has(ClientPieceKind::Column) {
                Presence::OfSnapshot
            } else {
                Presence::Taken(presence_of(world))
            },
        );
    }
    snap.population = kinds.has(ClientPieceKind::Population);
    snap.session = kinds.has(ClientPieceKind::Session);
    snap.tables = kinds.has(ClientPieceKind::Tables);
    snap.clock = kinds.has(ClientPieceKind::Clock);
    snap.roster = kinds.has(ClientPieceKind::Roster);
    snap.environment = kinds.has(ClientPieceKind::Environment);
    snap.activity = kinds.has(ClientPieceKind::Activity);
    snap.viewer = kinds.has(ClientPieceKind::Viewer) && moment.viewer.is_some();
    for (kind, ids) in [
        (ClientPieceKind::Mob, &mut snap.mobs),
        (ClientPieceKind::Item, &mut snap.items),
        (ClientPieceKind::Player, &mut snap.players),
    ] {
        if kinds.has(kind) {
            *ids = Ids::All;
        }
    }
    snap
}

/// Add one key to a selection. `listed`: the mod named it, so a key that is
/// not present is reported absent.
fn add_key(
    snap: &mut StateSnapshot,
    world: &ReplicaWorld,
    moment: &Moment,
    kinds: Kinds,
    key: ClientStateKey,
    listed: bool,
) {
    if !kinds.has(key.kind()) {
        return;
    }
    let present = match key {
        ClientStateKey::Section([x, y, z]) => SectionSnap::of(world, SectionPos::new(x, y, z))
            .map(|s| snap.sections.push(Some(s)))
            .is_some(),
        ClientStateKey::Column([x, z]) => ColumnSnap::of(world, ChunkPos::new(x, z))
            .map(|c| snap.columns.push(c))
            .is_some(),
        ClientStateKey::Presence => {
            snap.presence = Some(Presence::Taken(presence_of(world)));
            true
        }
        ClientStateKey::Population => {
            snap.population = true;
            true
        }
        ClientStateKey::Session => {
            snap.session = true;
            true
        }
        ClientStateKey::Tables => {
            snap.tables = true;
            true
        }
        ClientStateKey::Clock => {
            snap.clock = true;
            true
        }
        ClientStateKey::Roster => {
            snap.roster = true;
            true
        }
        ClientStateKey::Environment => {
            snap.environment = true;
            true
        }
        ClientStateKey::Activity => {
            snap.activity = true;
            true
        }
        ClientStateKey::Viewer => {
            snap.viewer = moment.viewer.is_some();
            snap.viewer
        }
        // Entities resolve off the frame against the moment's rows; one not
        // there goes to `absent` then.
        ClientStateKey::Mob(id) => push_id(&mut snap.mobs, id),
        ClientStateKey::Item(id) => push_id(&mut snap.items, id),
        ClientStateKey::Player(id) => push_id(&mut snap.players, u64::from(id.0)),
    };
    if !present && listed {
        snap.absent.push(key);
    }
}

fn push_id(ids: &mut Ids, id: u64) -> bool {
    if let Ids::Some(ids) = ids {
        ids.push(id);
    }
    true
}

/// One piece the record will hold, in record order.
enum Unit {
    Presence,
    Population,
    Session,
    Tables,
    Clock,
    Column(usize),
    Section(usize),
    Mob(usize),
    Item(usize),
    Player(usize),
    Roster,
    Environment,
    Activity,
    Viewer,
}

/// Everything the encoding jobs share.
struct Plan {
    snap: StateSnapshot,
    sections: Vec<SectionSnap>,
    units: Vec<Unit>,
    predicted_sections: FxHashSet<SectionPos>,
    predicted_columns: FxHashSet<ChunkPos>,
}

impl Plan {
    fn new(mut snap: StateSnapshot) -> (Self, Vec<ClientStateKey>) {
        let mut sections: Vec<SectionSnap> = std::mem::take(&mut snap.sections)
            .into_iter()
            .flatten()
            .collect();
        if !snap.draws.is_empty() {
            let mut draws: FxHashMap<SectionPos, Vec<BlockDrawEntry>> =
                std::mem::take(&mut snap.draws).into_iter().collect();
            for section in &mut sections {
                if let Some(d) = draws.remove(&section.pos) {
                    section.draws = d;
                }
            }
        }
        sections.sort_unstable_by_key(|s| (s.pos.cx, s.pos.cy, s.pos.cz));
        if let Some(Presence::OfSnapshot) = snap.presence {
            let mut bits: FxHashMap<ChunkPos, u32> =
                snap.columns.iter().map(|c| (c.pos, 0)).collect();
            for section in &sections {
                *bits.entry(section.pos.chunk_pos()).or_default() |=
                    1 << (section.pos.cy - SECTION_MIN_CY);
            }
            snap.presence = Some(Presence::Taken(bits.into_iter().collect()));
        }
        snap.columns.sort_unstable_by_key(|c| (c.pos.cx, c.pos.cz));
        let mut absent = std::mem::take(&mut snap.absent);
        let rows = &snap.moment.rows;
        let pick = |ids: &Ids,
                    all: usize,
                    id_of: &dyn Fn(usize) -> u64,
                    key: &dyn Fn(u64) -> ClientStateKey,
                    absent: &mut Vec<ClientStateKey>|
         -> Vec<usize> {
            match ids {
                Ids::All => (0..all).collect(),
                Ids::Some(wanted) => {
                    let index: FxHashMap<u64, usize> = (0..all).map(|i| (id_of(i), i)).collect();
                    let mut seen = FxHashSet::default();
                    wanted
                        .iter()
                        .filter(|id| seen.insert(**id))
                        .filter_map(|id| {
                            let found = index.get(id).copied();
                            if found.is_none() {
                                absent.push(key(*id));
                            }
                            found
                        })
                        .collect()
                }
            }
        };
        let mobs = pick(
            &snap.mobs,
            rows.mobs.len(),
            &|i| rows.mobs[i].id,
            &ClientStateKey::Mob,
            &mut absent,
        );
        let items = pick(
            &snap.items,
            rows.items.len(),
            &|i| rows.items[i].id,
            &ClientStateKey::Item,
            &mut absent,
        );
        let players = pick(
            &snap.players,
            rows.players.len(),
            &|i| u64::from(rows.players[i].id.0),
            &|id| ClientStateKey::Player(mod_api::PlayerId(id as u8)),
            &mut absent,
        );
        let mut units = Vec::with_capacity(snap.units());
        if snap.presence.is_some() {
            units.push(Unit::Presence);
        }
        for (flag, unit) in [
            (snap.population, Unit::Population),
            (snap.session, Unit::Session),
            (snap.tables, Unit::Tables),
            (snap.clock, Unit::Clock),
        ] {
            if flag {
                units.push(unit);
            }
        }
        units.extend((0..snap.columns.len()).map(Unit::Column));
        units.extend((0..sections.len()).map(Unit::Section));
        units.extend(mobs.into_iter().map(Unit::Mob));
        units.extend(items.into_iter().map(Unit::Item));
        units.extend(players.into_iter().map(Unit::Player));
        for (flag, unit) in [
            (snap.roster, Unit::Roster),
            (snap.environment, Unit::Environment),
            (snap.activity, Unit::Activity),
            (snap.viewer, Unit::Viewer),
        ] {
            if flag {
                units.push(unit);
            }
        }
        let predicted_sections: FxHashSet<SectionPos> = snap
            .moment
            .predicted
            .iter()
            .filter_map(|c| SectionPos::from_world(c.x, c.y, c.z))
            .collect();
        let predicted_columns = predicted_sections.iter().map(|s| s.chunk_pos()).collect();
        (
            Self {
                snap,
                sections,
                units,
                predicted_sections,
                predicted_columns,
            },
            absent,
        )
    }

    fn encode(&self, unit: &Unit) -> Result<Piece, pieces::EncodeError> {
        let snap = &self.snap;
        let m = &snap.moment;
        match *unit {
            Unit::Presence => match &snap.presence {
                Some(Presence::Taken(columns)) => {
                    pieces::state(ClientStateKey::Presence, false, &presence_body(columns))
                }
                _ => Err("the presence was never taken".into()),
            },
            Unit::Population => {
                pieces::state(ClientStateKey::Population, false, &m.rows.population())
            }
            Unit::Session => pieces::state(ClientStateKey::Session, false, &*m.session),
            Unit::Tables => pieces::state(ClientStateKey::Tables, false, &pieces::vocabulary().0),
            Unit::Clock => pieces::state(
                ClientStateKey::Clock,
                false,
                &mod_api::capture::ClientCapturedClock {
                    tick: m.tick,
                    day_clock: m.day_clock,
                },
            ),
            Unit::Column(i) => {
                let c = &snap.columns[i];
                let payload = crate::world::detached_column_payload(
                    c.pos,
                    &c.column,
                    c.summaries.as_deref(),
                    c.halo.clone(),
                    c.deep_band_lo,
                );
                pieces::state(
                    column_key(c.pos),
                    self.predicted_columns.contains(&c.pos),
                    &payload,
                )
            }
            Unit::Section(i) => {
                let s = &self.sections[i];
                let payload = crate::world::detached_section_payload(&s.section, s.draws.clone());
                pieces::state(
                    section_key(s.pos),
                    self.predicted_sections.contains(&s.pos),
                    &payload,
                )
            }
            Unit::Mob(i) => {
                let row = &m.rows.mobs[i];
                pieces::state(ClientStateKey::Mob(row.id), false, row)
            }
            Unit::Item(i) => {
                let row = &m.rows.items[i];
                pieces::state(ClientStateKey::Item(row.id), false, row)
            }
            Unit::Player(i) => {
                let row = &m.rows.players[i];
                let key = ClientStateKey::Player(mod_api::PlayerId(row.id.0));
                pieces::state(key, false, row)
            }
            Unit::Roster => pieces::state(ClientStateKey::Roster, false, &m.roster.to_vec()),
            Unit::Environment => pieces::state(ClientStateKey::Environment, false, &*m.env),
            Unit::Activity => pieces::state(ClientStateKey::Activity, false, &*m.activity),
            Unit::Viewer => match &m.viewer {
                Some(own) => pieces::state(ClientStateKey::Viewer, false, &**own),
                None => Err("the viewer's own state is gone".into()),
            },
        }
    }
}

fn presence_body(columns: &[(ChunkPos, u32)]) -> ClientPresence {
    let mut columns: Vec<ClientColumnPresence> = columns
        .iter()
        .map(|&(pos, bits)| ClientColumnPresence {
            pos: [pos.cx, pos.cz],
            sections: if bits == 0 {
                Vec::new()
            } else {
                vec![u64::from(bits)]
            },
        })
        .collect();
    columns.sort_unstable_by_key(|c| c.pos);
    ClientPresence {
        cy_min: SECTION_MIN_CY,
        columns,
    }
}

/// Queue a State record of `snap` into `file`, behind everything already
/// queued there, and encode it off the frame. `done` answers once it is
/// complete on disk. `Err` = refused at once (the file is in use).
pub fn submit(
    snap: StateSnapshot,
    file: FileRef,
    envelopes: Option<FileRef>,
    jobs: &Arc<JobPool>,
    done: impl FnOnce(Result<ClientFileAnswer, String>) + Send + 'static,
) -> Result<(), String> {
    let chunks = snap.units().div_ceil(CHUNK_PIECES).max(1);
    let envelopes = envelopes.map(EnvelopeTarget::issue);
    let record = Arc::new(StateRecord::reserve(file, chunks, envelopes, done)?);
    let pool = Arc::clone(jobs);
    jobs.submit(STATE_JOB_KEY, move || {
        let (plan, absent) = Plan::new(snap);
        let header = (
            plan.snap.revision,
            plan.snap.since,
            plan.snap.moment.tick,
            plan.snap.moment.presented_tick,
        );
        record.set_envelope(move |pieces| {
            mod_api::capture::ClientEnvelope::State(ClientStateEnvelope {
                revision: header.0,
                since: header.1,
                tick: header.2,
                presented_tick: header.3,
                pieces,
                absent,
            })
        });
        let plan = Arc::new(plan);
        let per_chunk = plan.units.len().div_ceil(chunks).max(1);
        for index in 0..chunks {
            let plan = Arc::clone(&plan);
            let record = Arc::clone(&record);
            pool.submit(STATE_JOB_KEY, move || {
                let start = (index * per_chunk).min(plan.units.len());
                let end = ((index + 1) * per_chunk).min(plan.units.len());
                let encoded: Result<Vec<Piece>, _> = plan.units[start..end]
                    .iter()
                    .map(|u| plan.encode(u))
                    .collect();
                match encoded {
                    Ok(pieces) => record.fill(index, pieces),
                    Err(why) => record.fail(why),
                }
            });
        }
    });
    Ok(())
}
