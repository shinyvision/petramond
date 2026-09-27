use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::mob::path::{body_clear, body_layer_clear, is_navigation_foothold_with, PathParams};
use petramond_math::math::IVec3;

pub const CHECK_INTERVAL: u8 = 60;

pub const FREE_CHECK_INTERVAL: u16 = 1200;

const FREE_RECHECK_MOVE: i32 = 8;

pub fn free_verdict_stale(
    cell: IVec3,
    checked_at: IVec3,
    nav_rev: u64,
    checked_rev: u64,
    free_age: u16,
) -> bool {
    nav_rev != checked_rev
        || free_age >= FREE_CHECK_INTERVAL
        || (cell.x - checked_at.x).abs() >= FREE_RECHECK_MOVE
        || (cell.z - checked_at.z).abs() >= FREE_RECHECK_MOVE
        || cell.y != checked_at.y
}

pub const MAX_REGION_SPAN: i32 = 48;

const MAX_REGION_CELLS: usize = (MAX_REGION_SPAN * MAX_REGION_SPAN * 2) as usize;

const INVALIDATION_MARGIN: i32 = 2;

const MAX_CACHED_REGIONS: usize = 32;

const REGION_MAX_AGE_TICKS: u64 = 1200;

const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

pub struct ConfinedRegion {
    pub cells: Vec<IVec3>,
    set: FxHashSet<IVec3>,
    min: IVec3,
    max: IVec3,
}

impl ConfinedRegion {
    pub fn contains(&self, cell: IVec3) -> bool {
        self.set.contains(&cell)
    }

    pub fn touched_by(&self, pos: IVec3) -> bool {
        pos.x >= self.min.x - INVALIDATION_MARGIN
            && pos.x <= self.max.x + INVALIDATION_MARGIN
            && pos.y >= self.min.y - INVALIDATION_MARGIN
            && pos.y <= self.max.y + INVALIDATION_MARGIN
            && pos.z >= self.min.z - INVALIDATION_MARGIN
            && pos.z <= self.max.z + INVALIDATION_MARGIN
    }
}

#[allow(clippy::too_many_arguments)]
pub fn confined_region(
    start: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    fluid: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    loaded: &impl Fn(IVec3) -> bool,
) -> Option<ConfinedRegion> {
    let memo = crate::mob::path::CellMemo::<512>::default();
    let foothold = |c: IVec3| {
        memo.get(c, |c| {
            is_navigation_foothold_with(c, params, solid, support, fluid)
        })
    };
    if !foothold(start) {
        return None;
    }

    for (dx, dz) in DIRS {
        if escapes_span(
            start,
            (dx, dz),
            params,
            solid,
            &foothold,
            step_allowed,
            loaded,
        ) {
            return None;
        }
    }

    let mut set = FxHashSet::default();
    let mut queue = Vec::new();
    let (mut min, mut max) = (start, start);
    set.insert(start);
    queue.push(start);

    while let Some(c) = queue.pop() {
        if set.len() > MAX_REGION_CELLS {
            return None;
        }

        for (dx, dz) in DIRS {
            let mut reach = |cell: IVec3| -> bool {
                if set.insert(cell) {
                    min = min.min(cell);
                    max = max.max(cell);
                    if max.x - min.x >= MAX_REGION_SPAN || max.z - min.z >= MAX_REGION_SPAN {
                        return false;
                    }
                    queue.push(cell);
                }
                true
            };
            match step_from(c, (dx, dz), params, solid, &foothold, step_allowed, loaded) {
                Step::Unloaded => return None,
                Step::Blocked => {}
                Step::To(next) => {
                    if !reach(next) {
                        return None;
                    }
                }
            }
        }
    }

    let mut cells: Vec<IVec3> = set.iter().copied().collect();
    cells.sort_unstable_by_key(|c| (c.x, c.z, c.y));
    Some(ConfinedRegion {
        cells,
        set,
        min,
        max,
    })
}

enum Step {
    Unloaded,
    Blocked,
    To(IVec3),
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn step_from(
    c: IVec3,
    (dx, dz): (i32, i32),
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    foothold: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    loaded: &impl Fn(IVec3) -> bool,
) -> Step {
    let side = c + IVec3::new(dx, 0, dz);
    if !loaded(side) {
        return Step::Unloaded;
    }

    let up = side + IVec3::Y;
    if foothold(up)
        && step_allowed(c, up)
        && body_layer_clear(c + IVec3::Y * params.head_cells(), params, solid)
    {
        return Step::To(up);
    }

    if foothold(side) && step_allowed(c, side) {
        return Step::To(side);
    }

    if body_clear(side, params, solid) {
        for dy in 1..=params.max_drop {
            let down = side - IVec3::Y * dy;
            if !loaded(down) {
                return Step::Unloaded;
            }
            if solid(down) {
                break;
            }
            if foothold(down) && step_allowed(c, down) {
                return Step::To(down);
            }
        }
    }
    Step::Blocked
}

#[allow(clippy::too_many_arguments)]
fn escapes_span(
    start: IVec3,
    dir: (i32, i32),
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    foothold: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    loaded: &impl Fn(IVec3) -> bool,
) -> bool {
    let mut c = start;
    for _ in 0..MAX_REGION_SPAN {
        match step_from(c, dir, params, solid, foothold, step_allowed, loaded) {
            Step::Unloaded => return true,
            Step::Blocked => return false,
            Step::To(next) => c = next,
        }
    }
    true
}

#[derive(Default)]
pub struct RegionCache {
    regions: Vec<(Arc<ConfinedRegion>, u64)>,
    now: u64,
}

impl RegionCache {
    pub fn set_now(&mut self, now: u64) {
        self.now = now;
        self.regions
            .retain(|(_, born)| now.saturating_sub(*born) <= REGION_MAX_AGE_TICKS);
    }

