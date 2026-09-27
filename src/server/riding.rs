use crate::events::PostEvent;
use crate::mob::riding::{
    dismount_footing_safe, dismount_spot, player_body_free, player_body_known_free, seat_world_pos,
    Mount, MountTarget,
};
use crate::player::{Player, PlayerInputSnapshot};
use petramond_math::math::Vec3;

use super::game::ServerGame;

const SAVE_DISMOUNT_RADIUS: i32 = 8;
const SAVE_DISMOUNT_DY: [i32; 9] = [0, 1, -1, 2, -2, 3, -3, 4, -4];

impl ServerGame {
    pub fn publish_player_inputs(&mut self) {
        let inputs = self
            .sessions
            .iter()
            .map(|sess| {
                let gameplay = sess.input.intent_gameplay;
                let wish = sess.input.move_wishdir;
                let (sy, cy) = sess.player.yaw.sin_cos();
                let (forward, strafe) = if gameplay {
                    (wish.x * sy + wish.z * cy, -wish.x * cy + wish.z * sy)
                } else {
                    (0.0, 0.0)
                };
                PlayerInputSnapshot {
                    id: sess.id.0,
                    forward: forward.clamp(-1.0, 1.0),
                    strafe: strafe.clamp(-1.0, 1.0),
                    jump: sess.input.move_jump && gameplay,
                    sneak: sess.sneaking(),
                    yaw: sess.player.yaw,
                    pitch: sess.player.pitch,
                }
            })
            .collect();
        self.world.set_player_inputs(inputs);
        let operators: Vec<bool> = (0..self.sessions.len())
            .map(|s| self.is_operator(s))
            .collect();
        let mut roster: Vec<crate::player::PlayerRosterSnapshot> = self
            .sessions
            .iter_mut()
            .zip(operators)
            .map(|(sess, operator)| crate::player::PlayerRosterSnapshot {
                name: sess.name.clone(),
                operator,
                id: sess.id.0,
                pos: sess.player.pos.to_array(),
                vel: sess.player.vel.to_array(),
                yaw: sess.player.yaw,
                pitch: sess.player.pitch,
                health: sess.player.health(),
                on_ground: sess.player.on_ground,
                spectator: sess.player.is_spectator(),
                sneak: sess.sneaking(),
                use_held: sess.using(),
                use_gesture: sess.player.use_gesture.clone(),
                held: sess.selected_item(),
                held_count: sess.player.inventory.selected().map_or(0, |st| st.count),
                off_held: sess.player.inventory.off_hand().map(|st| st.item),
                swing: mod_api::HandSwing {
                    mining: sess.sim.mining.overlay().is_some(),
                    ..std::mem::take(&mut sess.replication.swing_events)
                },
                conditions: crate::exposure::condition_data(sess.player.conditions()),
                entombed: sess.player.entombed(),
            })
            .collect();
        roster.sort_by_key(|p| p.id);
        self.world.set_player_roster(roster);
    }

    pub fn tick_riding(&mut self) {
        for s in 0..self.sessions.len() {
            let id = self.sessions[s].id.0;
            let sneak = self.sessions[s].sneaking();
            let prev_sneak = std::mem::replace(&mut self.sessions[s].input.prev_sneak, sneak);
            if self.world.riding().mount_of(id).is_none() {
                continue;
            }
            let sess = &self.sessions[s];
            if (sneak && !prev_sneak)
                || sess.player.health() <= 0
                || sess.player.is_spectator()
                || sess.sim.sleep.is_some()
            {
                self.world.riding_mut().dismount(id);
            }
        }

        let stale: Vec<u8> = self
            .world
            .riding()
            .players()
            .filter_map(|p| {
                let m = self.world.riding().mount_of(p)?;
                let mount_live = match m.target {
                    MountTarget::Mob(mob_id) => self.world.mobs().live(mob_id).is_some(),
                    MountTarget::Anchor(_) => true,
                };
                let has_session = self.sessions.iter().any(|sess| sess.id.0 == p);
                (!mount_live || !has_session).then_some(p)
            })
            .collect();
        for p in stale {
            self.world.riding_mut().dismount(p);
        }

        self.publish_dismounted();

        for s in 0..self.sessions.len() {
            let id = self.sessions[s].id.0;
            let now = self.world.riding().mount_of(id);
            let before = std::mem::replace(&mut self.sessions[s].sim.mount, now);
            if before.is_some() && before != now {
                self.place_dismounted_player(s);
            }
            if let Some(m) = now {
                self.slave_rider_to_seat(s, m);
            }
        }
    }

