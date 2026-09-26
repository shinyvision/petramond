//! Authoritative resolution of client-claimed mob targets.

use super::player::ConnectedPlayer;
use crate::player::{self, Player};
use crate::world::World;

/// Resolve a client-claimed stable mob id against `sess`'s current view ray
/// in `world`. The id is only a claim: it must name the nearest live body
/// before both terrain and reach, from the drift-bounded authoritative eye.
/// Dead players and spectators have no actionable mob target.
pub fn authoritative_mob_target(
    world: &World,
    sess: &ConnectedPlayer,
    requested: Option<u64>,
) -> Option<usize> {
    let requested = requested?;
    if sess.player.health() == 0 || sess.player.is_spectator() {
        return None;
    }

    let eye = super::movement::reach_eye(sess);
    let dir = sess.player.forward();
    let terrain_dist = Player::raycast_with_dist(eye, dir, world)
        .map(|(_, distance)| distance)
        .unwrap_or(player::REACH);
    let limit = terrain_dist.min(player::REACH);
    // Placement can mount a player earlier in this same tick, before the
    // riding pass refreshes the session mirror. The world registry is the
    // attachment authority at every stage boundary.
    let own_mount = world
        .riding()
        .mount_of(sess.id.0)
        .and_then(|mount| match mount.target {
            crate::mob::riding::MountTarget::Mob(id) => Some(id),
            crate::mob::riding::MountTarget::Anchor(_) => None,
        });
    let bodies = world
        .mobs()
        .instances()
        .iter()
        .enumerate()
        .filter(|(_, mob)| !mob.is_dead() && Some(mob.id()) != own_mount)
        .map(|(index, mob)| {
            (
                (mob.id(), index),
                mob.pos,
                mob.yaw,
                crate::mob::def(mob.kind).size,
            )
        });
    let ((id, index), _) = crate::mob::closest_body_ray_hit(eye, dir, limit, bodies)?;
    (id == requested).then_some(index)
}
