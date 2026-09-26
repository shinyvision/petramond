//! The per-tick spatial index over the AI snapshot: every neighbour query the
//! mob simulation makes — wander's body and companion vetoes, the navigator's
//! entity cell costs, hearing, melee and target resolution, the crowd veer —
//! goes through a [`MobSnapshot`] instead of scanning the whole live set.
//!
//! The snapshot is rebuilt once per tick from the start-of-tick mob state (the
//! same immutable view the AI always read), so query results never depend on
//! the order mobs tick in. Two structures ride on it:
//! - an id → index map, so resolving an [`EntityRef`](super::EntityRef) is
//!   O(1) instead of a linear `find`;
//! - a uniform horizontal grid of [`CELL_SIZE`]-metre columns (one chunk wide)
//!   holding the ACTIVE mobs, so a radius query touches only the columns its
//!   square overlaps.
//!
//! There is deliberately no "iterate everything" accessor: a behavior that
//! needs its neighbours asks [`MobSnapshot::near`], which is what keeps AI
//! cost proportional to local density rather than to the loaded population.

use rustc_hash::FxHashMap;

use petramond_math::world_pos::WorldPos;

use super::brain::AiMob;

/// Width (m) of one grid column. One chunk: the common query radii (wander
/// cohesion, body vetoes, touch lookups) land in one to four columns, and the
/// navigator's widest entity-cost radius in a 5×5 block of them.
const CELL_SIZE: f64 = 16.0;

type CellKey = (i32, i32);

fn cell_of(x: f64, z: f64) -> CellKey {
    (
        (x / CELL_SIZE).floor() as i32,
        (z / CELL_SIZE).floor() as i32,
    )
}

/// Read-only start-of-tick view of every live mob, indexed by stable id and
/// by position. Built by the manager once per tick; handed to every mob's AI
/// and navigator by shared reference.
#[derive(Default)]
pub struct MobSnapshot {
    mobs: Vec<AiMob>,
    by_id: FxHashMap<u64, u32>,
    /// Grid column → `order[start..start + len]`.
    cells: FxHashMap<CellKey, (u32, u32)>,
    /// Snapshot indices of the ACTIVE mobs grouped by column, ascending index
    /// within a column.
    order: Vec<u32>,
    /// Reused sort buffer for [`reindex`](Self::reindex).
    keyed: Vec<(CellKey, u32)>,
    /// Widest horizontal half-extent of any active body — the pad a query for
    /// overlapping BODIES (rather than feet positions) adds to its reach.
    max_half_extent: f32,
}

impl MobSnapshot {
    /// A snapshot of exactly `mobs` (in that order) — fixtures and one-off
    /// callers; the manager reuses one snapshot through [`rebuild`](Self::rebuild).
    pub fn from_mobs(mobs: impl IntoIterator<Item = AiMob>) -> Self {
        let mut snapshot = MobSnapshot::default();
        snapshot.rebuild(mobs);
        snapshot
    }

    /// The shared empty snapshot, for contexts with no mob population.
    pub fn empty() -> &'static MobSnapshot {
        static EMPTY: std::sync::LazyLock<MobSnapshot> =
            std::sync::LazyLock::new(MobSnapshot::default);
        &EMPTY
    }

    /// Replace the contents with `mobs` and rebuild both indices, reusing
    /// every buffer.
    pub fn rebuild(&mut self, mobs: impl IntoIterator<Item = AiMob>) {
        self.mobs.clear();
        self.mobs.extend(mobs);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.cells.clear();
        self.order.clear();
        self.keyed.clear();
        self.max_half_extent = 0.0;
        for (i, m) in self.mobs.iter().enumerate() {
            self.by_id.insert(m.id, i as u32);
            if m.active {
                self.keyed.push((cell_of(m.pos.x, m.pos.z), i as u32));
                self.max_half_extent = self.max_half_extent.max(half_extent(m));
            }
        }
        self.keyed.sort_unstable();
        let mut start = 0usize;
        while start < self.keyed.len() {
            let key = self.keyed[start].0;
            let end = start
                + self.keyed[start..]
                    .iter()
                    .take_while(|(k, _)| *k == key)
                    .count();
            self.cells.insert(key, (start as u32, (end - start) as u32));
            self.order
                .extend(self.keyed[start..end].iter().map(|&(_, i)| i));
            start = end;
        }
    }

    /// The mob with stable id `id` and its snapshot index, active or not.
    pub fn by_id(&self, id: u64) -> Option<(usize, &AiMob)> {
        let i = *self.by_id.get(&id)? as usize;
        Some((i, &self.mobs[i]))
    }

    /// The ACTIVE mob with stable id `id` — the targetable-entity lookup
    /// (`None` for an unknown, dead or frozen mob).
    pub fn live(&self, id: u64) -> Option<&AiMob> {
        self.by_id(id).map(|(_, m)| m).filter(|m| m.active)
    }

    /// Widest horizontal half-extent of any active body. A query for bodies
    /// that could OVERLAP a region pads its reach by this.
    pub fn max_half_extent(&self) -> f32 {
        self.max_half_extent
    }

    /// Every ACTIVE mob whose feet lie within the horizontal square of
    /// half-size `reach` around `pos`, with its snapshot index. Candidates
    /// only as far as the caller's own test is concerned (a circle, a body
    /// overlap) — the square is exact, so the caller's narrower test decides.
    /// Visits columns in a fixed order and indices ascending within one, so
    /// the sequence is deterministic for a given snapshot.
    pub fn near(&self, pos: WorldPos, reach: f32) -> impl Iterator<Item = (usize, &AiMob)> + '_ {
        let reach = f64::from(reach.max(0.0));
        let (x0, z0) = cell_of(pos.x - reach, pos.z - reach);
        let (x1, z1) = cell_of(pos.x + reach, pos.z + reach);
        (x0..=x1)
            .flat_map(move |cx| (z0..=z1).map(move |cz| (cx, cz)))
            .filter_map(|key| self.cells.get(&key))
            .flat_map(|&(start, len)| &self.order[start as usize..(start + len) as usize])
            .map(|&i| (i as usize, &self.mobs[i as usize]))
            .filter(move |(_, m)| {
                (m.pos.x - pos.x).abs() <= reach && (m.pos.z - pos.z).abs() <= reach
            })
    }
}

/// Horizontal half-extent of a snapshot body: its half-length for a long
/// body, else its half-width.
fn half_extent(m: &AiMob) -> f32 {
    let s = super::def(m.kind).size;
    s.half_length.unwrap_or(s.half_width).max(s.half_width)
}

#[cfg(test)]
mod tests;
