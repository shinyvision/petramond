//! Player riding: the ATTACHMENT registry.
//!
//! The engine owns the mechanism — which player is attached to which mob seat
//! (validated against `mobs.json` row `seats`) or pinned at which static pose
//! anchor — while attachment POLICY (who may sit where, who controls the
//! mount, where furniture seats exist) stays with mods through the
//! `MobMount`/`PlayerPoseSet`/`MobDismount` HostCalls. The registry lives on
//! `World` so those calls can reach it through `SimCtx`; the per-tick
//! consequences (slaving each rider's player to its seat, sneak-dismount,
//! pruning dead/vanished mounts) run in the server's riding pass
//! (`server::riding`), which reconciles sessions against this registry.
//!
//! Riding is transient session state: it is never persisted. A mob that dies,
//! despawns, or unloads sheds its riders on the next riding pass; a pose
//! anchor is released only by the engine valves (sneak, death, spectator,
//! leave) or the owning mod's detach call — furniture that breaks under a
//! sitter is the MOD's release to make.

use std::collections::BTreeMap;

use crate::player;
use crate::player::model::PLAYER_HIP_HEIGHT;
use petramond_math::math::{Tilt, Vec3};

const DISMOUNT_CLEARANCE: f32 = 0.45;
/// Cells above the seat a dismount candidate may stand, and so how far it may
/// drop to the ground it lands on.
const DISMOUNT_RISE: i32 = 1;

/// A static world-space actor pose: the anchor the body pins at, the body
/// yaw (player convention: yaw 0 faces `+Z`), and the named pose it holds
/// (vocabulary: `mod_api::pose`; unknown values render the rest pose).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PoseAnchor {
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub pose: u8,
}

/// What a player is attached to.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MountTarget {
    /// Live mob, addressed by its stable session id.
    Mob(u64),
    /// A static pose anchor (the `PlayerPoseSet` primitive — furniture).
    /// Target equality is the anchor value, so the registry's occupied-seat
    /// rule doubles as "no two players on one exact anchor".
    Anchor(PoseAnchor),
}

/// One player's attachment: the mount target and, for mob mounts, the seat
/// index into the species' declared `seats` list (`0` for pose anchors —
/// anchor identity is the anchor value itself).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Mount {
    pub target: MountTarget,
    pub seat: u8,
}

/// Rotate a mount-local seat offset (`x` right, `y` up, `z` facing) into world
/// space through the mount's whole body frame (yaw, then the tilt inside it —
/// the same frame the model renders with, so a seat stays in the cart when
/// the cart noses up a slope). A seat offset is declared as the rider's
/// ANCHOR (its feet), but the point that rests on a seat is the rider's HIP
/// (`PLAYER_HIP_HEIGHT` above the anchor): the hip is what the tilted frame
/// carries, and the anchor hangs straight below it — rotating the anchor
/// itself would swing the upright body's hips toward the nose. Both
/// authoritative slaving and client presentation use this exact transform.
pub fn seat_world_pos(
    mob_pos: petramond_math::world_pos::WorldPos,
    mob_yaw: f32,
    mob_tilt: Tilt,
    seat: [f32; 3],
) -> petramond_math::world_pos::WorldPos {
    // The seat's `z` runs along the facing, which is the body frame's -Z.
    let hip = Vec3::new(seat[0], seat[1] + PLAYER_HIP_HEIGHT, -seat[2]);
    mob_pos + mob_tilt.body_frame(mob_yaw).transform_vector3(hip) - Vec3::Y * PLAYER_HIP_HEIGHT
}

/// Pick the first collision-free dismount candidate beside `base` (right,
/// left, behind, ahead; base height then up to [`DISMOUNT_RISE`]), preferring safe
/// footing (see [`dismount_footing_safe`]). Pure over its probes so server
/// authority and client prediction agree.
pub fn dismount_spot(
    base: petramond_math::world_pos::WorldPos,
    yaw: f32,
    body_free: impl Fn(petramond_math::world_pos::WorldPos) -> bool,
    safe: impl Fn(petramond_math::world_pos::WorldPos) -> bool,
) -> Option<petramond_math::world_pos::WorldPos> {
    let (sy, cy) = yaw.sin_cos();
    let right = Vec3::new(-cy, 0.0, sy);
    let forward = Vec3::new(sy, 0.0, cy);
    let step = 2.0 * player::HALF_W + DISMOUNT_CLEARANCE;
    let mut fallback = None;
    for dir in [right, -right, -forward, forward] {
        for dy in 0..=DISMOUNT_RISE {
            let feet = base + dir * step + Vec3::new(0.0, dy as f32, 0.0);
            if !body_free(feet) {
                continue;
            }
            if safe(feet) {
                return Some(feet);
            }
            fallback.get_or_insert(feet);
        }
    }
    fallback
}

