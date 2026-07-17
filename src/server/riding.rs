//! The per-tick riding pass — the server half of `mob::riding`.
//!
//! Runs inside the Mobs stage, right after the mobs moved: it applies the
//! engine-owned dismount valves (sneak gesture, dead/vanished mount, dead,
//! sleeping, spectator, or departed rider), publishes completed detach transitions,
//! reconciles each session's `mount` mirror, and slaves every rider's player
//! to its seat.
//! Mount/dismount POLICY stays with mods (`MobMount`/`MobDismount` HostCalls
//! write the same registry); this pass owns only the physical consequences.
//!
//! A mounted session's own movement integration is skipped in
//! `tick_movement`; its movement INTENT still arrives every tick and is
//! published on the world (`publish_player_inputs`) so a driving mod can read
//! it back through the `PlayerInput` HostCall.

use crate::events::PostEvent;
use crate::mathh::Vec3;
use crate::mob::riding::{
    dismount_spot, player_body_free, player_body_known_free, seat_world_pos, Mount,
};
use crate::player::{Player, PlayerInputSnapshot};

use super::game::ServerGame;

/// Autosave may search farther than an interactive dismount, but remains
/// strictly bounded per mounted session. A miss defers that player's write.
const SAVE_DISMOUNT_RADIUS: i32 = 8;
const SAVE_DISMOUNT_DY: [i32; 9] = [0, 1, -1, 2, -2, 3, -3, 4, -4];

