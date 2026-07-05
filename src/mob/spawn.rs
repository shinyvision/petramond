//! Natural mob spawning: one attempt per game tick.
//!
//! Each tick the manager calls [`attempt`], which picks a random column in the
//! loaded area, picks a species that still has population room, and tests the site
//! against the universal "definitely no" rules -- too near the player, no footing,
//! no room for the body -- and then the species' own [`SpawnRule`](super::SpawnRule)
//! (biome + the block it stands on). If everything passes it returns the [`Spawn`]s
//! for the manager to apply; otherwise the tick simply spawns nothing.
//!
//! The population caps live elsewhere as data: per-species on the [`MobDef`] row, and
//! per-category on [`MobCategory`]. This module only enforces them, and only the
//! site/arithmetic logic that's worth pinning is factored out pure and tested below.
//!
//! [`MobDef`]: super::MobDef
//! [`MobCategory`]: super::MobCategory

use mod_api::HostileSpawnCandidate;

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{SectionPos, CHUNK_SX, CHUNK_SZ, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::mathh::{IVec3, Vec3};
use crate::world::{World, VERTICAL_LOAD_RADIUS};

use super::path::{body_clear, is_foothold, PathParams};
use super::{def, defs, Instance, Mob, MobCategory, MobRng};

/// Closest a natural spawn may appear to the player (blocks). Inside this, no spawn.
const MIN_PLAYER_DIST: f32 = 50.0;

/// How many times to resample a column offset to land one inside the loaded disc
/// before giving up for this tick (a near-degenerate disc could miss every time).
const COLUMN_TRIES: u32 = 8;
/// Radius around the first valid site where herd/pack members may be placed.
const GROUP_RADIUS: i32 = 4;
/// Attempts per extra group member to find another nearby site satisfying the same
/// species spawn rule.
const GROUP_MEMBER_TRIES: u32 = 24;
pub(crate) const HOSTILE_SPAWN_ATTEMPTS: u32 = 32;
const HOSTILE_MIN_SPAWN_DIST: f32 = 25.0;
const HOSTILE_MAX_SPAWN_DIST: f32 = 128.0;
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

/// Run one natural-spawn attempt. `room_for(kind)` reports how many more individuals
/// fit under a species' population caps (the manager supplies it from the live set).
/// Returns the spawns to perform, or `None` if this tick's site/species didn't qualify.
pub(super) fn attempt(
    world: &World,
    player_pos: Vec3,
    rng: &mut MobRng,
    room_for: impl Fn(Mob) -> u32,
) -> Option<Vec<Spawn>> {
    let (cx, cz, render_dist) = world.loaded_area()?;
    // Inset by a chunk so the column (and the neighbours a footing/biome read may
    // touch) are loaded — unloaded reads would just fail the attempt anyway.
    let r = (render_dist - 1).max(0);

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
    if too_close(player_pos, feet_pos, MIN_PLAYER_DIST) {
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

pub(crate) fn hostile_cap_full(world: &World) -> bool {
    (world
        .mobs()
        .instances()
        .iter()
        .filter(|m| def(m.kind).category == MobCategory::Hostile)
        .count() as u32)
        >= MobCategory::Hostile.cap()
}

pub(crate) fn hostile_attempt_sites(
    world: &World,
    player_pos: Vec3,
    attempt: u32,
) -> Vec<HostileSpawnSite> {
    let (wx, wz) = hostile_candidate_column(world, player_pos, attempt);
    hostile_column_candidates(world, player_pos, wx, wz)
}

fn hostile_candidate_column(world: &World, player_pos: Vec3, attempt: u32) -> (i32, i32) {
    let seed = (world.seed as u64)
        ^ world.current_tick().wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (attempt as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
        ^ HOSTILE_SPAWN_SALT;
    let angle = unit_f32(splitmix(seed ^ 0xA5A5_A5A5_A5A5_A5A5)) * std::f32::consts::TAU;
    let dist = HOSTILE_MIN_SPAWN_DIST
        + unit_f32(splitmix(seed ^ 0x5A5A_5A5A_5A5A_5A5A))
            * (HOSTILE_MAX_SPAWN_DIST - HOSTILE_MIN_SPAWN_DIST);
    let (sin, cos) = angle.sin_cos();
    (
        (player_pos.x + dist * cos).floor() as i32,
        (player_pos.z + dist * sin).floor() as i32,
    )
}

fn hostile_column_candidates(
    world: &World,
    player_pos: Vec3,
    wx: i32,
    wz: i32,
) -> Vec<HostileSpawnSite> {
    let Some(range) = hostile_scan_y_range(player_pos) else {
        return Vec::new();
    };
    range
        .rev()
        .filter_map(|y| hostile_candidate_at(world, player_pos, wx, y, wz))
        .collect()
}

fn hostile_scan_y_range(player_pos: Vec3) -> Option<std::ops::RangeInclusive<i32>> {
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
    player_pos: Vec3,
    wx: i32,
    y: i32,
    wz: i32,
) -> Option<HostileSpawnSite> {
    if !body_cell_open(world, wx, y, wz)
        || !body_cell_open(world, wx, y + 1, wz)
        || !world.block_is_full_spawn_support(wx, y - 1, wz)
    {
        return None;
    }
    let pos = Vec3::new(wx as f32 + 0.5, y as f32, wz as f32 + 0.5);
    Some(HostileSpawnSite {
        candidate: HostileSpawnCandidate {
            pos: [pos.x, pos.y, pos.z],
            cell: [wx, y, wz],
            combined_light: world.combined_light6_at_world(wx, y, wz),
            sky_light: world.skylight6_at_world(wx, y, wz),
            block_light: world.blocklight6_at_world(wx, y, wz),
        },
        pos,
        yaw: yaw_away_from_player(player_pos, pos),
    })
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

fn unit_f32(bits: u64) -> f32 {
    ((bits >> 40) as u32) as f32 * (1.0 / (1u32 << 24) as f32)
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

    fn far_player() -> Vec3 {
        Vec3::new(1000.0, 1000.0, 1000.0)
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
            spawn_site(&world, far_player(), Mob::Sheep, 8, 8).is_some(),
            "a dry grass foothold in a valid biome is spawnable"
        );
    }

    #[test]
    fn spawn_site_rejects_water_in_body_clearance() {
        let world = flat_grass_spawn_world(|chunk| {
            chunk.set_water(8, 65, 8, Block::Water, 0);
        });

        assert!(
            spawn_site(&world, far_player(), Mob::Sheep, 8, 8).is_none(),
            "the ground below the water is solid, but the mob body would spawn in water"
        );
    }

    #[test]
    fn hostile_column_scan_prefers_high_loaded_spawn_site() {
        let mut world = World::new(1, 1);
        world.insert_empty_column_for_test(crate::chunk::ChunkPos::new(0, 0));
        for y in [47, 63] {
            assert!(world.set_block_world(8, y, 8, Block::Grass));
        }

        let candidates = hostile_column_candidates(&world, Vec3::new(8.0, 64.0, 8.0), 8, 8);

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
    fn hostile_sampled_columns_stay_in_the_spawn_ring() {
        let world = World::new(11, 1);
        let player_pos = Vec3::new(8.0, 64.0, 8.0);

        for attempt in 0..HOSTILE_SPAWN_ATTEMPTS {
            let (wx, wz) = hostile_candidate_column(&world, player_pos, attempt);
            let dx = wx as f32 + 0.5 - player_pos.x;
            let dz = wz as f32 + 0.5 - player_pos.z;
            let dist = (dx * dx + dz * dz).sqrt();
            assert!(dist >= HOSTILE_MIN_SPAWN_DIST - 1.0);
            assert!(dist <= HOSTILE_MAX_SPAWN_DIST + 1.0);
        }
    }
}
