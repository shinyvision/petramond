//! Confinement detection: decide whether a mob is captive in a closed-off
//! area (a pen). The probe is a flood-fill over navigation footholds: if the
//! fill runs dry while its footprint still fits inside [`MAX_REGION_SPAN`]²
//! (24×24), the mob is confined and the discovered region — every foothold it
//! can reach — is returned for the shared [`RegionCache`], so pen-mates reuse
//! one fill and wander can pick destinations straight from the region.
//!
//! Confinement means ENCLOSED, not cramped: a 12×9 fenced pasture is just as
//! confined as a 2×2 box. Anything whose reachable footprint outgrows 24×24
//! is free.

use std::sync::Arc;

use rustc_hash::FxHashSet;

use crate::mathh::IVec3;
use crate::mob::path::{body_clear, body_layer_clear, is_navigation_foothold_with, PathParams};

/// Game ticks between confined-state re-evaluations for one mob.
pub const CHECK_INTERVAL: u8 = 60;

/// Maximum x/z span (in cells) of a reachable region that still counts as
/// confined. The fill gives up — mob is free — the moment its footprint
/// outgrows this in either horizontal axis.
///
/// 48 covers essentially every pasture a player actually builds, so real
/// pens get the GOOD wander behavior (region-picking) instead of the
/// probe-and-cancel fallback; enclosures past it are rare enough that the
/// fallback's weaker quality doesn't matter. The free-mob cost of a larger
/// span barely moves: the fill is a DFS, so open ground exits after ~one
/// straight run of `MAX_REGION_SPAN` cells either way.
pub const MAX_REGION_SPAN: i32 = 48;

/// Hard cap on visited cells: a multi-level structure can stack floors inside
/// the 48×48 footprint, but past this the space is roomy enough to call free.
const MAX_REGION_CELLS: usize = (MAX_REGION_SPAN * MAX_REGION_SPAN * 2) as usize;

/// How far (cells) outside a region's foothold bounds a block change can sit
/// and still alter its reachability: walls sit one cell outside the footholds,
/// and an over-the-top fence escape engages a step block up to two cells above.
const INVALIDATION_MARGIN: i32 = 2;

/// Upper bound on cached regions; the oldest is dropped past it. Pens are
/// player-built and rare — this is a memory backstop, not a working limit.
const MAX_CACHED_REGIONS: usize = 32;

/// A cached region expires after this many ticks even without a block change
/// (one minute) — INSURANCE, not the main invalidation: any change funnel
/// that bypasses the announce choke point (door toggles did, before they got
/// an explicit push) or a future filtering mistake heals here instead of
/// pinning stale confinement forever. Expiry reads as `is_live() == false`,
/// so the holding mobs re-check off-cadence and the first one re-fills the
/// cache for its whole pen — one bounded fill per pen per minute.
const REGION_MAX_AGE_TICKS: u64 = 1200;

/// Cardinal directions only; diagonals don't open new escape routes for a
/// confinement test and omitting them halves the probe work.
const DIRS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// The complete reachable area of one confined mob: every navigation foothold
/// the flood-fill reached, plus the bounds used for change invalidation.
pub struct ConfinedRegion {
    /// Every reachable foothold cell, SORTED — one canonical order no matter
    /// which pen-mate's fill discovered the region, so wander's indexed picks
    /// stay deterministic across sessions.
    pub cells: Vec<IVec3>,
    /// Membership mirror of [`cells`](Self::cells).
    set: FxHashSet<IVec3>,
    /// Inclusive foothold-cell bounds.
    min: IVec3,
    max: IVec3,
}

impl ConfinedRegion {
    pub fn contains(&self, cell: IVec3) -> bool {
        self.set.contains(&cell)
    }

    /// Whether a block change at `pos` could alter this region's shape or its
    /// enclosure (within [`INVALIDATION_MARGIN`] of the foothold bounds).
    pub fn touched_by(&self, pos: IVec3) -> bool {
        pos.x >= self.min.x - INVALIDATION_MARGIN
            && pos.x <= self.max.x + INVALIDATION_MARGIN
            && pos.y >= self.min.y - INVALIDATION_MARGIN
            && pos.y <= self.max.y + INVALIDATION_MARGIN
            && pos.z >= self.min.z - INVALIDATION_MARGIN
            && pos.z <= self.max.z + INVALIDATION_MARGIN
    }
}