    pub fn region_at(&self, cell: IVec3) -> Option<Arc<ConfinedRegion>> {
        self.regions
            .iter()
            .find(|(r, _)| r.touched_by(cell) && r.contains(cell))
            .map(|(r, _)| r.clone())
    }

    pub fn insert(&mut self, region: ConfinedRegion) -> Arc<ConfinedRegion> {
        if self.regions.len() >= MAX_CACHED_REGIONS {
            self.regions.remove(0);
        }
        let arc = Arc::new(region);
        self.regions.push((arc.clone(), self.now));
        arc
    }

    pub fn is_live(&self, region: &Arc<ConfinedRegion>) -> bool {
        self.regions.iter().any(|(r, _)| Arc::ptr_eq(r, region))
    }

    pub fn invalidate(&mut self, changed: &[IVec3], all: bool) {
        if all {
            self.regions.clear();
            return;
        }
        if changed.is_empty() || self.regions.is_empty() {
            return;
        }
        self.regions
            .retain(|(r, _)| !changed.iter().any(|&p| r.touched_by(p)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mob::path::PathParams;
    use crate::world::ServerWorld;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};

    fn flat_world_n(n: i32, mut edit: impl FnMut(&mut Chunk, i32, i32)) -> ServerWorld {
        let mut world = ServerWorld::new(0, 1);
        for cx in 0..n {
            for cz in 0..n {
                let mut chunk = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        chunk.set_block(x, 63, z, Block::Grass);
                        chunk.set_biome(x, z, petramond_world::biome::Biome::PLAINS.id());
                    }
                }
                edit(&mut chunk, cx, cz);
                world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
            }
        }
        world
    }

    fn flat_world(edit: impl FnMut(&mut Chunk, i32, i32)) -> ServerWorld {
        flat_world_n(3, edit)
    }

    fn walled_world_n(n: i32, x0: i32, z0: i32, x1: i32, z1: i32, height: i32) -> ServerWorld {
        flat_world_n(n, |chunk, cx, cz| {
            for wx in x0..=x1 {
                for wz in z0..=z1 {
                    let on_rim = wx == x0 || wx == x1 || wz == z0 || wz == z1;
                    if !on_rim {
                        continue;
                    }
                    let (lx, lz) = (wx - cx * CHUNK_SX as i32, wz - cz * CHUNK_SZ as i32);
                    if (0..CHUNK_SX as i32).contains(&lx) && (0..CHUNK_SZ as i32).contains(&lz) {
                        for y in 64..64 + height {
                            chunk.set_block(lx as usize, y as usize, lz as usize, Block::Stone);
                        }
                    }
                }
            }
        })
    }

    fn walled_world(x0: i32, z0: i32, x1: i32, z1: i32, height: i32) -> ServerWorld {
        walled_world_n(3, x0, z0, x1, z1, height)
    }

    fn params() -> PathParams {
        PathParams::for_body(2, 0.45)
    }

    fn probe(world: &ServerWorld, start: IVec3) -> Option<ConfinedRegion> {
        let cursor = world.cursor();
        let solid = crate::mob::nav::nav_solid_fn(&cursor);
        let support = crate::mob::nav::nav_support_fn(&cursor, params().half_width);
        let fluid = crate::mob::nav::nav_fluid_fn(&cursor);
        let step = crate::mob::nav::navigation_step_gate(&cursor, params(), 1.4);
        let loaded = crate::mob::nav::nav_loaded_fn(&cursor);
        confined_region(start, params(), &solid, &support, &fluid, &step, &loaded)
    }

    fn check(world: &ServerWorld, start: IVec3) -> bool {
        probe(world, start).is_some()
    }

    const MID: IVec3 = IVec3::new(24, 64, 24);

    #[test]
    fn open_field_is_not_confined() {
        let world = flat_world(|_, _, _| {});
        assert!(!check(&world, MID));
    }

    #[test]
    fn small_pen_is_confined() {
        let world = walled_world(21, 21, 27, 27, 4);
        assert!(check(&world, MID), "5×5 pen should read as confined");
    }

