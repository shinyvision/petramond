use super::player::ConnectedPlayer;
use crate::player;
use crate::world::ServerWorld;
use petramond_world::world::raycast;

pub fn authoritative_mob_target(
    world: &ServerWorld,
    sess: &ConnectedPlayer,
    requested: Option<crate::mob::MobId>,
) -> Option<crate::mob::MobId> {
    let requested = requested?;
    if sess.player.health() == 0 || sess.player.is_spectator() {
        return None;
    }

    let eye = super::movement::reach_eye(sess);
    let dir = sess.player.forward();
    let terrain_dist = raycast::with_dist(eye, dir, world.data())
        .map(|(_, distance)| distance)
        .unwrap_or(player::REACH);
    let limit = terrain_dist.min(player::REACH);
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
        .filter(|mob| !mob.is_dead() && Some(mob.id()) != own_mount)
        .map(|mob| (mob.id(), mob.pos, mob.yaw, crate::mob::def(mob.kind).size));
    let (id, _) = crate::mob::closest_body_ray_hit(eye, dir, limit, bodies)?;
    (id == requested).then_some(id)
}