/// Whether a player body at `feet` keeps clear of every fluid and every
/// navigation hazard under its footprint, from its head down to the ground it
/// lands on: the first layer with collision within [`DISMOUNT_RISE`] below
/// the feet. No ground within that drop is not safe. Server placement and
/// client prediction share it so both pick the same spot.
pub fn dismount_footing_safe(
    world: &crate::world::World,
    feet: petramond_math::world_pos::WorldPos,
) -> bool {
    let (min, max) = player_body_aabb(feet);
    let footprint = |y: i32| {
        (min[0].floor() as i32..=max[0].floor() as i32).flat_map(move |x| {
            (min[2].floor() as i32..=max[2].floor() as i32).map(move |z| (x, y, z))
        })
    };
    let layer_safe = |y: i32| {
        footprint(y).all(|(x, y, z)| {
            let block = world.physics_block(x, y, z);
            block.fluid().is_none() && !block.has_tag(petramond_world::block::BlockTag::NAV_HAZARD)
        })
    };
    let ground = min[1].floor() as i32;
    if !(ground + 1..=max[1].floor() as i32).all(layer_safe) {
        return false;
    }
    for y in (ground - DISMOUNT_RISE..=ground).rev() {
        if !layer_safe(y) {
            return false;
        }
        if footprint(y).any(|(x, y, z)| !world.collision_boxes_at(x, y, z).is_empty()) {
            return true;
        }
    }
    false
}

/// Whether a standing player body at `feet` overlaps neither cell collision
/// nor a dynamic solid body. Fluid is not collision; callers rank dryness.
pub fn player_body_free(
    world: &crate::world::World,
    feet: petramond_math::world_pos::WorldPos,
    obstacles: &[petramond_world::collision::DynBox],
) -> bool {
    let (min, max) = player_body_aabb(feet);
    !petramond_world::collision::aabb_hits_cells(min, max, |x, y, z| {
        world.collision_boxes_at(x, y, z)
    }) && !petramond_world::collision::aabb_hits_dynamic(
        min,
        max,
        obstacles,
        petramond_world::collision::NOT_AN_ENTITY,
    )
}

/// Persistence-strength form of [`player_body_free`]: every terrain cell the
/// body reads must be stream-final as well as collision-free. This prevents an
/// absent mixed section or an in-flight saved overlay from masquerading as
/// open air while a mounted player's detached snapshot is chosen.
pub fn player_body_known_free(
    world: &crate::world::World,
    feet: petramond_math::world_pos::WorldPos,
    obstacles: &[petramond_world::collision::DynBox],
) -> bool {
    if !feet.is_finite() {
        return false;
    }
    let (min, max) = player_body_aabb(feet);
    for x in min[0].floor() as i32..=max[0].floor() as i32 {
        for y in min[1].floor() as i32..=max[1].floor() as i32 {
            for z in min[2].floor() as i32..=max[2].floor() as i32 {
                if !world.physics_cell_final_at(x, y, z) {
                    return false;
                }
            }
        }
    }
    player_body_free(world, feet, obstacles)
}

#[inline]
fn player_body_aabb(feet: petramond_math::world_pos::WorldPos) -> ([f64; 3], [f64; 3]) {
    let (hw, height) = (f64::from(player::HALF_W), f64::from(player::HEIGHT));
    (
        [feet.x - hw, feet.y + 0.01, feet.z - hw],
        [feet.x + hw, feet.y + height - 0.01, feet.z + hw],
    )
}

/// The riding registry: player id → mount. BTreeMap so every iteration
/// (occupancy checks, the riding pass, rider queries) is deterministic.
#[derive(Default)]
pub struct Riding {
    mounts: BTreeMap<u8, Mount>,
    /// Completed detach transitions waiting for the server riding pass to
    /// publish them. Recording the transition here means a mount followed by
    /// a dismount before the session mirror runs still produces one event.
    dismounted: Vec<(u8, Mount)>,
}

impl Riding {
    /// The mount `player` currently occupies, if any.
    #[inline]
    pub fn mount_of(&self, player: u8) -> Option<Mount> {
        self.mounts.get(&player).copied()
    }