impl ServerGame {
    /// Publish every session's movement intent AND state snapshot for this
    /// tick on the world — the read models behind the `PlayerInput` and
    /// `Players` HostCalls. Intents are decomposed into the player's own yaw
    /// frame with the client's exact wish basis (forward = `(sin yaw, cos
    /// yaw)`, right = `(-cos yaw, sin yaw)`), gameplay-gated like the intents
    /// `tick_movement` integrates.
    pub(crate) fn publish_player_inputs(&mut self) {
        let inputs = self
            .sessions
            .iter()
            .map(|sess| {
                let gameplay = sess.intent_gameplay;
                let wish = sess.move_wishdir;
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
                    jump: sess.move_jump && gameplay,
                    sneak: sess.sneaking(),
                    yaw: sess.player.yaw,
                    pitch: sess.player.pitch,
                }
            })
            .collect();
        self.world.set_player_inputs(inputs);
        // Session storage order changes with swap_remove joins/leaves; the
        // roster SORTS by id so the ABI's "session-id order" stays true.
        let mut roster: Vec<crate::player::PlayerRosterSnapshot> = self
            .sessions
            .iter()
            .map(|sess| crate::player::PlayerRosterSnapshot {
                id: sess.id.0,
                pos: sess.player.pos.to_array(),
                vel: sess.player.vel.to_array(),
                yaw: sess.player.yaw,
                pitch: sess.player.pitch,
                health: sess.player.health(),
                on_ground: sess.player.on_ground,
                spectator: sess.player.is_spectator(),
            })
            .collect();
        roster.sort_by_key(|p| p.id);
        self.world.set_player_roster(roster);
    }

    /// The riding pass (see module docs). Order matters: valves first, then
    /// publish completed detaches, reconcile mirrors (physical consequences),
    /// then slave riders to the moved mobs.
    pub(crate) fn tick_riding(&mut self) {
        // Engine dismount valves, session side: the sneak RISING EDGE while
        // mounted is the get-off gesture; death and spectator shed the seat.
        for s in 0..self.sessions.len() {
            let id = self.sessions[s].id.0;
            let sneak = self.sessions[s].sneaking();
            let prev_sneak = std::mem::replace(&mut self.sessions[s].prev_sneak, sneak);
            if self.world.riding().mount_of(id).is_none() {
                continue;
            }
            let sess = &self.sessions[s];
            if (sneak && !prev_sneak)
                || sess.player.health() <= 0
                || sess.player.is_spectator()
                || sess.sleep.is_some()
            {
                self.world.riding_mut().dismount(id);
            }
        }

        // Registry-side valves: a mount whose mob is gone or dead, or whose
        // player has no session anymore (left), detaches.
        let stale: Vec<u8> = self
            .world
            .riding()
            .players()
            .filter_map(|p| {
                let m = self.world.riding().mount_of(p)?;
                let mob_live = self
                    .world
                    .mobs()
                    .index_of_id(m.mob_id)
                    .is_some_and(|idx| !self.world.mobs().instances()[idx].is_dead());
                let has_session = self.sessions.iter().any(|sess| sess.id.0 == p);
                (!mob_live || !has_session).then_some(p)
            })
            .collect();
        for p in stale {
            self.world.riding_mut().dismount(p);
        }

        self.publish_dismounted();

        // Reconcile each session's physical mirror, then slave riders to their
        // seats. Events come from the registry transition above, not from this
        // later observation, so even sub-tick mount/dismount pairs are visible.
        for s in 0..self.sessions.len() {
            let id = self.sessions[s].id.0;
            let now = self.world.riding().mount_of(id);
            let before = std::mem::replace(&mut self.sessions[s].mount, now);
            if before.is_some() && before != now {
                self.place_dismounted_player(s);
            }
            if let Some(m) = now {
                self.slave_rider_to_seat(s, m);
            }
        }
    }

    /// Move completed registry transitions onto the event bus. The remote
    /// leave path also calls this before an id can be recycled, so a headless
    /// server retains the notification while it has no sessions or ticks.
    pub(crate) fn publish_dismounted(&mut self) {
        let detached: Vec<_> = self.world.riding_mut().drain_dismounted().collect();
        for (player, mount) in detached {
            self.bus.emit(PostEvent::PlayerDismounted {
                player: crate::server::player::PlayerId(player),
                mob_id: mount.mob_id,
            });
        }
    }

    /// Detach one session before it is persisted and removed. The registry
    /// transition is authoritative; the session mirror only decides whether
    /// the body needs physical dismount placement before saving.
    pub(crate) fn detach_departing_session(&mut self, s: usize) {
        let id = self.sessions[s].id.0;
        self.world.riding_mut().dismount(id);
        if self.sessions[s].mount.take().is_some() {
            self.place_dismounted_player(s);
        }
        self.publish_dismounted();
    }

    /// Clone one player's persistent state. Riding itself is transient, so a
    /// mounted body must be encoded at a stream-final, collision-free detached
    /// position rather than its seat-slaved transform. This moves only the
    /// clone: autosave leaves the live attachment and player untouched.
    ///
    /// `None` means no such position was provable inside the bounded search.
    /// The caller must defer this player's write, retaining the last complete
    /// save (or letting a never-saved player use fresh-spawn restore policy).
    pub(crate) fn player_snapshot_for_save(
        &self,
        s: usize,
        obstacles: &[crate::collision::DynBox],
    ) -> Option<Player> {
        let sess = &self.sessions[s];
        let mut snapshot = sess.player.clone();
        let mounted = sess.mount.is_some() || self.world.riding().mount_of(sess.id.0).is_some();
        if !mounted {
            return Some(snapshot);
        }
        let feet = self.save_dismount_spot_for(&snapshot, obstacles)?;
        snapshot.teleport(feet);
        snapshot.vel = Vec3::ZERO;
        Some(snapshot)
    }

    /// Pin one rider's player to its seat on the mount's post-tick pose. The
    /// slaved body has no physics of its own: velocity zeroes, the fall
    /// tracker re-anchors every tick (leaving a boat mid-air is a fresh fall
    /// from there), and grounding is nominal.
    fn slave_rider_to_seat(&mut self, s: usize, m: Mount) {
        let Some(idx) = self.world.mobs().index_of_id(m.mob_id) else {
            return; // vanished this tick; the next pass detaches
        };
        let mob = &self.world.mobs().instances()[idx];
        let d = crate::mob::def(mob.kind);
        let Some(&seat) = d.seats.get(m.seat as usize) else {
            return;
        };
        let pos = seat_world_pos(mob.pos, mob.yaw, seat);
        let sess = &mut self.sessions[s];
        sess.player.pos = pos;
        sess.player.vel = Vec3::ZERO;
        sess.player.on_ground = true;
        sess.fall.reset(pos.y);
        sess.pending_fall = 0.0;
        sess.pending_splash = 0.0;
    }

    /// Stand a freshly dismounted player somewhere sensible: the first
    /// collision-free spot beside where they sat (right, left, behind, ahead
    /// of the facing, at seat height or one block up), preferring dry feet;
    /// nowhere free = stay put (they'll swim or stand where the mount was).
    /// Dead/spectator riders skip placement (respawn/noclip owns them).
    fn place_dismounted_player(&mut self, s: usize) {
        let sess = &self.sessions[s];
        if sess.player.health() <= 0 || sess.player.is_spectator() {
            return;
        }
        // Solid entities — the just-left mount's hull first among them — are
        // as blocking as terrain for the landing spot.
        let obstacles = self.world.mobs().solid_obstacles();
        if let Some(feet) = self.dismount_spot_for(&sess.player, &obstacles) {
            self.sessions[s].player.teleport(feet);
        }
    }

    fn dismount_spot_for(
        &self,
        player: &Player,
        obstacles: &[crate::collision::DynBox],
    ) -> Option<Vec3> {
        dismount_spot(
            player.pos,
            player.yaw,
            |feet| player_body_free(&self.world, feet, obstacles),
            |feet| {
                let c = crate::mathh::voxel_at(feet);
                !self.world.water_cell_at(c.x, c.y, c.z)
                    && !self.world.water_cell_at(c.x, c.y - 1, c.z)
            },
        )
    }

    /// Persistence first tries the ordinary predicted dismount geometry, then
    /// expands through deterministic horizontal rings around the seat. Every
    /// candidate must read only stream-final terrain and clear all dynamic
    /// solids. Interactive dismount remains the deliberately smaller eight-
    /// probe rule above; this search only chooses a detached save snapshot.
    fn save_dismount_spot_for(
        &self,
        player: &Player,
        obstacles: &[crate::collision::DynBox],
    ) -> Option<Vec3> {
        let known_free = |feet| player_body_known_free(&self.world, feet, obstacles);
        let dry = |feet| {
            let c = crate::mathh::voxel_at(feet);
            self.world.physics_cell_final_at(c.x, c.y, c.z)
                && self.world.physics_cell_final_at(c.x, c.y - 1, c.z)
                && !self.world.water_cell_at(c.x, c.y, c.z)
                && !self.world.water_cell_at(c.x, c.y - 1, c.z)
        };
        if let Some(feet) = dismount_spot(player.pos, player.yaw, known_free, dry) {
            return Some(feet);
        }
        if !player.pos.is_finite() {
            return None;
        }

        let origin = crate::mathh::voxel_at(player.pos);
        for radius in 1..=SAVE_DISMOUNT_RADIUS {
            let mut wet = None;
            for dy in SAVE_DISMOUNT_DY {
                for dx in -radius..=radius {
                    for dz in -radius..=radius {
                        if dx.abs().max(dz.abs()) != radius {
                            continue;
                        }
                        let feet = Vec3::new(
                            (origin.x + dx) as f32 + 0.5,
                            player.pos.y + dy as f32,
                            (origin.z + dz) as f32 + 0.5,
                        );
                        if !known_free(feet) {
                            continue;
                        }
                        if dry(feet) {
                            return Some(feet);
                        }
                        wet.get_or_insert(feet);
                    }
                }
            }
            if wet.is_some() {
                return wet;
            }
        }
        None
    }
}
