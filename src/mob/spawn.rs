//! Natural mob spawning: one attempt per game tick.
//!
//! Each tick the manager calls [`attempt`], which picks a random column in the
//! loaded area, picks a species that still has population room, and tests the site
//! against the universal "definitely no" rules -- too near the player, no footing,
//! no room for the body -- and then the species' own [`SpawnRule`](super::SpawnRule)
//! (biome + the block it stands on). If everything passes it returns the [`Spawn`]s
//! for the manager to apply; otherwise the tick simply spawns nothing.
//!
//! An attempt waits only for the mob census within nine chunks of the player it is
//! spawning around. Mobs saved in still-streaming nearby section records aren't in
//! the live list yet, so the cap would otherwise refill on every world join; unrelated
//! far-edge streaming must not stop local spawning.
//!
//! The population caps live elsewhere as data: per-species on the [`MobDef`] row, and
//! per-category on [`MobCategory`]. This module only enforces them, and only the
//! site/arithmetic logic that's worth pinning is factored out pure and tested below.
//!
//! [`MobDef`]: super::MobDef
//! [`MobCategory`]: super::MobCategory

use mod_api::HostileSpawnCandidate;
use rustc_hash::FxHashSet;

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{
    ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::mathh::{IVec3, Vec3};
use crate::world::{World, VERTICAL_LOAD_RADIUS};

use super::path::{body_clear, is_foothold, PathParams};
use super::{def, defs, Instance, Mob, MobCategory, MobRng};

/// Closest a natural spawn may appear to the player (blocks). Inside this, no spawn.
const MIN_PLAYER_DIST: f32 = 50.0;
/// Farthest any natural spawn may appear from its player anchor. The nine-chunk
/// census margin encloses this range even when the player stands at a chunk edge.
const MAX_PLAYER_DIST: f32 = 128.0;

/// How many times to resample a column offset to land one inside the loaded disc
/// before giving up for this tick (a near-degenerate disc could miss every time).
const COLUMN_TRIES: u32 = 8;
/// Radius around the first valid site where herd/pack members may be placed.
const GROUP_RADIUS: i32 = 4;
/// Attempts per extra group member to find another nearby site satisfying the same
/// species spawn rule.
const GROUP_MEMBER_TRIES: u32 = 24;
pub(crate) const HOSTILE_SPAWN_ATTEMPTS: u32 = 32;
const HOSTILE_SPAWN_CHUNK_RADIUS: i32 = 8;
/// One chunk beyond the 128-block hostile spawn/despawn range. This margin makes the
/// local live list trustworthy before applying population caps without waiting for
/// the entire render-distance disc.
const MOB_CENSUS_CHUNK_RADIUS: i32 = HOSTILE_SPAWN_CHUNK_RADIUS + 1;
const HOSTILE_SPAWN_CHUNKS_PER_PLAYER: u32 = 289;
const HOSTILE_MIN_SPAWN_DIST: f32 = 24.0;
const HOSTILE_SPAWN_SALT: u64 = 0xA11C_0DE5_5A55_0001;

/// A spawn the manager should perform: a species at a feet position, facing `yaw`.
pub(super) struct Spawn {
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
}

/// A core-selected hostile spawn candidate plus the spawn transform core will use
/// if a registered hostile spawner admits it.
pub(crate) struct HostileSpawnSite {
    pub candidate: HostileSpawnCandidate,
    pub pos: Vec3,
    pub yaw: f32,
}

#[derive(Copy, Clone)]
struct HostileSpawnAnchor {
    pos: Vec3,
    chunk: ChunkPos,
}

impl HostileSpawnAnchor {
    fn new(pos: Vec3) -> Self {
        Self {
            pos,
            chunk: chunk_pos_at(pos),
        }
    }
}

/// Per-tick hostile spawn-cap data
/// a global cap scaled by the union of each player's 17x17 spawnable chunks,
/// plus a per-player local cap that gates each candidate chunk.
pub(crate) struct HostileSpawnPlan {
    anchors: Vec<HostileSpawnAnchor>,
    spawnable_chunks: Vec<ChunkPos>,
    attempt_chunks: Vec<ChunkPos>,
    local_counts: Vec<u32>,
    hostile_count: u32,
    hostile_cap: u32,
}

/// Run one natural-spawn attempt. `room_for(kind)` reports how many more individuals
/// fit under a species' population caps (the manager supplies it from the live set).
/// Returns the spawns to perform, or `None` if this tick's site/species didn't qualify.
pub(super) fn attempt(
    world: &World,
    player_pos: Vec3,
    rng: &mut MobRng,
    room_for: impl Fn(Mob) -> u32,
) -> Option<Vec<Spawn>> {
    // The caps compare against the live mob list, which undercounts while saved
    // mobs are still streaming in with their section records. Spawning through
    // that window refills the caps on top of the mobs about to be restored —
    // a per-session population ratchet.
    if !mob_census_ready(world, player_pos) {
        return None;
    }
    let (cx, cz, render_dist) = world.loaded_area()?;
    // Inset by a chunk so the column (and the neighbours a footing/biome read may
    // touch) are loaded — unloaded reads would just fail the attempt anyway.
    let r = (render_dist - 1).max(0).min(HOSTILE_SPAWN_CHUNK_RADIUS);

    // Pick a species that still has room; the site is then judged for *that* species.
    let kind = choose_kind(rng, &room_for, world.disabled_mods())?;
    let d = def(kind);
    let want = d.spawn_group.roll(rng).min(room_for(kind));

    let (wx, wz) = random_column(rng, cx, cz, r)?;
    let first = spawn_at(world, player_pos, kind, wx, wz, rng)?;
    let mut spawns = Vec::with_capacity(want as usize);
    spawns.push(first);

    let origin = IVec3::new(wx, 0, wz);
    while spawns.len() < want as usize {
        let next = nearby_spawn(world, player_pos, kind, origin, &spawns, rng)?;
        spawns.push(next);
    }
    Some(spawns)
}

fn spawn_at(
    world: &World,
    player_pos: Vec3,
    kind: Mob,
    wx: i32,
    wz: i32,
    rng: &mut MobRng,
) -> Option<Spawn> {
    let pos = spawn_site(world, player_pos, kind, wx, wz)?;
    let yaw = rng.next_f32() * std::f32::consts::TAU;
    Some(Spawn { kind, pos, yaw })
}

fn spawn_site(world: &World, player_pos: Vec3, kind: Mob, wx: i32, wz: i32) -> Option<Vec3> {
    let d = def(kind);
    // The surface to stand on, and the feet cell resting on top of it.
    let ground_y = world.surface_collision_y(wx, wz)?;
    let feet = IVec3::new(wx, ground_y + 1, wz);
    let feet_pos = Vec3::new(wx as f32 + 0.5, feet.y as f32, wz as f32 + 0.5);

    // --- Universal "definitely no" checks. ---
    // Too close to the player.
    if too_close(player_pos, feet_pos, MIN_PLAYER_DIST)
        || !too_close(player_pos, feet_pos, MAX_PLAYER_DIST)
    {
        return None;
    }
    // The ground must have collision AND the body must fit (clearance above the
    // feet) — exactly what a foothold test asserts.
    if !body_fits_at(world, kind, feet) {
        return None;
    }

    // --- Species rule: biome + the block it would stand on. ---
    let biome = Biome::from_id(world.column_biome(wx, wz)?);
    let ground = Block::from_id(world.chunk_block(wx, ground_y, wz));
    if !d.spawn.admits(biome, ground) {
        return None;
    }

    Some(feet_pos)
}

/// Whether `kind` can physically stand with its feet in `feet`.
pub(crate) fn body_fits_at(world: &World, kind: Mob, feet: IVec3) -> bool {
    let d = def(kind);
    let params = PathParams::for_body(d.size.head_cells(), d.size.half_width);
    let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
    if !is_foothold(feet, params, &solid) {
        return false;
    }
    let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
    body_clear(feet, params, &water)
}

pub(crate) fn hostile_spawn_plan(
    world: &World,
    player_positions: &[Vec3],
) -> Option<HostileSpawnPlan> {
    let anchors: Vec<_> = player_positions
        .iter()
        .copied()
        .filter(|&pos| mob_census_ready(world, pos))
        .map(HostileSpawnAnchor::new)
        .collect();
    if anchors.is_empty() {
        return None;
    }

    let spawnable_chunks =
        hostile_spawnable_chunks(&anchors, |chunk| world.chunk_loaded(chunk.cx, chunk.cz));
    if spawnable_chunks.is_empty() {
        return None;
    }

    let list = world.mobs().instances();
    let hostile_count = live_hostile_count(list);
    let hostile_cap = scaled_mob_cap(MobCategory::Hostile.cap(), spawnable_chunks.len() as u32);
    if hostile_count >= hostile_cap {
        return None;
    }

    let local_counts = hostile_local_counts(list, &anchors);
    let attempt_chunks: Vec<_> = spawnable_chunks
        .iter()
        .copied()
        .filter(|&chunk| hostile_chunk_has_local_room(&anchors, &local_counts, chunk))
        .collect();
    if attempt_chunks.is_empty() {
        return None;
    }

    Some(HostileSpawnPlan {
        anchors,
        spawnable_chunks,
        attempt_chunks,
        local_counts,
        hostile_count,
        hostile_cap,
    })
}

fn mob_census_ready(world: &World, player_pos: Vec3) -> bool {
    let center = chunk_pos_at(player_pos);
    world.mob_census_loaded_around(center, MOB_CENSUS_CHUNK_RADIUS)
}

pub(crate) fn hostile_kind_has_room(world: &World, plan: &HostileSpawnPlan, kind: Mob) -> bool {
    let d = def(kind);
    if d.category != MobCategory::Hostile || plan.hostile_count >= plan.hostile_cap {
        return false;
    }
    let species = world
        .mobs()
        .instances()
        .iter()
        .filter(|m| !m.is_dead() && m.kind == kind)
        .count() as u32;
    species < scaled_mob_cap(d.cap, plan.spawnable_chunks.len() as u32)
}

pub(crate) fn hostile_attempt_sites(
    world: &World,
    plan: &HostileSpawnPlan,
    attempt: u32,
) -> Vec<HostileSpawnSite> {
    let Some((wx, wz)) = hostile_candidate_column(world, plan, attempt) else {
        return Vec::new();
    };
    hostile_column_candidates(world, plan, wx, wz)
}

fn hostile_candidate_column(
    world: &World,
    plan: &HostileSpawnPlan,
    attempt: u32,
) -> Option<(i32, i32)> {
    let chunk = hostile_candidate_chunk(world, plan, attempt)?;
    let seed = hostile_attempt_seed(world, attempt);
    let lx = (splitmix(seed ^ 0xC01A_51DE_1234_0001) % CHUNK_SX as u64) as i32;
    let lz = (splitmix(seed ^ 0xC01A_51DE_1234_0002) % CHUNK_SZ as u64) as i32;
    Some((
        chunk.cx * CHUNK_SX as i32 + lx,
        chunk.cz * CHUNK_SZ as i32 + lz,
    ))
}

fn hostile_candidate_chunk(
    world: &World,
    plan: &HostileSpawnPlan,
    attempt: u32,
) -> Option<ChunkPos> {
    let chunks = &plan.attempt_chunks;
    if chunks.is_empty() {
        return None;
    }
    let seed = hostile_attempt_seed(world, attempt);
    let i = (splitmix(seed ^ 0xC01A_51DE_1234_0000) % chunks.len() as u64) as usize;
    Some(chunks[i])
}

fn hostile_attempt_seed(world: &World, attempt: u32) -> u64 {
    (world.seed as u64)
        ^ world.current_tick().wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (attempt as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ HOSTILE_SPAWN_SALT
}

fn hostile_column_candidates(
    world: &World,
    plan: &HostileSpawnPlan,
    wx: i32,
    wz: i32,
) -> Vec<HostileSpawnSite> {
    let chunk = ChunkPos::new(wx >> 4, wz >> 4);
    let Some(range) = hostile_scan_y_range(plan, chunk) else {
        return Vec::new();
    };
    range
        .rev()
        .filter_map(|y| hostile_candidate_at(world, plan, wx, y, wz))
        .collect()
}

fn hostile_scan_y_range(
    plan: &HostileSpawnPlan,
    chunk: ChunkPos,
) -> Option<std::ops::RangeInclusive<i32>> {
    let mut lo = i32::MAX;
    let mut hi = i32::MIN;
    for (i, anchor) in plan.anchors.iter().enumerate() {
        if plan.local_counts[i] >= MobCategory::Hostile.cap()
            || !chunk_in_spawn_range(anchor.chunk, chunk)
        {
            continue;
        }
        let range = hostile_anchor_scan_y_range(anchor.pos)?;
        lo = lo.min(*range.start());
        hi = hi.max(*range.end());
    }
    (lo <= hi).then_some(lo..=hi)
}

fn hostile_anchor_scan_y_range(player_pos: Vec3) -> Option<std::ops::RangeInclusive<i32>> {
    let player_section = SectionPos::from_world(
        player_pos.x.floor() as i32,
        player_pos.y.floor() as i32,
        player_pos.z.floor() as i32,
    )?;
    let lo_cy = (player_section.cy - VERTICAL_LOAD_RADIUS).max(SECTION_MIN_CY);
    let hi_cy = (player_section.cy + VERTICAL_LOAD_RADIUS).min(SECTION_MAX_CY);
    let lo = lo_cy * SECTION_SIZE as i32;
    let hi = (hi_cy + 1) * SECTION_SIZE as i32 - 1;
    Some(lo..=hi)
}

fn hostile_candidate_at(
    world: &World,
    plan: &HostileSpawnPlan,
    wx: i32,
    y: i32,
    wz: i32,
) -> Option<HostileSpawnSite> {
    let pos = Vec3::new(wx as f32 + 0.5, y as f32, wz as f32 + 0.5);
    let nearest = nearest_anchor_pos(&plan.anchors, pos)?;
    if too_close(nearest, pos, HOSTILE_MIN_SPAWN_DIST) || !too_close(nearest, pos, MAX_PLAYER_DIST)
    {
        return None;
    }
    if !body_cell_open(world, wx, y, wz)
        || !body_cell_open(world, wx, y + 1, wz)
        || !world.block_is_full_spawn_support(wx, y - 1, wz)
    {
        return None;
    }
    Some(HostileSpawnSite {
        candidate: HostileSpawnCandidate {
            pos: [pos.x, pos.y, pos.z],
            cell: [wx, y, wz],
            combined_light: world.combined_light6_at_world(wx, y, wz),
            sky_light: world.skylight6_at_world(wx, y, wz),
            block_light: world.blocklight6_at_world(wx, y, wz),
            nearest_player_dist: (nearest - pos).length(),
        },
        pos,
        yaw: yaw_away_from_player(nearest, pos),
    })
}

fn nearest_anchor_pos(anchors: &[HostileSpawnAnchor], pos: Vec3) -> Option<Vec3> {
    anchors
        .iter()
        .min_by(|a, b| {
            let da = dist2(a.pos, pos);
            let db = dist2(b.pos, pos);
            da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|a| a.pos)
}

fn hostile_spawnable_chunks(
    anchors: &[HostileSpawnAnchor],
    mut loaded: impl FnMut(ChunkPos) -> bool,
) -> Vec<ChunkPos> {
    let mut chunks = Vec::new();
    let mut seen = FxHashSet::default();
    for anchor in anchors {
        for dz in -HOSTILE_SPAWN_CHUNK_RADIUS..=HOSTILE_SPAWN_CHUNK_RADIUS {
            for dx in -HOSTILE_SPAWN_CHUNK_RADIUS..=HOSTILE_SPAWN_CHUNK_RADIUS {
                let chunk = ChunkPos::new(anchor.chunk.cx + dx, anchor.chunk.cz + dz);
                if loaded(chunk) && seen.insert(chunk) {
                    chunks.push(chunk);
                }
            }
        }
    }
    chunks
}

fn hostile_local_counts(list: &[Instance], anchors: &[HostileSpawnAnchor]) -> Vec<u32> {
    let mut counts = vec![0; anchors.len()];
    for mob in live_hostile_mobs(list) {
        let chunk = chunk_pos_at(mob.pos);
        for (i, anchor) in anchors.iter().enumerate() {
            if chunk_in_spawn_range(anchor.chunk, chunk) {
                counts[i] += 1;
            }
        }
    }
    counts
}

fn hostile_chunk_has_local_room(
    anchors: &[HostileSpawnAnchor],
    local_counts: &[u32],
    chunk: ChunkPos,
) -> bool {
    anchors.iter().enumerate().any(|(i, anchor)| {
        local_counts[i] < MobCategory::Hostile.cap() && chunk_in_spawn_range(anchor.chunk, chunk)
    })
}

fn chunk_in_spawn_range(anchor: ChunkPos, chunk: ChunkPos) -> bool {
    (chunk.cx - anchor.cx).abs() <= HOSTILE_SPAWN_CHUNK_RADIUS
        && (chunk.cz - anchor.cz).abs() <= HOSTILE_SPAWN_CHUNK_RADIUS
}

fn live_hostile_count(list: &[Instance]) -> u32 {
    live_hostile_mobs(list).count() as u32
}

fn live_hostile_mobs(list: &[Instance]) -> impl Iterator<Item = &Instance> {
    list.iter()
        .filter(|m| !m.is_dead() && def(m.kind).category == MobCategory::Hostile)
}

fn scaled_mob_cap(base: u32, spawnable_chunks: u32) -> u32 {
    ((base as u64 * spawnable_chunks as u64) / HOSTILE_SPAWN_CHUNKS_PER_PLAYER as u64) as u32
}

fn chunk_pos_at(pos: Vec3) -> ChunkPos {
    ChunkPos::new(pos.x.floor() as i32 >> 4, pos.z.floor() as i32 >> 4)
}

fn dist2(a: Vec3, b: Vec3) -> f32 {
    let (dx, dy, dz) = (a.x - b.x, a.y - b.y, a.z - b.z);
    dx * dx + dy * dy + dz * dz
}

fn body_cell_open(world: &World, wx: i32, y: i32, wz: i32) -> bool {
    world.placement_cell_open(IVec3::new(wx, y, wz)) && !world.water_cell_at(wx, y, wz)
}

fn yaw_away_from_player(player_pos: Vec3, spawn_pos: Vec3) -> f32 {
    let dx = player_pos.x - spawn_pos.x;
    let dz = player_pos.z - spawn_pos.z;
    (-dx).atan2(-dz)
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn nearby_spawn(
    world: &World,
    player_pos: Vec3,
    kind: Mob,
    origin: IVec3,
    existing: &[Spawn],
    rng: &mut MobRng,
) -> Option<Spawn> {
    let r2 = GROUP_RADIUS * GROUP_RADIUS;
    for _ in 0..GROUP_MEMBER_TRIES {
        let dx = rng.next_range(-GROUP_RADIUS, GROUP_RADIUS);
        let dz = rng.next_range(-GROUP_RADIUS, GROUP_RADIUS);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let (wx, wz) = (origin.x + dx, origin.z + dz);
        let Some(spawn) = spawn_at(world, player_pos, kind, wx, wz, rng) else {
            continue;
        };
        if too_near_existing(kind, spawn.pos, existing) {
            continue;
        }
        return Some(spawn);
    }
    None
}

fn too_near_existing(kind: Mob, pos: Vec3, existing: &[Spawn]) -> bool {
    let min_gap = (def(kind).size.half_width * 2.0).max(0.75);
    let min_gap2 = min_gap * min_gap;
    existing.iter().any(|s| {
        let dx = s.pos.x - pos.x;
        let dz = s.pos.z - pos.z;
        dx * dx + dz * dz < min_gap2
    })
}

/// How many more individuals of `kind` fit under both its caps, given the live set.
pub(super) fn room_for(list: &[Instance], kind: Mob) -> u32 {
    let d = def(kind);
    let species = list.iter().filter(|m| m.kind == kind).count() as u32;
    let category = list
        .iter()
        .filter(|m| def(m.kind).category == d.category)
        .count() as u32;
    cap_room(species, d.cap, category, d.category.cap())
}

/// Pure cap arithmetic: the remaining spawn room is constrained by both the
/// per-species and per-category limits. Factored out so the rule is tested without
/// pinning any species' actual cap numbers.
fn cap_room(species: u32, species_cap: u32, category: u32, category_cap: u32) -> u32 {
    species_cap
        .saturating_sub(species)
        .min(category_cap.saturating_sub(category))
}

/// Whether `feet` is within `min_dist` of the player (3-D), so a spawn is forbidden.
fn too_close(player: Vec3, feet: Vec3, min_dist: f32) -> bool {
    let (dx, dy, dz) = (player.x - feet.x, player.y - feet.y, player.z - feet.z);
    dx * dx + dy * dy + dz * dz < min_dist * min_dist
}

/// A random world column `(wx, wz)` inside the loaded disc of chunk-radius `r` around
/// `(cx, cz)`, or `None` if no offset landed in the disc within a few tries.
fn random_column(rng: &mut MobRng, cx: i32, cz: i32, r: i32) -> Option<(i32, i32)> {
    for _ in 0..COLUMN_TRIES {
        let dx = rng.next_range(-r, r);
        let dz = rng.next_range(-r, r);
        if dx * dx + dz * dz > r * r {
            continue;
        }
        let lx = rng.next_range(0, CHUNK_SX as i32 - 1);
        let lz = rng.next_range(0, CHUNK_SZ as i32 - 1);
        let wx = (cx + dx) * CHUNK_SX as i32 + lx;
        let wz = (cz + dz) * CHUNK_SZ as i32 + lz;
        return Some((wx, wz));
    }
    None
}

/// Pick one species uniformly among those with population room, or `None` if none
/// has room. Reservoir sampling — uniform without allocating a candidate list.
/// A species whose spawn rule can't admit any site (empty biome/ground list — a
/// programmatic-spawn-only mob, e.g. a mod's own night spawner) is never a
/// candidate, so it can't eat the tick's single attempt. Species namespaced to
/// a mod the world disabled (`disabled` — per-world `settings.json`) are never
/// candidates either: no new disabled-mod content enters the world.
fn choose_kind(
    rng: &mut MobRng,
    room_for: &impl Fn(Mob) -> u32,
    disabled: &std::collections::BTreeSet<String>,
) -> Option<Mob> {
    let mut chosen = None;
    let mut seen = 0i32;
    for m in defs().iter().map(|d| d.mob) {
        if !def(m).spawn.is_spawnable() || room_for(m) < def(m).spawn_group.min_count() {
            continue;
        }
        if crate::registry::namespace(def(m).name).is_some_and(|ns| disabled.contains(ns)) {
            continue;
        }
        seen += 1;
        // Replace the held pick with probability 1/seen → every eligible kind ends up
        // equally likely.
        if rng.next_range(0, seen - 1) == 0 {
            chosen = Some(m);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_grass_spawn_world(extra: impl FnOnce(&mut crate::chunk::Chunk)) -> World {
        let mut world = World::new(0, 1);
        let mut chunk = crate::chunk::Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Grass);
                chunk.set_biome(x, z, Biome::Plains.id());
            }
        }
        extra(&mut chunk);
        world.insert_chunk_for_test(crate::chunk::ChunkPos::new(0, 0), chunk);
        world
    }

    fn valid_spawn_distance_player() -> Vec3 {
        Vec3::new(-60.0, 65.0, 8.0)
    }

    fn hostile_test_plan(player_pos: Vec3, chunk: ChunkPos) -> HostileSpawnPlan {
        HostileSpawnPlan {
            anchors: vec![HostileSpawnAnchor::new(player_pos)],
            spawnable_chunks: vec![chunk],
            attempt_chunks: vec![chunk],
            local_counts: vec![0],
            hostile_count: 0,
            hostile_cap: MobCategory::Hostile.cap(),
        }
    }

    #[test]
    fn cap_room_needs_room_in_both() {
        // Room in both -> fits, limited by the tighter cap.
        assert_eq!(cap_room(0, 8, 0, 25), 8);
        assert_eq!(cap_room(7, 8, 20, 25), 1);
        // Species full -> no, even with category room.
        assert_eq!(cap_room(8, 8, 0, 25), 0);
        // Category full -> no, even with species room.
        assert_eq!(cap_room(0, 8, 25, 25), 0);
        // Both full -> no.
        assert_eq!(cap_room(8, 8, 25, 25), 0);
    }

    #[test]
    fn too_close_is_a_sphere_around_the_player() {
        let player = Vec3::new(0.0, 0.0, 0.0);
        // Just inside 50 blocks → forbidden.
        assert!(too_close(player, Vec3::new(49.0, 0.0, 0.0), 50.0));
        // Just outside → allowed.
        assert!(!too_close(player, Vec3::new(51.0, 0.0, 0.0), 50.0));
        // Distance is 3-D: 50 up is also too close.
        assert!(too_close(player, Vec3::new(0.0, 49.0, 0.0), 50.0));
    }

    #[test]
    fn spawn_site_accepts_a_dry_valid_foothold() {
        let world = flat_grass_spawn_world(|_| {});

        assert!(
            spawn_site(&world, valid_spawn_distance_player(), Mob::Sheep, 8, 8).is_some(),
            "a dry grass foothold in a valid biome is spawnable"
        );
    }

    #[test]
    fn spawn_site_rejects_water_in_body_clearance() {
        let world = flat_grass_spawn_world(|chunk| {
            chunk.set_water(8, 65, 8, Block::Water, 0);
        });

        assert!(
            spawn_site(&world, valid_spawn_distance_player(), Mob::Sheep, 8, 8).is_none(),
            "the ground below the water is solid, but the mob body would spawn in water"
        );
    }

    #[test]
    fn passive_spawn_sites_share_the_128_block_outer_limit() {
        let world = flat_grass_spawn_world(|_| {});
        assert!(spawn_site(&world, Vec3::new(-200.0, 65.0, 8.0), Mob::Sheep, 8, 8).is_none());
    }

    #[test]
    fn hostile_global_cap_scales_by_unique_spawnable_chunks() {
        assert_eq!(scaled_mob_cap(70, 0), 0);
        assert_eq!(scaled_mob_cap(70, HOSTILE_SPAWN_CHUNKS_PER_PLAYER), 70);
        assert_eq!(scaled_mob_cap(70, HOSTILE_SPAWN_CHUNKS_PER_PLAYER * 2), 140);
        assert_eq!(
            scaled_mob_cap(70, HOSTILE_SPAWN_CHUNKS_PER_PLAYER / 2),
            34,
            "Floors the scaled cap after multiplying by unique chunks"
        );
    }

    #[test]
    fn hostile_spawnable_chunks_deduplicate_overlapping_players() {
        let a = HostileSpawnAnchor::new(Vec3::new(0.5, 64.0, 0.5));
        let b = HostileSpawnAnchor::new(Vec3::new(16.5, 64.0, 0.5));

        let solo = hostile_spawnable_chunks(&[a], |_| true);
        let together = hostile_spawnable_chunks(&[a, b], |_| true);

        assert_eq!(solo.len(), HOSTILE_SPAWN_CHUNKS_PER_PLAYER as usize);
        assert_eq!(
            together.len(),
            18 * 17,
            "adjacent players share most of their 17x17 chunk squares"
        );
        for i in 0..together.len() {
            assert!(
                !together[i + 1..].contains(&together[i]),
                "overlapping chunks count once toward the global cap"
            );
        }
    }

    #[test]
    fn hostile_plan_ignores_only_players_whose_local_census_is_still_loading() {
        let mut world = World::new(1, 1);
        let ready = Vec3::new(0.5, 64.0, 0.5);
        let loading = Vec3::new(160.5, 64.0, 0.5);
        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            world.insert_empty_column_for_test(ChunkPos::new(dx, dz));
        }

        let plan = hostile_spawn_plan(&world, &[ready, loading])
            .expect("the ready player's loaded neighborhood can spawn");
        assert_eq!(plan.anchors.len(), 1);
        assert_eq!(plan.anchors[0].chunk, ChunkPos::new(0, 0));
    }

    #[test]
    fn hostile_local_cap_allows_chunks_owned_by_any_player_with_room() {
        let cap = MobCategory::Hostile.cap();
        let a = HostileSpawnAnchor::new(Vec3::new(0.5, 64.0, 0.5));
        let b = HostileSpawnAnchor::new(Vec3::new(64.5, 64.0, 0.5));
        let anchors = [a, b];

        assert!(
            hostile_chunk_has_local_room(&anchors, &[cap, 0], ChunkPos::new(4, 0)),
            "an overlapping chunk may still spawn for the player whose local cap has room"
        );
        assert!(
            !hostile_chunk_has_local_room(&anchors, &[cap, 0], ChunkPos::new(-8, 0)),
            "a chunk only owned by a capped player is blocked"
        );
        assert!(
            hostile_chunk_has_local_room(&anchors, &[cap, 0], ChunkPos::new(12, 0)),
            "the uncapped player's non-overlap side still spawns"
        );
    }

    #[test]
    fn hostile_column_scan_prefers_high_loaded_spawn_site() {
        let mut world = World::new(1, 1);
        let chunk = ChunkPos::new(0, 0);
        world.insert_empty_column_for_test(chunk);
        for y in [47, 63] {
            assert!(world.set_block_world(8, y, 8, Block::Grass));
        }

        let plan = hostile_test_plan(Vec3::new(80.0, 64.0, 8.0), chunk);
        let candidates = hostile_column_candidates(&world, &plan, 8, 8);

        assert_eq!(
            candidates.first().map(|site| site.candidate.cell),
            Some([8, 64, 8])
        );
        assert!(
            candidates
                .iter()
                .any(|site| site.candidate.cell == [8, 48, 8]),
            "lower sites remain available when higher candidates are rejected"
        );
    }

    #[test]
    fn hostile_sampled_columns_come_from_eligible_chunks() {
        let world = World::new(11, 1);
        let chunks = vec![
            ChunkPos::new(-8, 0),
            ChunkPos::new(0, 0),
            ChunkPos::new(8, 8),
        ];
        let plan = HostileSpawnPlan {
            anchors: vec![HostileSpawnAnchor::new(Vec3::new(0.5, 64.0, 0.5))],
            spawnable_chunks: chunks.clone(),
            attempt_chunks: chunks.clone(),
            local_counts: vec![0],
            hostile_count: 0,
            hostile_cap: MobCategory::Hostile.cap(),
        };

        for attempt in 0..HOSTILE_SPAWN_ATTEMPTS {
            let (wx, wz) = hostile_candidate_column(&world, &plan, attempt).unwrap();
            let chunk = ChunkPos::new(wx >> 4, wz >> 4);
            assert!(chunks.contains(&chunk));
            assert!((0..CHUNK_SX as i32).contains(&(wx - chunk.cx * CHUNK_SX as i32)));
            assert!((0..CHUNK_SZ as i32).contains(&(wz - chunk.cz * CHUNK_SZ as i32)));
        }
    }
}
