use rustc_hash::FxHashMap;

use petramond_math::world_pos::WorldPos;

use super::brain::AiMob;

const CELL_SIZE: f64 = 16.0;

type CellKey = (i32, i32);

fn cell_of(x: f64, z: f64) -> CellKey {
    (
        (x / CELL_SIZE).floor() as i32,
        (z / CELL_SIZE).floor() as i32,
    )
}

#[derive(Default)]
pub(super) struct ColumnIndex {
    cells: FxHashMap<CellKey, (u32, u32)>,
    order: Vec<u32>,
    keyed: Vec<(CellKey, u32)>,
}

impl ColumnIndex {
    pub(super) fn rebuild(&mut self, points: impl IntoIterator<Item = (WorldPos, u32)>) {
        self.cells.clear();
        self.order.clear();
        self.keyed.clear();
        self.keyed
            .extend(points.into_iter().map(|(p, i)| (cell_of(p.x, p.z), i)));
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

    pub(super) fn candidates(&self, pos: WorldPos, reach: f64) -> impl Iterator<Item = u32> + '_ {
        let (x0, z0) = cell_of(pos.x - reach, pos.z - reach);
        let (x1, z1) = cell_of(pos.x + reach, pos.z + reach);
        (x0..=x1)
            .flat_map(move |cx| (z0..=z1).map(move |cz| (cx, cz)))
            .filter_map(|key| self.cells.get(&key))
            .flat_map(|&(start, len)| &self.order[start as usize..(start + len) as usize])
            .copied()
    }
}

#[derive(Default)]
pub struct MobSnapshot {
    mobs: Vec<AiMob>,
    by_id: FxHashMap<u64, u32>,
    grid: ColumnIndex,
    max_half_extent: f32,
}

impl MobSnapshot {
    pub fn from_mobs(mobs: impl IntoIterator<Item = AiMob>) -> Self {
        let mut snapshot = MobSnapshot::default();
        snapshot.rebuild(mobs);
        snapshot
    }

    pub fn empty() -> &'static MobSnapshot {
        static EMPTY: std::sync::LazyLock<MobSnapshot> =
            std::sync::LazyLock::new(MobSnapshot::default);
        &EMPTY
    }

    pub fn rebuild(&mut self, mobs: impl IntoIterator<Item = AiMob>) {
        self.mobs.clear();
        self.mobs.extend(mobs);
        self.reindex();
    }

    fn reindex(&mut self) {
        self.by_id.clear();
        self.max_half_extent = 0.0;
        for (i, m) in self.mobs.iter().enumerate() {
            self.by_id.insert(m.id, i as u32);
            if m.active {
                self.max_half_extent = self.max_half_extent.max(half_extent(m));
            }
        }
        self.grid.rebuild(
            self.mobs
                .iter()
                .enumerate()
                .filter(|(_, m)| m.active)
                .map(|(i, m)| (m.pos, i as u32)),
        );
    }

    pub fn by_id(&self, id: u64) -> Option<(usize, &AiMob)> {
        let i = *self.by_id.get(&id)? as usize;
        Some((i, &self.mobs[i]))
    }

    pub fn live(&self, id: u64) -> Option<&AiMob> {
        self.by_id(id).map(|(_, m)| m).filter(|m| m.active)
    }

    pub fn max_half_extent(&self) -> f32 {
        self.max_half_extent
    }

    pub fn near(&self, pos: WorldPos, reach: f32) -> impl Iterator<Item = (usize, &AiMob)> + '_ {
        let reach = f64::from(reach.max(0.0));
        self.grid
            .candidates(pos, reach)
            .map(|i| (i as usize, &self.mobs[i as usize]))
            .filter(move |(_, m)| {
                (m.pos.x - pos.x).abs() <= reach && (m.pos.z - pos.z).abs() <= reach
            })
    }
}

fn half_extent(m: &AiMob) -> f32 {
    let s = super::def(m.kind).size;
    s.half_length.unwrap_or(s.half_width).max(s.half_width)
}

#[cfg(test)]
mod tests;