/// Flood-fill the reachable footholds from `start`. Returns the region when it
/// is genuinely closed off within [`MAX_REGION_SPAN`]², `None` when the mob is
/// free — the footprint outgrew the span, the fill visited more than
/// [`MAX_REGION_CELLS`], or it reached a cell the world hasn't finished
/// streaming (`loaded`, the `physics_cell_final_at` predicate): an unknown
/// border cell might be open, so an edge-of-stream pen must read free rather
/// than lock stale confinement in.
///
/// `solid`, `support`, `water`, and `step_allowed` match the pathfinder's
/// semantics (see `mob::nav`): the fill must agree with real routes — a lone
/// fence refuses the jump from below, while a step block beside it opens the
/// way over (a pen with a step inside is genuinely escapable). Mobs that are
/// not on a foothold (swimming, mid-air) are never confined.
#[allow(clippy::too_many_arguments)]
pub fn confined_region(
    start: IVec3,
    params: PathParams,
    solid: &impl Fn(IVec3) -> bool,
    support: &impl Fn(IVec3) -> bool,
    water: &impl Fn(IVec3) -> bool,
    step_allowed: &impl Fn(IVec3, IVec3) -> bool,
    loaded: &impl Fn(IVec3) -> bool,
) -> Option<ConfinedRegion> {
    let foothold = |c: IVec3| is_navigation_foothold_with(c, params, solid, support, water);
    if !foothold(start) {
        return None;
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
            let side = c + IVec3::new(dx, 0, dz);
            if !loaded(side) {
                return None;
            }

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

            // Jump up one block.
            let up = side + IVec3::Y;
            if foothold(up)
                && step_allowed(c, up)
                && body_layer_clear(c + IVec3::Y * params.head_cells(), params, solid)
            {
                if !reach(up) {
                    return None;
                }
                continue;
            }

            // Flat step.
            if foothold(side) && step_allowed(c, side) {
                if !reach(side) {
                    return None;
                }
                continue;
            }

            // Descend to the first foothold within max_drop.
            if body_clear(side, params, solid) {
                for dy in 1..=params.max_drop {
                    let down = side - IVec3::Y * dy;
                    if !loaded(down) {
                        return None;
                    }
                    if solid(down) {
                        break;
                    }
                    if foothold(down) && step_allowed(c, down) {
                        if !reach(down) {
                            return None;
                        }
                        break;
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

/// The shared store of live confined regions, owned by the mob manager: one
/// fill serves every mob in the same pen; a nav-relevant block change near a
/// region drops it immediately, and [`REGION_MAX_AGE_TICKS`] bounds how long
/// any region may live regardless.
#[derive(Default)]
pub struct RegionCache {
    /// Each live region with the tick it was filled on.
    regions: Vec<(Arc<ConfinedRegion>, u64)>,
    /// The current game tick, fed once per mob tick by the manager.
    now: u64,
}

impl RegionCache {
    /// Advance the cache's clock and expire regions past their maximum age.
    /// Called once at the top of each mob tick.
    pub fn set_now(&mut self, now: u64) {
        self.now = now;
        self.regions
            .retain(|(_, born)| now.saturating_sub(*born) <= REGION_MAX_AGE_TICKS);
    }

    /// The live region whose reachable set contains `cell`, if any.
    pub fn region_at(&self, cell: IVec3) -> Option<Arc<ConfinedRegion>> {
        self.regions
            .iter()
            .find(|(r, _)| r.touched_by(cell) && r.contains(cell))
            .map(|(r, _)| r.clone())
    }

    /// Store a freshly-filled region and hand back its shared handle.
    pub fn insert(&mut self, region: ConfinedRegion) -> Arc<ConfinedRegion> {
        if self.regions.len() >= MAX_CACHED_REGIONS {
            self.regions.remove(0);
        }
        let arc = Arc::new(region);
        self.regions.push((arc.clone(), self.now));
        arc
    }

    /// Whether `region` is still in the cache (i.e. neither a block change
    /// nor age dropped it). Instances holding a stale handle must re-evaluate.
    pub fn is_live(&self, region: &Arc<ConfinedRegion>) -> bool {
        self.regions.iter().any(|(r, _)| Arc::ptr_eq(r, region))
    }

    /// Drop every region a changed block could have altered. `all` drops
    /// everything (the change buffer overflowed, so positions are unknown).
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
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
    use crate::mob::path::PathParams;
    use crate::world::World;

    /// An `n`×`n` chunk grid of flat grass. The fill treats unloaded borders
    /// as inconclusive (free), so open-field tests pass regardless of grid
    /// size; pens must fit inside the grid.
    fn flat_world_n(n: i32, mut edit: impl FnMut(&mut Chunk, i32, i32)) -> World {
        let mut world = World::new(0, 1);
        for cx in 0..n {
            for cz in 0..n {
                let mut chunk = Chunk::new(cx, cz);
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        chunk.set_block(x, 63, z, Block::Grass);
                        chunk.set_biome(x, z, crate::biome::Biome::Plains.id());
                    }
                }
                edit(&mut chunk, cx, cz);
                world.insert_chunk_for_test(ChunkPos::new(cx, cz), chunk);
            }
        }
        world
    }

    /// The default 3×3 grid (48×48 blocks) — room for ordinary test pens.
    fn flat_world(edit: impl FnMut(&mut Chunk, i32, i32)) -> World {
        flat_world_n(3, edit)
    }

    /// Build stone walls (up to `y1` exclusive) around the WORLD-coord rect
    /// [x0, x1]×[z0, z1] into whichever chunk each wall cell lands in, on an
    /// `n`×`n` chunk grid.
    fn walled_world_n(n: i32, x0: i32, z0: i32, x1: i32, z1: i32, height: i32) -> World {
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

    fn walled_world(x0: i32, z0: i32, x1: i32, z1: i32, height: i32) -> World {
        walled_world_n(3, x0, z0, x1, z1, height)
    }

    fn params() -> PathParams {
        PathParams::for_body(2, 0.45)
    }

    fn probe(world: &World, start: IVec3) -> Option<ConfinedRegion> {
        let solid = crate::mob::nav::nav_solid_fn(world);
        let support = crate::mob::nav::nav_support_fn(world, params().half_width);
        let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
        let step = crate::mob::nav::partial_step_gate(world, params(), 1.4);
        let loaded = |c: IVec3| world.physics_cell_final_at(c.x, c.y, c.z);
        confined_region(start, params(), &solid, &support, &water, &step, &loaded)
    }

    fn check(world: &World, start: IVec3) -> bool {
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
        // 18×18 interior: far more than the old 33-cell "cramped" rule, but
        // enclosed — the 2026-07-20 semantics change this file exists for.
        let world = walled_world(14, 14, 33, 33, 4);
        assert!(check(&world, MID), "an enclosed pasture is confined");
    }

    #[test]
    fn a_pen_wider_than_the_span_cap_is_not_confined() {
        // 51-cell interior span in x: outgrows MAX_REGION_SPAN (48). Needs a
        // 5×5 chunk grid so the pen (and its surroundings) are loaded.
        let world = walled_world_n(5, 10, 20, 62, 27, 4);
        assert!(!check(&world, IVec3::new(36, 64, 24)));
    }

    #[test]
    fn a_large_pasture_up_to_the_span_cap_is_confined() {
        // 38×38 interior: the whole point of the 48 cap — real player-built
        // pastures read as confined and get region-picking wander.
        let world = walled_world(5, 5, 44, 44, 4);
        assert!(check(&world, MID));
    }

    #[test]
    fn the_region_covers_the_whole_pen_sorted() {
        let world = walled_world(21, 21, 27, 27, 4);
        let region = probe(&world, MID).expect("confined");
        // 5×5 interior.
        assert_eq!(region.cells.len(), 25);
        assert!(region.contains(IVec3::new(22, 64, 22)));
        assert!(!region.contains(IVec3::new(20, 64, 24)), "outside the wall");
        let mut sorted = region.cells.clone();
        sorted.sort_unstable_by_key(|c| (c.x, c.z, c.y));
        assert_eq!(region.cells, sorted, "canonical order for deterministic picks");
    }

    #[test]
    fn doorway_makes_pen_non_confined() {
        let mut world = walled_world(21, 21, 27, 27, 4);
        for y in 64..66 {
            assert!(world.set_block_world(27, y, 24, Block::Air));
        }
        assert!(!check(&world, MID), "a door should break confinement");
    }

    /// A 5×5 pen of one-high fences. The fill must treat the fence as a wall
    /// even though a mob's physical jump could clear it.
    fn fence_pen(extra: impl FnOnce(&mut World)) -> World {
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
        // A block inside the pen beside the fence is an honest escape route:
        // jump onto the block, walk over the fence top, drop outside.
        let world = fence_pen(|w| {
            assert!(w.set_block_world(26, 64, 24, Block::Dirt));
        });
        assert!(!check(&world, MID), "a step beside the fence opens the pen");
    }

    #[test]
    fn swimming_mob_is_not_confined() {
        let world = flat_world(|chunk, cx, cz| {
            if (cx, cz) == (1, 1) {
                chunk.set_water(8, 64, 8, Block::Water, 0);
                chunk.set_water(8, 65, 8, Block::Water, 0);
            }
        });
        // Feet submerged with no dry foothold: not "confined", just swimming.
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

        // A change far away keeps the region; one at the wall drops it.
        cache.invalidate(&[IVec3::new(0, 64, 0)], false);
        assert!(cache.is_live(&arc));
        cache.invalidate(&[IVec3::new(27, 64, 24)], false);
        assert!(!cache.is_live(&arc), "a wall change invalidates the pen");
        assert!(cache.region_at(mate).is_none());
    }

    #[test]
    fn a_cached_region_expires_after_its_maximum_age() {
        // Insurance against invalidation funnels the choke point never sees:
        // a stale region must die of old age, not live forever.
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
