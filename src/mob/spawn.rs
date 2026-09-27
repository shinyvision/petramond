use mod_api::HostileSpawnCandidate;
use rustc_hash::FxHashSet;

use crate::world::{ServerWorld, VERTICAL_LOAD_RADIUS};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::chunk::{
    ChunkPos, SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};

use super::path::{body_clear, is_foothold};
use super::{def, defs, Instance, Mob, MobCategory, MobRng};

const MIN_PLAYER_DIST: f32 = 50.0;
const MAX_PLAYER_DIST: f32 = 128.0;

pub const PASSIVE_SPAWN_INTERVAL_TICKS: u64 = 400;

const COLUMN_TRIES: u32 = 8;
const GROUP_RADIUS: i32 = 4;
const GROUP_MEMBER_TRIES: u32 = 24;
pub const HOSTILE_SPAWN_ATTEMPTS: u32 = 32;
const HOSTILE_SPAWN_CHUNK_RADIUS: i32 = 8;
const MOB_CENSUS_CHUNK_RADIUS: i32 = HOSTILE_SPAWN_CHUNK_RADIUS + 1;
const HOSTILE_SPAWN_CHUNKS_PER_PLAYER: u32 = 289;
const HOSTILE_MIN_SPAWN_DIST: f32 = 24.0;
const HOSTILE_SPAWN_SALT: u64 = 0xA11C_0DE5_5A55_0001;

pub(super) struct Spawn {
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
}

pub struct HostileSpawnSite {
    pub candidate: HostileSpawnCandidate,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
}

#[derive(Copy, Clone)]
struct HostileSpawnAnchor {
    pos: WorldPos,
    chunk: ChunkPos,
}

impl HostileSpawnAnchor {
    fn new(pos: WorldPos) -> Self {
        Self {
            pos,
            chunk: chunk_pos_at(pos),
        }
    }
}

pub struct HostileSpawnPlan {
    anchors: Vec<HostileSpawnAnchor>,
    spawnable_chunks: Vec<ChunkPos>,
    attempt_chunks: Vec<ChunkPos>,
    local_counts: Vec<u32>,
    hostile_count: u32,
    hostile_cap: u32,
}

pub(super) fn attempt(
    world: &ServerWorld,
    player_pos: WorldPos,
    rng: &mut MobRng,
    room_for: impl Fn(Mob) -> u32,
) -> Option<Vec<Spawn>> {
    if !mob_census_ready(world, player_pos) {
        return None;
    }
    let (cx, cz, render_dist) = world.data().loaded_area()?;
    let r = (render_dist - 1).clamp(0, HOSTILE_SPAWN_CHUNK_RADIUS);

    let kind = choose_kind(rng, &room_for, world.data().disabled_mods())?;
    let d = def(kind);
    let want = d.spawn_group.roll(rng).min(room_for(kind));

    let (wx, wz) = random_column(rng, cx, cz, r)?;
    let site = |world: &ServerWorld, kind: Mob, wx: i32, wz: i32| {
        spawn_site(world, player_pos, kind, wx, wz)
    };
    let first = spawn_with(world, kind, wx, wz, rng, &site)?;
    if !biome_chance_passes(world, kind, wx, wz, rng) {
        return None;
    }
    let mut spawns = Vec::with_capacity(want as usize);
    spawns.push(first);

    let origin = IVec3::new(wx, 0, wz);
    while spawns.len() < want as usize {
        let next = nearby_spawn(world, kind, origin, &spawns, rng, &site)?;
        spawns.push(next);
    }
    Some(spawns)
}

pub(super) fn spawn_with(
    world: &ServerWorld,
    kind: Mob,
    wx: i32,
    wz: i32,
    rng: &mut MobRng,
    site: &impl Fn(&ServerWorld, Mob, i32, i32) -> Option<WorldPos>,
) -> Option<Spawn> {
    let pos = site(world, kind, wx, wz)?;
    let yaw = rng.next_f32() * std::f32::consts::TAU;
    if !world.mob_spawn_pose_clear(kind, pos, yaw)
        || def(kind)
            .spawn
            .space
            .is_some_and(|space| !volume_site::body_in_space(world, kind, pos, yaw, space))
    {
        return None;
    }
    Some(Spawn { kind, pos, yaw })
}

