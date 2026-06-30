//! Natural mob spawning: one attempt per game tick.
//!
//! Each tick the manager calls [`attempt`], which picks a random column in the
//! loaded area, picks a species that still has population room, and tests the site
//! against the universal "definitely no" rules — too near the player, no footing, no
//! room for the body — and then the species' own [`SpawnRule`](super::SpawnRule)
//! (biome + the block it stands on). If everything passes it returns the [`Spawn`]
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

use super::path::is_foothold;
use super::{def, Instance, Mob, MobRng, ALL_MOBS};

/// Closest a natural spawn may appear to the player (blocks). Inside this, no spawn.
const MIN_PLAYER_DIST: f32 = 50.0;

/// How many times to resample a column offset to land one inside the loaded disc
/// before giving up for this tick (a near-degenerate disc could miss every time).
const COLUMN_TRIES: u32 = 8;

/// A spawn the manager should perform: a species at a feet position, facing `yaw`.
pub(super) struct Spawn {
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
}

/// Run one natural-spawn attempt. `has_room(kind)` reports whether a species is still
/// under its population caps (the manager supplies it from the live set). Returns the
/// spawn to perform, or `None` if this tick's site/species didn't qualify.
pub(super) fn attempt(
    world: &World,
    player_pos: Vec3,
    rng: &mut MobRng,
    has_room: impl Fn(Mob) -> bool,
) -> Option<Spawn> {
    let (cx, cz, render_dist) = world.loaded_area()?;
    // Inset by a chunk so the column (and the neighbours a footing/biome read may
    // touch) are loaded — unloaded reads would just fail the attempt anyway.
    let r = (render_dist - 1).max(0);
    let (wx, wz) = random_column(rng, cx, cz, r)?;

    // The surface to stand on, and the feet cell resting on top of it.
    let ground_y = world.surface_collision_y(wx, wz)?;
    let feet = IVec3::new(wx, ground_y + 1, wz);
    let feet_pos = Vec3::new(wx as f32 + 0.5, feet.y as f32, wz as f32 + 0.5);

    // Pick a species that still has room; the site is then judged for *that* species.
    let kind = choose_kind(rng, &has_room)?;
    let d = def(kind);

    // --- Universal "definitely no" checks. ---
    // Too close to the player.
    if too_close(player_pos, feet_pos, MIN_PLAYER_DIST) {
        return None;
    }
    // The ground must have collision AND the body must fit (clearance above the
    // feet) — exactly what a foothold test asserts.
    let solid = |c: IVec3| world.blocks_movement_at(c.x, c.y, c.z);
    if !is_foothold(feet, d.size.head_cells(), &solid) {
        return None;
    }

    // --- Species rule: biome + the block it would stand on. ---
    let biome = Biome::from_id(world.column_biome(wx, wz)?);
    let ground = Block::from_id(world.chunk_block(wx, ground_y, wz));
    if !d.spawn.admits(biome, ground) {
        return None;
    }

    let yaw = rng.next_f32() * std::f32::consts::TAU;
    Some(Spawn {
        kind,
        pos: feet_pos,
        yaw,
    })
}

/// Whether another individual of `kind` fits under both its caps, given the live set.
pub(super) fn has_room(list: &[Instance], kind: Mob) -> bool {
    let d = def(kind);
    let species = list.iter().filter(|m| m.kind == kind).count() as u32;
    let category = list
        .iter()
        .filter(|m| def(m.kind).category == d.category)
        .count() as u32;
    under_caps(species, d.cap, category, d.category.cap())
}

/// Pure cap test: an individual fits only if it's under both the per-species and the
/// per-category limit. Factored out so the rule is tested without pinning any
/// species' actual cap numbers.
fn under_caps(species: u32, species_cap: u32, category: u32, category_cap: u32) -> bool {
    species < species_cap && category < category_cap
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
fn choose_kind(rng: &mut MobRng, has_room: &impl Fn(Mob) -> bool) -> Option<Mob> {
    let mut chosen = None;
    let mut seen = 0i32;
    for &m in ALL_MOBS {
        if !has_room(m) {
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

    #[test]
    fn under_caps_needs_room_in_both() {
        // Room in both → fits.
        assert!(under_caps(0, 8, 0, 25));
        assert!(under_caps(7, 8, 24, 25));
        // Species full → no, even with category room.
        assert!(!under_caps(8, 8, 0, 25));
        // Category full → no, even with species room.
        assert!(!under_caps(0, 8, 25, 25));
        // Both full → no.
        assert!(!under_caps(8, 8, 25, 25));
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
}
