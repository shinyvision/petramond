//! Game-owned client presentation/activity helpers.
//!
//! These methods expose neutral animation, activity, light, and mesh-budget policy
//! to sibling game modules without moving renderer DTOs into `Game`. Only
//! REPLICATED state is read here (the stores + the replica world — the sim
//! lives on the server thread); everything mutated is
//! client-owned (particles, lids, swings, the mesh pump).

use crate::block::{Block, ShapeFamily};
use crate::mathh::{voxel_at, IVec3};

use super::{Game, MINING_DUST_INTERVAL};

/// Chest-lid open/close speed (fraction per second)
const CHEST_LID_SPEED: f32 = 3.5;

/// Door swing open/close speed (fraction per second). A touch slower than the chest
/// lid so the 90 degree swing reads as a deliberate door, not a snap.
const DOOR_SWING_SPEED: f32 = 4.5;

impl Game {
    /// Spawn the presentation consequences of this frame's fixed ticks from
    /// the REPLICATED world-anchored events — break bursts and door-swing
    /// seeds. Deliberately event-driven (not read from sim state): every
    /// client, local or remote, drives these off the identical messages.
    pub(super) fn apply_world_effects(&mut self, events: &[super::tick::WorldEvent]) {
        for ev in events {
            match *ev {
                super::tick::WorldEvent::BlockBroken { pos, block, normal } => {
                    // Sampled against the REPLICA, which already applied this
                    // pump's deltas (the break landed before the events).
                    let (sky, blk, warm) =
                        crate::server::breaking::break_light(&self.replica, pos, normal);
                    match block.model_kind() {
                        Some(kind) => self
                            .particles
                            .spawn_break_burst_model(pos, kind, sky, blk, warm),
                        None => self
                            .particles
                            .spawn_break_burst_lit(pos, block, sky, blk, warm),
                    }
                    // A broken door's swing entry dies with it (client-owned
                    // state the sim can no longer clear). The event carries
                    // the mined cell — either half — so the LOWER cell (the
                    // swing key) is that cell or the one below.
                    if block.shape_family() == ShapeFamily::Door {
                        self.door_swings.remove(&pos);
                        self.door_swings.remove(&(pos + IVec3::new(0, -1, 0)));
                    }
                }
                super::tick::WorldEvent::DoorToggled { lower, open } => {
                    // Seed the swing from the door's OLD resting pose so it
                    // eases to the new one; a mid-swing entry keeps its angle.
                    self.door_swings
                        .entry(lower)
                        .or_insert(if open { 0.0 } else { 1.0 });
                }
                super::tick::WorldEvent::EmitterBurst {
                    emitter,
                    pos,
                    intensity,
                } => {
                    // A one-shot burst bundle: spawn its physics particles into
                    // the client-local system, world-lit at the burst point.
                    let Some(spec) =
                        crate::particle_emitters::def(emitter).and_then(|b| b.burst.as_ref())
                    else {
                        continue;
                    };
                    let c = voxel_at(pos);
                    let (sky, blk, warm) = self.replica.dynamic_light_at_world(c.x, c.y, c.z);
                    self.particles
                        .spawn_emitter_burst(spec, pos, intensity, sky, blk, warm);
                }
                // Sounds only (played by the app); lids follow `open_chests`.
                super::tick::WorldEvent::BlockPlaced { .. }
                | super::tick::WorldEvent::ChestOpened { .. }
                | super::tick::WorldEvent::ChestClosed { .. }
                | super::tick::WorldEvent::ItemPickedUp { .. } => {}
            }
        }
    }

    /// Falling asleep tucks the local player in on the tick (`ServerGame`'s
    /// bed stage); the camera mirror is presentation, applied off the
    /// replicated sleep-open one-shot right after the fixed ticks — before
    /// any presentation read. The tucked transform was already adopted into
    /// the predicted player (`adopt_authoritative_transform` runs first).
    pub(super) fn sync_sleep_camera_on_open(
        &mut self,
        self_events: &crate::net::protocol::SelfEvents,
    ) {
        if self_events.open_screen != Some(crate::net::protocol::OpenScreen::Sleep) {
            return;
        }
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
        self.sync_camera_to_player_eye(0.0);
    }

    /// A small dust fleck every [`MINING_DUST_INTERVAL`] while the LOCAL player
    /// is actively mining, gated on the REPLICATED mining state (`SelfView`).
    /// Presentation only; remote players' dust is client-derived the same
    /// way, from their replicated mining state. Per frame — the
    /// cadence is paced on REAL frame time now, where it used to accumulate
    /// TICK_DT inside the fixed tick; both tick over every 0.1 s, so the fleck
    /// rate is visually identical.
    pub(super) fn tick_mining_dust(&mut self, dt: f32) {
        if self.self_view.mining.is_none() {
            self.mining_dust_t = 0.0;
            return;
        }
        // The dust anchors on the CLIENT's fresh raycast (same cell the
        // session's latched target came from).
        let Some(h) = self.look else {
            return;
        };
        self.mining_dust_t += dt.clamp(0.0, MINING_DUST_INTERVAL);
        if self.mining_dust_t < MINING_DUST_INTERVAL {
            return;
        }
        self.mining_dust_t = 0.0;
        let world = &self.replica;
        let block = Block::from_id(world.chunk_block(h.block.x, h.block.y, h.block.z));
        let cell = h.block + h.normal;
        let (sky, blk, warm) = world.dynamic_light_at_world(cell.x, cell.y, cell.z);
        match block.model_kind() {
            Some(kind) => self
                .particles
                .spawn_mining_model(h.block, h.normal, kind, sky, blk, warm),
            None => self
                .particles
                .spawn_mining_lit(h.block, h.normal, block, sky, blk, warm),
        }
    }