fn spawn_site(
    world: &ServerWorld,
    player_pos: WorldPos,
    kind: Mob,
    wx: i32,
    wz: i32,
) -> Option<WorldPos> {
    let feet_pos = site_for(world, kind, wx, wz)?;
    if too_close(player_pos, feet_pos, MIN_PLAYER_DIST)
        || !too_close(player_pos, feet_pos, MAX_PLAYER_DIST)
    {
        return None;
    }
    Some(feet_pos)
}

pub(super) fn site_for(world: &ServerWorld, kind: Mob, wx: i32, wz: i32) -> Option<WorldPos> {
    if let Some(band) = def(kind).spawn.y {
        return volume_site::find(world, kind, &def(kind).spawn, wx, wz, band);
    }
    let ground_y = world.data().surface_collision_y(wx, wz)?;
    let feet = IVec3::new(wx, ground_y + 1, wz);
    let feet_pos = WorldPos::block_min(feet) + Vec3::new(0.5, 0.0, 0.5);

    if !body_fits_at(world, kind, feet) {
        return None;
    }

    let biome = Biome::from_id(world.data().column_biome(wx, wz)?);
    let ground = Block::from_id(world.data().chunk_block(wx, ground_y, wz));
    if !def(kind).spawn.admits(biome, ground) {
        return None;
    }

    Some(feet_pos)
}

mod volume_site;

pub(super) fn biome_chance_passes(
    world: &ServerWorld,
    kind: Mob,
    wx: i32,
    wz: i32,
    rng: &mut MobRng,
) -> bool {
    let Some(biome) = world.data().column_biome(wx, wz).map(Biome::from_id) else {
        return false;
    };
    chance_gate(def(kind).spawn.chance_in(biome), || rng.next_f32())
}

fn chance_gate(chance: f32, roll: impl FnOnce() -> f32) -> bool {
    chance >= 1.0 || roll() < chance
}

pub fn body_fits_at(world: &ServerWorld, kind: Mob, feet: IVec3) -> bool {
    let params = def(kind).path_params();
    let solid = |c: IVec3| world.data().blocks_movement_at(c.x, c.y, c.z);
    if !is_foothold(feet, params, &solid) {
        return false;
    }
    let fluid = |c: IVec3| world.data().fluid_cell_at(c.x, c.y, c.z);
    body_clear(feet, params, &fluid)
        && !super::nav::foothold_in_hazard(&world.cursor(), feet, params)
}

#[derive(Default)]
pub struct HostileSpawnCache {
    key: Option<(Vec<ChunkPos>, u64)>,
    census_ready: Vec<bool>,
    spawnable_chunks: Vec<ChunkPos>,
}

impl HostileSpawnCache {
    fn refresh(&mut self, world: &ServerWorld, player_positions: &[WorldPos]) {
        let anchor_chunks: Vec<ChunkPos> =
            player_positions.iter().map(|&p| chunk_pos_at(p)).collect();
        let key = (anchor_chunks, world.terrain_revision());
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.census_ready = player_positions
            .iter()
            .map(|&pos| mob_census_ready(world, pos))
            .collect();
        let chunks: Vec<ChunkPos> = key
            .0
            .iter()
            .copied()
            .zip(&self.census_ready)
            .filter_map(|(chunk, &ready)| ready.then_some(chunk))
            .collect();
        self.spawnable_chunks = hostile_spawnable_chunks(&chunks, |chunk| {
            world.data().chunk_loaded(chunk.cx, chunk.cz)
        });
        self.key = Some(key);
    }
}

