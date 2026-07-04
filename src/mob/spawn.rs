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

use crate::biome::Biome;
use crate::block::Block;
use crate::chunk::{CHUNK_SX, CHUNK_SZ};
use crate::mathh::{IVec3, Vec3};
use crate::world::World;

use super::path::{body_clear, is_foothold, PathParams};
use super::{def, defs, Instance, Mob, MobRng};

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

/// A spawn the manager should perform: a species at a feet position, facing `yaw`.
pub(super) struct Spawn {
    pub kind: Mob,
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
        let Some(next) = nearby_spawn(world, player_pos, kind, origin, &spawns, rng) else {
            return None;
        };
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
    let params = PathParams::for_body(d.size.head_cells(), d.size.half_width);
    let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
    if !is_foothold(feet, params, &solid) {
        return None;
    }
    let water = |c: IVec3| world.water_cell_at(c.x, c.y, c.z);
    if !body_clear(feet, params, &water) {
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
}
