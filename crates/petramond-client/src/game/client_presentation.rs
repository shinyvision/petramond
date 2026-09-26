//! Game-owned client presentation/activity helpers.
//!
//! These methods expose neutral animation, activity, light, and mesh-budget policy
//! to sibling game modules without moving renderer DTOs into `Game`. Only
//! REPLICATED state is read here (the stores + the replica world — the sim
//! lives on the server thread); everything mutated is
//! client-owned (particles, block animations, the mesh pump).

use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

use super::dig_feedback::DigFeedback;
use super::Game;

impl Game {
    /// Spawn the presentation consequences of this frame's fixed ticks from
    /// the REPLICATED world-anchored events — break bursts and block-swing
    /// seeds. Deliberately event-driven (not read from sim state): every
    /// client, local or remote, drives these off the identical messages.
    pub(super) fn apply_world_effects(&mut self, events: &[super::tick::WorldEvent]) {
        for ev in events {
            match *ev {
                super::tick::WorldEvent::BlockBroken {
                    pos,
                    block,
                    normal,
                    tint,
                } => {
                    // Sampled against the REPLICA, which already applied this
                    // pump's deltas (the break landed before the events).
                    let (sky, blk) =
                        petramond::rules::breaking::break_light(self.replica.data(), pos, normal);
                    // A swing in progress on the broken block lapses on the
                    // next advance, which finds no animated block there.
                    self.burst(
                        crate::particle::BLOCK_BREAK,
                        crate::particle::BurstEvent::broken(pos, block, tint, sky, blk),
                    );
                }
                super::tick::WorldEvent::PanelToggled { anchor, open } => {
                    // Seed the swing from the panel's OLD resting pose so it
                    // eases to the new one; a mid-swing entry keeps its angle.
                    self.block_animations.begin(anchor, !open);
                }
                super::tick::WorldEvent::EmitterBurst {
                    emitter,
                    pos,
                    intensity,
                    direction,
                    look,
                } => {
                    // A one-shot burst bundle: spawn its physics particles into
                    // the client-local system, world-lit at the burst point.
                    let Some(spec) = petramond_world::particle_emitters::def(emitter)
                        .and_then(|b| b.burst.as_ref())
                    else {
                        continue;
                    };
                    let c = pos.block();
                    let (sky, blk) = self.replica.data().dynamic_light_at_world(c.x, c.y, c.z);
                    self.particles.spawn_burst(
                        spec,
                        crate::particle::BurstEvent {
                            pos,
                            intensity,
                            direction: direction.map(petramond_math::math::Vec3::from),
                            look,
                            skylight: sky,
                            blocklight: blk,
                        },
                    );
                }
                // Sounds only (played by the app); lids follow `open_chests`
                // (see `set_open_chests`).
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
        self_events: &petramond::net::protocol::SelfEvents,
    ) {
        if self_events.open_screen != Some(petramond::net::protocol::OpenScreen::Sleep) {
            return;
        }
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
        self.sync_camera_to_player_eye(0.0);
    }

    /// Dust off the face the LOCAL player is mining, gated on the REPLICATED
    /// mining state (`SelfView`) and paced on real frame time. Its dig sound
    /// is the app's loop, so only the dust pulse is read.
    pub(super) fn tick_mining_dust(&mut self, dt: f32) {
        let Some((cell, _)) = self.self_view.mining else {
            self.mining_feedback = DigFeedback::default();
            return;
        };
        // The dust is the MINED cell's. The fresh raycast only says which of
        // its faces is struck: on the frames it has already moved on (the
        // block just broke, the aim slid off) it names another block, and
        // dust cut from that one is the wrong colour.
        let Some(h) = self.look.filter(|h| h.block == cell) else {
            return;
        };
        if self.mining_feedback.advance(dt).dust {
            self.dig_dust(cell, h.normal);
        }
    }

    /// Fire one of the engine's own burst rows.
    pub(super) fn burst(&mut self, key: &str, event: crate::particle::BurstEvent) {
        if let Some(spec) = petramond_world::particle_emitters::by_key(key).and_then(|b| b.burst) {
            self.particles.spawn_burst(&spec, event);
        }
    }

    /// A digging mob reads like a player mining: dust off the face it works
    /// and the block's dig hit at the same pace, both from its replicated dig
    /// state. The sounds are returned for the frame's events.
    pub(super) fn tick_mob_digging(&mut self, dt: f32) -> Vec<petramond::events::tick::SoundEvent> {
        let digging: Vec<(u64, IVec3, WorldPos)> = self
            .entities
            .mobs()
            .iter()
            .filter_map(|m| {
                let (cell, _) = m.curr.dig?;
                let eye = petramond::mob::def(petramond::mob::Mob(m.curr.kind_id)).eye_height;
                Some((m.curr.id, cell, m.curr.pos + Vec3::new(0.0, eye, 0.0)))
            })
            .collect();
        let live: std::collections::HashSet<u64> = digging.iter().map(|(id, ..)| *id).collect();
        self.mob_digging.retain(|id, _| live.contains(id));
        let mut sounds = Vec::new();
        for (id, cell, eye) in digging {
            let pulse = self
                .mob_digging
                .entry(id)
                .or_insert_with(DigFeedback::primed)
                .advance(dt);
            let centre = WorldPos::block_center(cell);
            if pulse.hit {
                let block = Block::from_id(self.replica.data().chunk_block(cell.x, cell.y, cell.z));
                if let Some(sound) = block.sound(petramond_world::block::BlockSoundAction::Dig) {
                    sounds.push(petramond::events::tick::SoundEvent {
                        sound,
                        pos: Some(centre),
                    });
                }
            }
            if pulse.dust {
                // The face turned toward the digger's eye, where its reach starts.
                let to = (eye - centre).to_array();
                let axis = (0..3)
                    .max_by(|a, b| to[*a].abs().total_cmp(&to[*b].abs()))
                    .unwrap_or(1);
                let mut normal = IVec3::ZERO;
                normal[axis] = if to[axis] < 0.0 { -1 } else { 1 };
                self.dig_dust(cell, normal);
            }
        }
        sounds
    }

    /// Per-frame presentation update: only particles, which are a purely visual effect
    /// (they don't touch the world — they collide against the REPLICA). Everything that
    /// simulates the world or its entities — mob AI/physics AND dropped-item physics —
    /// runs on the fixed game tick (see `ServerGame::game_tick_step`); the renderer
    /// interpolates between ticks.
    pub(super) fn tick_entities(&mut self, dt: f32) {
        self.particles.tick(dt, &self.replica);
    }

    /// Adopt the REPLICATED open-chest set (the server's viewer counts); see
    /// `BlockAnimations::set_open_chests`.
    pub(super) fn set_open_chests(&mut self, open: rustc_hash::FxHashSet<IVec3>) {
        self.block_animations.set_open_chests(open);
    }

    /// The linear open progress (`0.0` closed .. `1.0` open) of the animated
    /// block anchored at `anchor` whose cell pose says `pose_open`: eased
    /// mid-swing, else resting at its logical state — the pose, or anyone
    /// looking inside it. The presentation snapshot reads this per block.
    #[inline]
    pub(super) fn block_open_progress(&self, anchor: IVec3, pose_open: bool) -> f32 {
        self.block_animations.open_progress(anchor, pose_open)
    }

    /// Advance every animated block mid-swing by `dt` toward its logical open
    /// state (its cell pose — flipped on the tick server-side and mirrored
    /// onto the REPLICA by the cell-state deltas — or the replicated
    /// open-chest set), at its model's own speed.
    pub(super) fn advance_block_animations(&mut self, dt: f32) {
        let world = &self.replica;
        self.block_animations.advance_poses(dt, |anchor| {
            let (model, pose) = world.animated_pose_at(anchor)?;
            Some((pose.open, model.open_speed))
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
        self.entities.alpha()
    }

    /// Two-channel light at the player's eye, for lighting the first-person hand
    /// / held item: it brightens AND takes the colour of nearby block light,
    /// and the torch channel keeps it lit at night.
    pub(super) fn held_item_light(&self) -> (u8, petramond_world::light::BlockLight6) {
        let c = self.cam.pos.block();
        self.replica.data().dynamic_light_at_world(c.x, c.y, c.z)
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
        let replica = &self.replica;
        self.net
            .report_terrain_backlog(|| replica.terrain_presentation_backlog());
    }
}