pub fn hostile_spawn_plan(
    world: &ServerWorld,
    cache: &mut HostileSpawnCache,
    player_positions: &[WorldPos],
) -> Option<HostileSpawnPlan> {
    cache.refresh(world, player_positions);
    let anchors: Vec<HostileSpawnAnchor> = player_positions
        .iter()
        .copied()
        .zip(&cache.census_ready)
        .filter(|&(_pos, &ready)| ready)
        .map(|(pos, &_ready)| HostileSpawnAnchor::new(pos))
        .collect();
    if anchors.is_empty() {
        return None;
    }

    let spawnable_chunks = cache.spawnable_chunks.clone();
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

pub(super) fn mob_census_ready(world: &ServerWorld, player_pos: WorldPos) -> bool {
    let center = chunk_pos_at(player_pos);
    world.mob_census_loaded_around(center, MOB_CENSUS_CHUNK_RADIUS)
}

pub fn hostile_kind_has_room(world: &ServerWorld, plan: &HostileSpawnPlan, kind: Mob) -> bool {
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

pub fn hostile_attempt_sites(
    world: &ServerWorld,
    plan: &HostileSpawnPlan,
    attempt: u32,
) -> Vec<HostileSpawnSite> {
    let Some((wx, wz)) = hostile_candidate_column(world, plan, attempt) else {
        return Vec::new();
    };
    hostile_column_candidates(world, plan, wx, wz)
}

fn hostile_candidate_column(
    world: &ServerWorld,
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
    world: &ServerWorld,
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

fn hostile_attempt_seed(world: &ServerWorld, attempt: u32) -> u64 {
    (world.data().seed as u64)
        ^ world.current_tick().wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (attempt as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ HOSTILE_SPAWN_SALT
}

fn hostile_column_candidates(
    world: &ServerWorld,
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

fn hostile_anchor_scan_y_range(player_pos: WorldPos) -> Option<std::ops::RangeInclusive<i32>> {
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
    world: &ServerWorld,
    plan: &HostileSpawnPlan,
    wx: i32,
    y: i32,
    wz: i32,
) -> Option<HostileSpawnSite> {
    let pos = WorldPos::new(f64::from(wx) + 0.5, f64::from(y), f64::from(wz) + 0.5);
    let nearest = nearest_anchor_pos(&plan.anchors, pos)?;
    if too_close(nearest, pos, HOSTILE_MIN_SPAWN_DIST) || !too_close(nearest, pos, MAX_PLAYER_DIST)
    {
        return None;
    }
    if !body_cell_open(world, wx, y, wz)
        || !body_cell_open(world, wx, y + 1, wz)
        || !world.data().block_is_full_spawn_support(wx, y - 1, wz)
        || world
            .data()
            .physics_block(wx, y - 1, wz)
            .has_tag(petramond_world::block::BlockTag::NAV_HAZARD)
    {
        return None;
    }
    Some(HostileSpawnSite {
        candidate: HostileSpawnCandidate {
            pos: pos.to_array(),
            cell: [wx, y, wz],
            combined_light: world.data().combined_light6_at_world(wx, y, wz),
            sky_light: world.data().skylight6_at_world(wx, y, wz),
            block_light: world.data().blocklight6_at_world(wx, y, wz),
            nearest_player_dist: (nearest - pos).length(),
        },
        pos,
        yaw: yaw_away_from_player(nearest, pos),
    })
}

fn nearest_anchor_pos(anchors: &[HostileSpawnAnchor], pos: WorldPos) -> Option<WorldPos> {
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
    anchor_chunks: &[ChunkPos],
    mut loaded: impl FnMut(ChunkPos) -> bool,
) -> Vec<ChunkPos> {
    let mut chunks = Vec::new();
    let mut seen = FxHashSet::default();
    for anchor in anchor_chunks {
        for dz in -HOSTILE_SPAWN_CHUNK_RADIUS..=HOSTILE_SPAWN_CHUNK_RADIUS {
            for dx in -HOSTILE_SPAWN_CHUNK_RADIUS..=HOSTILE_SPAWN_CHUNK_RADIUS {
                let chunk = ChunkPos::new(anchor.cx + dx, anchor.cz + dz);
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

fn chunk_pos_at(pos: WorldPos) -> ChunkPos {
    ChunkPos::new(pos.x.floor() as i32 >> 4, pos.z.floor() as i32 >> 4)
}

fn dist2(a: WorldPos, b: WorldPos) -> f64 {
    a.distance_squared(b)
}

fn body_cell_open(world: &ServerWorld, wx: i32, y: i32, wz: i32) -> bool {
    let block = world.data().physics_block(wx, y, wz);
    world.data().placement_cell_open(IVec3::new(wx, y, wz))
        && block.fluid().is_none()
        && !block.has_tag(petramond_world::block::BlockTag::NAV_HAZARD)
}

fn yaw_away_from_player(player_pos: WorldPos, spawn_pos: WorldPos) -> f32 {
    let d = player_pos - spawn_pos;
    let (dx, dz) = (d.x, d.z);
    (-dx).atan2(-dz)
}

pub(super) fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

pub(super) fn nearby_spawn(
    world: &ServerWorld,
    kind: Mob,
    origin: IVec3,
    existing: &[Spawn],
    rng: &mut MobRng,
    site: &impl Fn(&ServerWorld, Mob, i32, i32) -> Option<WorldPos>,
) -> Option<Spawn> {
    let r2 = GROUP_RADIUS * GROUP_RADIUS;
    for _ in 0..GROUP_MEMBER_TRIES {
        let dx = rng.next_range(-GROUP_RADIUS, GROUP_RADIUS);
        let dz = rng.next_range(-GROUP_RADIUS, GROUP_RADIUS);
        if (dx == 0 && dz == 0) || dx * dx + dz * dz > r2 {
            continue;
        }
        let (wx, wz) = (origin.x + dx, origin.z + dz);
        let Some(spawn) = spawn_with(world, kind, wx, wz, rng, site) else {
            continue;
        };
        if too_near_existing(kind, spawn.pos, existing) {
            continue;
        }
        return Some(spawn);
    }
    None
}

fn too_near_existing(kind: Mob, pos: WorldPos, existing: &[Spawn]) -> bool {
    let min_gap = (def(kind).size.half_width * 2.0).max(0.75);
    let min_gap2 = min_gap * min_gap;
    existing.iter().any(|s| {
        let d = s.pos - pos;
        d.x * d.x + d.z * d.z < min_gap2
    })
}

pub(super) fn room_for(list: &[Instance], kind: Mob) -> u32 {
    let d = def(kind);
    let species = list.iter().filter(|m| m.kind == kind).count() as u32;
    let category = list
        .iter()
        .filter(|m| def(m.kind).category == d.category)
        .count() as u32;
    cap_room(species, d.cap, category, d.category.cap())
}

fn cap_room(species: u32, species_cap: u32, category: u32, category_cap: u32) -> u32 {
    species_cap
        .saturating_sub(species)
        .min(category_cap.saturating_sub(category))
}

fn too_close(player: WorldPos, feet: WorldPos, min_dist: f32) -> bool {
    player.distance_squared(feet) < f64::from(min_dist) * f64::from(min_dist)
}

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

pub(super) fn species_enabled(kind: Mob, disabled: &std::collections::BTreeSet<String>) -> bool {
    !petramond_world::registry::namespace(def(kind).name).is_some_and(|ns| disabled.contains(ns))
}

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
        if !species_enabled(m, disabled) {
            continue;
        }
        seen += 1;
        if rng.next_range(0, seen - 1) == 0 {
            chosen = Some(m);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_grass_spawn_world(
        extra: impl FnOnce(&mut petramond_world::chunk::Chunk),
    ) -> ServerWorld {
        let mut world = ServerWorld::new(0, 1);
        let mut chunk = petramond_world::chunk::Chunk::new(0, 0);
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                chunk.set_block(x, 64, z, Block::Grass);
                chunk.set_biome(x, z, Biome::PLAINS.id());
            }
        }
        extra(&mut chunk);
        world.insert_chunk_for_test(petramond_world::chunk::ChunkPos::new(0, 0), chunk);
        world
    }

    fn valid_spawn_distance_player() -> WorldPos {
        WorldPos::new(-60.0, 65.0, 8.0)
    }

    fn hostile_test_plan(player_pos: WorldPos, chunk: ChunkPos) -> HostileSpawnPlan {
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
        assert_eq!(cap_room(0, 8, 0, 25), 8);
        assert_eq!(cap_room(7, 8, 20, 25), 1);
        assert_eq!(cap_room(8, 8, 0, 25), 0);
        assert_eq!(cap_room(0, 8, 25, 25), 0);
        assert_eq!(cap_room(8, 8, 25, 25), 0);
    }

    #[test]
    fn the_chance_gate_is_certain_at_full_chance_and_rolls_below_it() {
        assert!(chance_gate(1.0, || unreachable!(
            "full chance draws no roll"
        )));
        assert!(chance_gate(0.2, || 0.19));
        assert!(!chance_gate(0.2, || 0.2));
    }

    #[test]
    fn too_close_is_a_sphere_around_the_player() {
        let player = WorldPos::new(0.0, 0.0, 0.0);
        assert!(too_close(player, WorldPos::new(49.0, 0.0, 0.0), 50.0));
        assert!(!too_close(player, WorldPos::new(51.0, 0.0, 0.0), 50.0));
        assert!(too_close(player, WorldPos::new(0.0, 49.0, 0.0), 50.0));
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
    fn spawn_site_rejects_fluid_in_body_clearance() {
        let world = flat_grass_spawn_world(|chunk| {
            chunk.set_fluid(8, 65, 8, Block::Water, 0);
        });

        assert!(
            spawn_site(&world, valid_spawn_distance_player(), Mob::Sheep, 8, 8).is_none(),
            "the ground below the water is solid, but the mob body would spawn in water"
        );
    }

    #[test]
    fn spawn_sites_refuse_hazardous_floors() {
        crate::entity::fluid_fixture::with_content("spawn-hazards", spawn_hazards_inner);
    }

    fn spawn_hazards_inner() {
        use crate::entity::fluid_fixture::{block, pool, CINDER, FLOOR_Y};
        let kind = crate::mob::by_key("bodyfluid:swim").unwrap();
        let feet = IVec3::new(8, FLOOR_Y, 8);
        let near = WorldPos::new(
            f64::from(8.5 + HOSTILE_MIN_SPAWN_DIST + 1.0),
            FLOOR_Y as f64,
            8.5,
        );
        let plan = hostile_test_plan(near, ChunkPos::new(0, 0));
        for (floor, safe) in [(Block::Stone, true), (block(CINDER), false)] {
            let mut world = pool(Block::Air, FLOOR_Y - 1);
            world.set_block_world(8, FLOOR_Y - 1, 8, floor);
            assert_eq!(body_fits_at(&world, kind, feet), safe, "{floor:?}");
            assert_eq!(
                hostile_candidate_at(&world, &plan, 8, FLOOR_Y, 8).is_some(),
                safe,
                "{floor:?}"
            );
        }
    }

    #[test]
    fn passive_spawn_sites_share_the_128_block_outer_limit() {
        let world = flat_grass_spawn_world(|_| {});
        assert!(spawn_site(&world, WorldPos::new(-200.0, 65.0, 8.0), Mob::Sheep, 8, 8).is_none());
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
        let a = chunk_pos_at(WorldPos::new(0.5, 64.0, 0.5));
        let b = chunk_pos_at(WorldPos::new(16.5, 64.0, 0.5));

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
        let mut world = ServerWorld::new(1, 1);
        let ready = WorldPos::new(0.5, 64.0, 0.5);
        let loading = WorldPos::new(160.5, 64.0, 0.5);
        for (dx, dz) in [(0, 0), (-1, 0), (1, 0), (0, -1), (0, 1)] {
            world.insert_empty_column_for_test(ChunkPos::new(dx, dz));
        }

        let plan = hostile_spawn_plan(&world, &mut HostileSpawnCache::default(), &[ready, loading])
            .expect("the ready player's loaded neighborhood can spawn");
        assert_eq!(plan.anchors.len(), 1);
        assert_eq!(plan.anchors[0].chunk, ChunkPos::new(0, 0));
    }

    #[test]
    fn hostile_local_cap_allows_chunks_owned_by_any_player_with_room() {
        let cap = MobCategory::Hostile.cap();
        let a = HostileSpawnAnchor::new(WorldPos::new(0.5, 64.0, 0.5));
        let b = HostileSpawnAnchor::new(WorldPos::new(64.5, 64.0, 0.5));
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
        let mut world = ServerWorld::new(1, 1);
        let chunk = ChunkPos::new(0, 0);
        world.insert_empty_column_for_test(chunk);
        for y in [47, 63] {
            assert!(world.set_block_world(8, y, 8, Block::Grass));
        }

        let plan = hostile_test_plan(WorldPos::new(80.0, 64.0, 8.0), chunk);
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
        let world = ServerWorld::new(11, 1);
        let chunks = vec![
            ChunkPos::new(-8, 0),
            ChunkPos::new(0, 0),
            ChunkPos::new(8, 8),
        ];
        let plan = HostileSpawnPlan {
            anchors: vec![HostileSpawnAnchor::new(WorldPos::new(0.5, 64.0, 0.5))],
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