    /// Per-frame presentation update: only particles, which are a purely visual effect
    /// (they don't touch the world — they collide against the REPLICA). Everything that
    /// simulates the world or its entities — mob AI/physics AND dropped-item physics —
    /// runs on the fixed game tick (see `ServerGame::game_tick_step`); the renderer
    /// interpolates between ticks.
    pub(super) fn tick_entities(&mut self, dt: f32) {
        self.particles.tick(dt, &self.replica);
    }

    /// The transient open progress (`0.0` closed .. `1.0` open) of the chest at
    /// `pos`, or `0.0` if it isn't tracked. The presentation snapshot reads this
    /// to bake the chest's lid hinge; the easing/animation lives in
    /// [`advance_chest_lids`](Self::advance_chest_lids).
    #[inline]
    pub(super) fn chest_lid_angle(&self, pos: IVec3) -> f32 {
        self.chest_lids.get(&pos).copied().unwrap_or(0.0)
    }

    /// Advance the transient chest-lid animation by `dt`: the open chest's lid eases
    /// toward fully open, every other tracked lid toward closed, and lids that reach
    /// closed (and aren't the open chest) are dropped. The open/closed target is the
    /// REPLICATED open-chest set (`TickUpdate::open_chests` — the server's viewer
    /// counts), so the lid follows any player's open screen, purely client-side,
    /// never saved.
    pub(super) fn advance_chest_lids(&mut self, dt: f32) {
        let step = (dt * CHEST_LID_SPEED).clamp(0.0, 1.0);
        // Ensure every viewed chest is tracked so it animates from closed on
        // the first frame — ANY player's open lifts the lid on every screen.
        for pos in &self.open_chests {
            self.chest_lids.entry(*pos).or_insert(0.0);
        }
        let open = &self.open_chests;
        self.chest_lids.retain(|&pos, lid| {
            let target = if open.contains(&pos) { 1.0 } else { 0.0 };
            if *lid < target {
                *lid = (*lid + step).min(target);
            } else if *lid > target {
                *lid = (*lid - step).max(target);
            }
            // Keep while still animating, or while anyone is looking inside.
            *lid > f32::EPSILON || open.contains(&pos)
        });
    }

    /// The transient swing angle (`0.0` closed .. `1.0` open) of the door whose LOWER
    /// cell is `lower`. While a door is mid-swing the eased value is read from
    /// [`door_swings`](Self::door_swings); once it settles the entry is dropped and the
    /// door rests at its logical open state (read straight from the door map). The
    /// presentation snapshot calls this per visible door to bake its hinge.
    #[inline]
    pub(super) fn door_swing_angle(&self, lower: IVec3) -> f32 {
        if let Some(&a) = self.door_swings.get(&lower) {
            return a;
        }
        // Not animating: rest at the door's logical state (replica door map).
        match self.replica.door_state_at(lower.x, lower.y, lower.z) {
            Some(s) if s.open => 1.0,
            _ => 0.0,
        }
    }

    /// Advance the transient door-swing animation by `dt`: each tracked door eases
    /// toward its current logical open state (flipped on the tick by
    /// [`World::toggle_door`] server-side, mirrored onto the REPLICA's door map by
    /// the `Door` state deltas), and a door that reaches its target is dropped (it
    /// then rests at that state). Purely client-side, never saved, like
    /// [`advance_chest_lids`](Self::advance_chest_lids).
    ///
    /// [`World::toggle_door`]: crate::world::World::toggle_door
    pub(super) fn advance_door_swings(&mut self, dt: f32) {
        let step = (dt * DOOR_SWING_SPEED).clamp(0.0, 1.0);
        let world = &self.replica;
        self.door_swings.retain(|&lower, angle| {
            let target = match world.door_state_at(lower.x, lower.y, lower.z) {
                Some(s) if s.open => 1.0,
                Some(_) => 0.0,
                // The door was removed while swinging: stop tracking it.
                None => return false,
            };
            if *angle < target {
                *angle = (*angle + step).min(target);
            } else if *angle > target {
                *angle = (*angle - step).max(target);
            }
            // Keep only while still travelling toward the target.
            (*angle - target).abs() > f32::EPSILON
        });
    }

    /// Fraction (`0..1`) into the next fixed tick, the blend factor the scene uses to
    /// interpolate each entity's render pose between its previous and current tick, so the
    /// mobs and dropped items (which simulate at 20 TPS) move smoothly at any frame rate.
    /// Measured client-side from the arrival time of the last applied
    /// `TickUpdate` ([`tick::ReplicaClock`]) — the server accumulator lives on
    /// its own thread now.
    #[inline]
    pub(super) fn tick_alpha(&self) -> f32 {
        self.replica_clock.alpha()
    }

    /// Two-channel light + warm-tint amount at the player's eye, for lighting the
    /// first-person hand / held item: it brightens AND warms near torches/furnaces,
    /// and the torch channel keeps it lit at night.
    pub(super) fn held_item_light(&self) -> (u8, u8, u8) {
        let c = voxel_at(self.cam.pos);
        self.replica.dynamic_light_at_world(c.x, c.y, c.z)
    }

    pub(super) fn tick_mesh_budget(&mut self) {
        // Generous count — the pump's own time budget (MESH_SUBMIT_TIME_BUDGET) is
        // what actually protects the frame; a small count here just frame-quantized
        // streaming bursts into a multi-second trickle. Pumps the REPLICA's
        // mesh + light queues (the server world never meshes).
        // High enough that the real per-frame limits are the mesh pump's
        // in-flight window and its submit-time budget, not this count: 64
        // admission-limited RD32 flight meshing while the workers sat idle.
        const MESH_BUDGET: usize = 256;
        self.replica.tick_mesh_budget(MESH_BUDGET);
    }
}