    #[test]
    fn a_roomy_pasture_is_still_confined() {
        let world = walled_world(14, 14, 33, 33, 4);
        assert!(check(&world, MID), "an enclosed pasture is confined");
    }

    #[test]
    fn a_pen_wider_than_the_span_cap_is_not_confined() {
        let world = walled_world_n(5, 10, 20, 62, 27, 4);
        assert!(!check(&world, IVec3::new(36, 64, 24)));
    }

    #[test]
    fn a_large_pasture_up_to_the_span_cap_is_confined() {
        let world = walled_world(5, 5, 44, 44, 4);
        assert!(check(&world, MID));
    }

    #[test]
    fn the_region_covers_the_whole_pen_sorted() {
        let world = walled_world(21, 21, 27, 27, 4);
        let region = probe(&world, MID).expect("confined");
        assert_eq!(region.cells.len(), 25);
        assert!(region.contains(IVec3::new(22, 64, 22)));
        assert!(!region.contains(IVec3::new(20, 64, 24)), "outside the wall");
        let mut sorted = region.cells.clone();
        sorted.sort_unstable_by_key(|c| (c.x, c.z, c.y));
        assert_eq!(
            region.cells, sorted,
            "canonical order for deterministic picks"
        );
    }

    #[test]
    fn doorway_makes_pen_non_confined() {
        let mut world = walled_world(21, 21, 27, 27, 4);
        for y in 64..66 {
            assert!(world.set_block_world(27, y, 24, Block::Air));
        }
        assert!(!check(&world, MID), "a door should break confinement");
    }

    fn fence_pen(extra: impl FnOnce(&mut ServerWorld)) -> ServerWorld {
        let mut world = flat_world(|_, _, _| {});
        for i in 21..=27 {
            for (x, z) in [(21, i), (27, i), (i, 21), (i, 27)] {
                assert!(world.set_block_world(x, 64, z, Block::OakFence));
            }
        }
        extra(&mut world);
        world
    }

    #[test]
    fn small_fence_pen_is_confined() {
        let world = fence_pen(|_| {});
        assert!(
            check(&world, MID),
            "a one-high fence pen holds: the fill must not hop the fence"
        );
    }

    #[test]
    fn fence_pen_with_a_gap_is_not_confined() {
        let world = fence_pen(|w| {
            assert!(w.set_block_world(27, 64, 24, Block::Air));
        });
        assert!(!check(&world, MID), "a gap in the fence breaks the pen");
    }

    #[test]
    fn fence_pen_with_a_step_up_inside_is_not_confined() {
        let world = fence_pen(|w| {
            assert!(w.set_block_world(26, 64, 24, Block::Dirt));
        });
        assert!(!check(&world, MID), "a step beside the fence opens the pen");
    }

    #[test]
    fn swimming_mob_is_not_confined() {
        let world = flat_world(|chunk, cx, cz| {
            if (cx, cz) == (1, 1) {
                chunk.set_fluid(8, 64, 8, Block::Water, 0);
                chunk.set_fluid(8, 65, 8, Block::Water, 0);
            }
        });
        assert!(!check(&world, IVec3::new(24, 65, 24)));
    }

    #[test]
    fn cache_shares_lookups_and_invalidates_on_nearby_changes() {
        let world = walled_world(21, 21, 27, 27, 4);
        let region = probe(&world, MID).expect("confined");
        let mut cache = RegionCache::default();
        let arc = cache.insert(region);
        assert!(cache.is_live(&arc));
        let mate = IVec3::new(22, 64, 26);
        assert!(
            cache.region_at(mate).is_some(),
            "a pen-mate reuses the cached fill"
        );
        assert!(cache.region_at(IVec3::new(5, 64, 5)).is_none());

        cache.invalidate(&[IVec3::new(0, 64, 0)], false);
        assert!(cache.is_live(&arc));
        cache.invalidate(&[IVec3::new(27, 64, 24)], false);
        assert!(!cache.is_live(&arc), "a wall change invalidates the pen");
        assert!(cache.region_at(mate).is_none());
    }

    #[test]
    fn a_cached_region_expires_after_its_maximum_age() {
        let world = walled_world(21, 21, 27, 27, 4);
        let region = probe(&world, MID).expect("confined");
        let mut cache = RegionCache::default();
        cache.set_now(100);
        let arc = cache.insert(region);
        cache.set_now(100 + REGION_MAX_AGE_TICKS);
        assert!(cache.is_live(&arc), "a region within its age stays live");
        cache.set_now(101 + REGION_MAX_AGE_TICKS);
        assert!(!cache.is_live(&arc), "past the age it expires");
        assert!(cache.region_at(MID).is_none());
    }

    #[test]
    fn cache_overflow_flag_drops_everything() {
        let world = walled_world(21, 21, 27, 27, 4);
        let region = probe(&world, MID).expect("confined");
        let mut cache = RegionCache::default();
        let arc = cache.insert(region);
        cache.invalidate(&[], true);
        assert!(!cache.is_live(&arc));
    }
}