    /// Every rider of `target` as `(seat, player)`, in player-id order.
    pub fn riders_of(&self, target: MountTarget) -> Vec<(u8, u8)> {
        self.mounts
            .iter()
            .filter(|(_, m)| m.target == target)
            .map(|(&player, m)| (m.seat, player))
            .collect()
    }

    /// Whether `seat` of `target` is taken.
    pub fn seat_taken(&self, target: MountTarget, seat: u8) -> bool {
        self.mounts
            .values()
            .any(|m| m.target == target && m.seat == seat)
    }

    /// Attach `player` to `seat` of `target`. Refused when the player is
    /// already mounted or the seat is taken — seat-count/liveness validation
    /// against the mount itself is the caller's job.
    pub fn mount(&mut self, player: u8, target: MountTarget, seat: u8) -> bool {
        if self.mounts.contains_key(&player) || self.seat_taken(target, seat) {
            return false;
        }
        self.mounts.insert(player, Mount { target, seat });
        true
    }

    /// Detach `player`, recording the completed transition for the server to
    /// announce. Returns the mount that was left, or `None` when the player
    /// was already detached.
    pub fn dismount(&mut self, player: u8) -> Option<Mount> {
        let mount = self.mounts.remove(&player)?;
        self.dismounted.push((player, mount));
        Some(mount)
    }