    pub fn publish_dismounted(&mut self) {
        let detached: Vec<_> = self.world.riding_mut().drain_dismounted().collect();
        for (player, mount) in detached {
            self.mods.emit(PostEvent::PlayerDismounted {
                player: crate::player::PlayerId(player),
                mount,
            });
        }
    }

    pub fn detach_departing_session(&mut self, s: usize) {
        let id = self.sessions[s].id.0;
        self.world.riding_mut().dismount(id);
        if self.sessions[s].sim.mount.take().is_some() {
            self.place_dismounted_player(s);
        }
        self.publish_dismounted();
    }

    pub fn player_snapshot_for_save(
        &self,
        s: usize,
        obstacles: &[petramond_world::collision::DynBox],
    ) -> Option<Player> {
        let sess = &self.sessions[s];
        let mut snapshot = sess.player.clone();
        let mounted = sess.sim.mount.is_some() || self.world.riding().mount_of(sess.id.0).is_some();
        if !mounted {
            return Some(snapshot);
        }
        let feet = self.save_dismount_spot_for(&snapshot, obstacles)?;
        snapshot.teleport(feet);
        snapshot.vel = Vec3::ZERO;
        Some(snapshot)
    }

    fn slave_rider_to_seat(&mut self, s: usize, m: Mount) {
        let pos = match m.target {
            MountTarget::Mob(mob_id) => {
                let Some(mob) = self.world.mobs().get(mob_id) else {
                    return;
                };
                let d = crate::mob::def(mob.kind);
                let Some(&seat) = d.seats.get(m.seat as usize) else {
                    return;
                };
                seat_world_pos(mob.pos, mob.yaw, mob.tilt, seat)
            }
            MountTarget::Anchor(a) => a.pos,
        };
        let sess = &mut self.sessions[s];
        sess.player.pos = pos;
        sess.player.vel = Vec3::ZERO;
        sess.player.on_ground = true;
        sess.sim.fall.reset(pos.y);
        sess.sim.pending_fall = 0.0;
        sess.sim.pending_splash = 0.0;
    }

    fn place_dismounted_player(&mut self, s: usize) {
        let sess = &self.sessions[s];
        if sess.player.health() <= 0 || sess.player.is_spectator() {
            return;
        }
        let obstacles = self.world.mobs().solid_obstacles();
        if let Some(feet) = self.dismount_spot_for(&sess.player, &obstacles) {
            self.sessions[s].player.teleport(feet);
        }
    }

    fn dismount_spot_for(
        &self,
        player: &Player,
        obstacles: &[petramond_world::collision::DynBox],
    ) -> Option<petramond_math::world_pos::WorldPos> {
        dismount_spot(
            player.pos,
            player.yaw,
            |feet| player_body_free(self.world.data(), feet, obstacles),
            |feet| dismount_footing_safe(self.world.data(), feet),
        )
    }

    fn save_dismount_spot_for(
        &self,
        player: &Player,
        obstacles: &[petramond_world::collision::DynBox],
    ) -> Option<petramond_math::world_pos::WorldPos> {
        let known_free = |feet: petramond_math::world_pos::WorldPos| {
            player_body_known_free(&self.world, feet, obstacles)
        };
        let safe = |feet: petramond_math::world_pos::WorldPos| {
            let c = feet.block();
            self.world.physics_cell_final_at(c.x, c.y - 1, c.z)
                && dismount_footing_safe(self.world.data(), feet)
        };
        if let Some(feet) = dismount_spot(player.pos, player.yaw, known_free, safe) {
            return Some(feet);
        }
        if !player.pos.is_finite() {
            return None;
        }

        let origin = player.pos.block();
        for radius in 1..=SAVE_DISMOUNT_RADIUS {
            let mut unsafe_spot = None;
            for dy in SAVE_DISMOUNT_DY {
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx.abs().max(dz.abs()) != radius {
                            continue;
                        }
                        let feet = petramond_math::world_pos::WorldPos::new(
                            f64::from(origin.x + dx) + 0.5,
                            player.pos.y + f64::from(dy),
                            f64::from(origin.z + dz) + 0.5,
                        );
                        if !known_free(feet) {
                            continue;
                        }
                        if safe(feet) {
                            return Some(feet);
                        }
                        unsafe_spot.get_or_insert(feet);
                    }
                }
            }
            if unsafe_spot.is_some() {
                return unsafe_spot;
            }
        }
        None
    }
}
