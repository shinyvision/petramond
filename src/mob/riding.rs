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

use crate::mathh::Vec3;
use crate::player;

const DISMOUNT_CLEARANCE: f32 = 0.45;

/// A static world-space actor pose: the anchor the body pins at, the body
/// yaw (player convention: yaw 0 faces `+Z`), and the named pose it holds
/// (vocabulary: `mod_api::pose`; unknown values render the rest pose).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PoseAnchor {
    pub pos: Vec3,
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
/// space. Both authoritative slaving and client presentation use this exact
/// transform for mob seats.
pub(crate) fn seat_world_pos(mob_pos: Vec3, mob_yaw: f32, seat: [f32; 3]) -> Vec3 {
    let (sy, cy) = mob_yaw.sin_cos();
    let facing = Vec3::new(-sy, 0.0, -cy);
    let right = Vec3::new(cy, 0.0, -sy);
    mob_pos + right * seat[0] + Vec3::new(0.0, seat[1], 0.0) + facing * seat[2]
}

/// Pick the first collision-free dismount candidate beside `base` (right,
/// left, behind, ahead; base height then one block up), preferring dry feet.
/// Pure over its probes so server authority and client prediction agree.
pub(crate) fn dismount_spot(
    base: Vec3,
    yaw: f32,
    body_free: impl Fn(Vec3) -> bool,
    dry: impl Fn(Vec3) -> bool,
) -> Option<Vec3> {
    let (sy, cy) = yaw.sin_cos();
    let right = Vec3::new(-cy, 0.0, sy);
    let forward = Vec3::new(sy, 0.0, cy);
    let step = 2.0 * player::HALF_W + DISMOUNT_CLEARANCE;
    let mut fallback = None;
    for dir in [right, -right, -forward, forward] {
        for dy in [0.0, 1.0] {
            let feet = base + dir * step + Vec3::new(0.0, dy, 0.0);
            if !body_free(feet) {
                continue;
            }
            if dry(feet) {
                return Some(feet);
            }
            fallback.get_or_insert(feet);
        }
    }
    fallback
}

/// Whether a standing player body at `feet` overlaps neither cell collision
/// nor a dynamic solid body. Water is not collision; callers rank dryness.
pub(crate) fn player_body_free(
    world: &crate::world::World,
    feet: Vec3,
    obstacles: &[crate::collision::DynBox],
) -> bool {
    let (min, max) = player_body_aabb(feet);
    !crate::collision::aabb_hits_cells(min, max, |x, y, z| world.collision_boxes_at(x, y, z))
        && !crate::collision::aabb_hits_dynamic(
            min,
            max,
            obstacles,
            crate::collision::NOT_AN_ENTITY,
        )
}

/// Persistence-strength form of [`player_body_free`]: every terrain cell the
/// body reads must be stream-final as well as collision-free. This prevents an
/// absent mixed section or an in-flight saved overlay from masquerading as
/// open air while a mounted player's detached snapshot is chosen.
pub(crate) fn player_body_known_free(
    world: &crate::world::World,
    feet: Vec3,
    obstacles: &[crate::collision::DynBox],
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
fn player_body_aabb(feet: Vec3) -> ([f32; 3], [f32; 3]) {
    (
        [
            feet.x - player::HALF_W,
            feet.y + 0.01,
            feet.z - player::HALF_W,
        ],
        [
            feet.x + player::HALF_W,
            feet.y + player::HEIGHT - 0.01,
            feet.z + player::HALF_W,
        ],
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
    /// occurred. A transition is recorded exactly once by [`dismount`].
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

    #[test]
    fn one_seat_one_rider_one_mount_per_player() {
        let mut r = Riding::default();
        let boat = MountTarget::Mob(77);
        let chair = MountTarget::Anchor(PoseAnchor {
            pos: Vec3::new(1.5, 2.0, 3.5),
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
        let pos = Vec3::new(10.0, 5.0, 10.0);
        let bow = seat_world_pos(pos, 0.0, [0.0, 0.25, 1.0]);
        assert!(
            (bow - Vec3::new(10.0, 5.25, 9.0)).length() < 1e-5,
            "{bow:?}"
        );
        let bow = seat_world_pos(pos, std::f32::consts::PI, [0.0, 0.25, 1.0]);
        assert!(
            (bow - Vec3::new(10.0, 5.25, 11.0)).length() < 1e-4,
            "{bow:?}"
        );
        let side = seat_world_pos(pos, std::f32::consts::FRAC_PI_2, [1.0, 0.0, 0.0]);
        assert!(
            (side - Vec3::new(10.0, 5.0, 9.0)).length() < 1e-4,
            "{side:?}"
        );
    }

    #[test]
    fn two_players_cannot_share_one_exact_anchor() {
        let mut r = Riding::default();
        let anchor = |pose| {
            MountTarget::Anchor(PoseAnchor {
                pos: Vec3::new(4.5, 64.0, -2.6),
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
            pos: Vec3::new(4.5, 64.0, -2.4),
            yaw: 1.0,
            pose: 1,
        });
        assert!(r.mount(2, nearby, 0), "a distinct anchor is a distinct seat");
    }
}