    /// Drain completed detach transitions in the deterministic order they
    /// occurred. A transition is recorded exactly once by `dismount`.
    pub fn drain_dismounted(&mut self) -> impl Iterator<Item = (u8, Mount)> + '_ {
        self.dismounted.drain(..)
    }

    /// The mounted player ids, in order (the riding pass iterates this against
    /// the live sessions).
    pub fn players(&self) -> impl Iterator<Item = u8> + '_ {
        self.mounts.keys().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::fluid_fixture::{self, block, pool, BRINE, CINDER, FLOOR_Y, SYRUP};
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;

    #[test]
    fn dismounts_prefer_footing_clear_of_fluids_and_hazards() {
        let root = fluid_fixture::stage("dismount-footing");
        crate::modding::tests::run_child_test(&root, "mob::riding::tests::dismount_footing_inner");
    }

    #[test]
    #[ignore = "child of dismounts_prefer_footing_clear_of_fluids_and_hazards"]
    fn dismount_footing_inner() {
        let feet = WorldPos::new(8.5, FLOOR_Y as f64, 8.5);
        let floor = FLOOR_Y - 1;
        let cases: [(&str, i32, i32, bool); 5] = [
            ("petramond:stone", 8, floor, true),
            (BRINE, 8, FLOOR_Y, false),
            (SYRUP, 8, floor, false),
            (CINDER, 8, floor, false),
            (CINDER, 9, floor, true),
        ];
        for (name, x, y, safe) in cases {
            let mut world = pool(Block::Air, floor);
            world.set_block_world(x, y, 8, block(name));
            assert_eq!(
                dismount_footing_safe(&world, feet),
                safe,
                "{name} at ({x}, {y})"
            );
        }

        let mut world = pool(Block::Air, floor);
        for z in 0..16 {
            world.set_block_world(7, floor, z, block(CINDER));
        }
        let spot = dismount_spot(
            feet,
            0.0,
            |at| player_body_free(&world, at, &[]),
            |at| dismount_footing_safe(&world, at),
        )
        .expect("a free spot");
        assert!(
            spot.x > feet.x,
            "the hazardous right side loses to the safe left: {spot:?}"
        );
    }

    #[test]
    fn one_seat_one_rider_one_mount_per_player() {
        let mut r = Riding::default();
        let boat = MountTarget::Mob(77);
        let chair = MountTarget::Anchor(PoseAnchor {
            pos: WorldPos::new(1.5, 2.0, 3.5),
            yaw: 0.0,
            pose: 1,
        });
        assert!(r.mount(1, boat, 0));
        assert!(!r.mount(2, boat, 0), "occupied seat refuses a second rider");
        assert!(r.mount(2, boat, 1));
        assert!(!r.mount(1, MountTarget::Mob(88), 0), "already mounted");
        assert_eq!(r.riders_of(boat), vec![(0, 1), (1, 2)]);
        assert_eq!(
            r.dismount(1),
            Some(Mount {
                target: boat,
                seat: 0
            })
        );
        assert_eq!(r.dismount(1), None);
        assert!(r.mount(1, chair, 0));
        assert_eq!(
            r.mount_of(1),
            Some(Mount {
                target: chair,
                seat: 0
            })
        );
        assert_eq!(
            r.drain_dismounted().collect::<Vec<_>>(),
            vec![(
                1,
                Mount {
                    target: boat,
                    seat: 0
                }
            )]
        );
    }

    #[test]
    fn seat_offsets_rotate_with_the_mob_facing() {
        let pos = WorldPos::new(10.0, 5.0, 10.0);
        let bow = seat_world_pos(pos, 0.0, Tilt::LEVEL, [0.0, 0.25, 1.0]);
        assert!(
            (bow - WorldPos::new(10.0, 5.25, 9.0)).length() < 1e-5,
            "{bow:?}"
        );
        let bow = seat_world_pos(pos, std::f32::consts::PI, Tilt::LEVEL, [0.0, 0.25, 1.0]);
        assert!(
            (bow - WorldPos::new(10.0, 5.25, 11.0)).length() < 1e-4,
            "{bow:?}"
        );
        let side = seat_world_pos(
            pos,
            std::f32::consts::FRAC_PI_2,
            Tilt::LEVEL,
            [1.0, 0.0, 0.0],
        );
        assert!(
            (side - WorldPos::new(10.0, 5.0, 9.0)).length() < 1e-4,
            "{side:?}"
        );
    }

    /// A tilted body carries its seats with it AT THE HIPS: nose up by a
    /// right angle, a rider whose hips sat ahead of the origin now has them
    /// straight above it, hips that sat above the origin have swung back
    /// behind it, and the anchor hangs one hip height below either. Rolled
    /// right side up by a right angle, hips out to the right are straight
    /// above the origin.
    #[test]
    fn seat_offsets_follow_the_body_tilt_at_the_hips() {
        let pos = WorldPos::new(10.0, 5.0, 10.0);
        let nose_up = Tilt::new(std::f32::consts::FRAC_PI_2, 0.0);
        let hips = |tilt: Tilt, seat: [f32; 3]| {
            seat_world_pos(pos, 0.0, tilt, seat) + Vec3::Y * PLAYER_HIP_HEIGHT
        };
        let ahead = hips(nose_up, [0.0, -PLAYER_HIP_HEIGHT, 1.0]);
        assert!(
            (ahead - WorldPos::new(10.0, 6.0, 10.0)).length() < 1e-4,
            "{ahead:?}"
        );
        let above = hips(nose_up, [0.0, 1.0 - PLAYER_HIP_HEIGHT, 0.0]);
        assert!(
            (above - WorldPos::new(10.0, 5.0, 11.0)).length() < 1e-4,
            "{above:?}"
        );
        let side = hips(nose_up, [1.0, -PLAYER_HIP_HEIGHT, 0.0]);
        assert!(
            (side - WorldPos::new(11.0, 5.0, 10.0)).length() < 1e-4,
            "{side:?}"
        );
        let right_up = Tilt::new(0.0, std::f32::consts::FRAC_PI_2);
        let rolled = hips(right_up, [1.0, -PLAYER_HIP_HEIGHT, 0.0]);
        assert!(
            (rolled - WorldPos::new(10.0, 6.0, 10.0)).length() < 1e-4,
            "{rolled:?}"
        );
        // Level, the hip correction cancels: the seat IS the anchor.
        let level = seat_world_pos(pos, 0.0, Tilt::LEVEL, [0.0, -0.35, 0.0]);
        assert!(
            (level - WorldPos::new(10.0, 4.65, 10.0)).length() < 1e-5,
            "{level:?}"
        );
    }

    #[test]
    fn two_players_cannot_share_one_exact_anchor() {
        let mut r = Riding::default();
        let anchor = |pose| {
            MountTarget::Anchor(PoseAnchor {
                pos: WorldPos::new(4.5, 64.0, -2.6),
                yaw: 1.0,
                pose,
            })
        };
        assert!(r.mount(1, anchor(1), 0));
        assert!(
            !r.mount(2, anchor(1), 0),
            "an occupied anchor refuses a second body"
        );
        let nearby = MountTarget::Anchor(PoseAnchor {
            pos: WorldPos::new(4.5, 64.0, -2.4),
            yaw: 1.0,
            pose: 1,
        });
        assert!(
            r.mount(2, nearby, 0),
            "a distinct anchor is a distinct seat"
        );
    }
}
