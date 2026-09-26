//! Client-side world presentation effects, owned as one subsystem: the
//! particle system, dig feedback pacing (the local player's mining and every
//! replicated digger), animated-block easing, and the local body's bone
//! easing.
//!
//! Nothing here touches simulation state. Every world read goes through the
//! replica borrow the caller passes in, so the effects can be driven (and
//! tested) against any replica without the rest of the session.

use std::collections::HashMap;

use petramond::world::ReplicaWorld;
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::block::Block;

use super::block_animation::BlockAnimations;
use super::bone_ease::BoneEase;
use super::dig_feedback::DigFeedback;
use super::replicated::ReplicatedMobs;
use super::tick::WorldEvent;
use crate::particle::ParticleSystem;

#[derive(Default)]
pub(super) struct WorldFx {
    pub(super) particles: ParticleSystem,
    /// Dust pacing while the local player is actively mining.
    mining_feedback: DigFeedback,
    /// Dust and dig-hit pacing per digging mob.
    mob_digging: HashMap<u64, DigFeedback>,
    /// The eased open fraction of every animated block mid-swing (a chest's
    /// lid, a door's or trapdoor's panel), keyed by its anchor cell. Seeded
    /// when a block's logical open state changes, eased by
    /// [`advance_block_animations`](Self::advance_block_animations), read per
    /// frame by the presentation snapshot through
    /// [`block_open_progress`](Self::block_open_progress). Client-side
    /// animation only, never persisted — the authoritative state lives in the
    /// cell-state store and the replicated open-chest set (which it also
    /// holds).
    block_animations: BlockAnimations,
    /// The local body's eased bone offsets — the third-person twin of the
    /// held item's pose easing, at the same rate. Holds the eased value
    /// between frames; the presentation gather copies it into the frame's
    /// arena.
    local_bones: BoneEase,
    /// Scratch for this frame's resolved bone-offset target, reused so
    /// advancing the local body's easing allocates nothing.
    local_bone_target: Vec<crate::animation::BoneOffset>,
}

impl WorldFx {
    /// Spawn the presentation consequences of this frame's fixed ticks from
    /// the REPLICATED world-anchored events — break bursts and block-swing
    /// seeds. Deliberately event-driven (not read from sim state): every
    /// client, local or remote, drives these off the identical messages.
    /// `world` is the replica, which already applied this pump's deltas (the
    /// break landed before the events).
    pub(super) fn apply_world_effects(&mut self, world: &ReplicaWorld, events: &[WorldEvent]) {
        for ev in events {
            match *ev {
                WorldEvent::BlockBroken {
                    pos,
                    block,
                    normal,
                    tint,
                } => {
                    let (sky, blk) =
                        petramond::rules::breaking::break_light(world.data(), pos, normal);
                    // A swing in progress on the broken block lapses on the
                    // next advance, which finds no animated block there.
                    self.burst(
                        crate::particle::BLOCK_BREAK,
                        crate::particle::BurstEvent::broken(pos, block, tint, sky, blk),
                    );
                }
                WorldEvent::PanelToggled { anchor, open } => {
                    // Seed the swing from the panel's OLD resting pose so it
                    // eases to the new one; a mid-swing entry keeps its angle.
                    self.block_animations.begin(anchor, !open);
                }
                WorldEvent::EmitterBurst {
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
                    let (sky, blk) = world.data().dynamic_light_at_world(c.x, c.y, c.z);
                    self.particles.spawn_burst(
                        spec,
                        crate::particle::BurstEvent {
                            pos,
                            intensity,
                            direction: direction.map(Vec3::from),
                            look,
                            skylight: sky,
                            blocklight: blk,
                        },
                    );
                }
                // Sounds only (played by the app); lids follow `open_chests`
                // (see `set_open_chests`).
                WorldEvent::BlockPlaced { .. }
                | WorldEvent::ChestOpened { .. }
                | WorldEvent::ChestClosed { .. }
                | WorldEvent::ItemPickedUp { .. } => {}
            }
        }
    }

    /// Fire one of the engine's own burst rows.
    pub(super) fn burst(&mut self, key: &str, event: crate::particle::BurstEvent) {
        if let Some(spec) = petramond_world::particle_emitters::by_key(key).and_then(|b| b.burst) {
            self.particles.spawn_burst(&spec, event);
        }
    }

    /// Dust off `cell`'s `normal` face, cut from the block and lit from the
    /// open side.
    pub(super) fn dig_dust(&mut self, world: &ReplicaWorld, cell: IVec3, normal: IVec3) {
        let block = Block::from_id(world.data().chunk_block(cell.x, cell.y, cell.z));
        // A cell already broken (a replicated dig outlives its block by a
        // beat) sheds nothing.
        if block == Block::Air {
            return;
        }
        let lit = cell + normal;
        let (sky, blk) = world.data().dynamic_light_at_world(lit.x, lit.y, lit.z);
        let kv_tint = world.data().cell_burst_tint(cell);
        self.burst(
            crate::particle::BLOCK_DUST,
            crate::particle::BurstEvent::struck(cell, normal, block, kv_tint, sky, blk),
        );
    }

    /// Dust off the face the LOCAL player is mining, gated on the REPLICATED
    /// mining cell (`SelfView::mining`) and paced on real frame time. Its dig
    /// sound is the app's loop, so only the dust pulse is read. `look` is the
    /// fresh raycast: it only says which face of the mined cell is struck.
    pub(super) fn tick_mining_dust(
        &mut self,
        world: &ReplicaWorld,
        mining: Option<IVec3>,
        look: Option<petramond::player::RaycastHit>,
        dt: f32,
    ) {
        let Some(cell) = mining else {
            self.mining_feedback = DigFeedback::default();
            return;
        };
        // The dust is the MINED cell's. On the frames the raycast has already
        // moved on (the block just broke, the aim slid off) it names another
        // block, and dust cut from that one is the wrong colour.
        let Some(h) = look.filter(|h| h.block == cell) else {
            return;
        };
        if self.mining_feedback.advance(dt).dust {
            self.dig_dust(world, cell, h.normal);
        }
    }

    /// A digging mob reads like a player mining: dust off the face it works
    /// and the block's dig hit at the same pace, both from its replicated dig
    /// state. The dig sounds are appended to `sounds`.
    pub(super) fn tick_mob_digging(
        &mut self,
        world: &ReplicaWorld,
        mobs: &ReplicatedMobs,
        dt: f32,
        sounds: &mut Vec<petramond::events::tick::SoundEvent>,
    ) {
        // A digger that stopped (or left the view) drops its pacing, so a
        // fresh dig starts primed.
        self.mob_digging
            .retain(|id, _| mobs.get(*id).is_some_and(|m| m.curr.dig.is_some()));
        for m in mobs.iter() {
            let Some((cell, _)) = m.curr.dig else {
                continue;
            };
            let eye_height = petramond::mob::def(petramond::mob::Mob(m.curr.kind_id)).eye_height;
            let eye = m.curr.pos + Vec3::new(0.0, eye_height, 0.0);
            let pulse = self
                .mob_digging
                .entry(m.curr.id)
                .or_insert_with(DigFeedback::primed)
                .advance(dt);
            let centre = WorldPos::block_center(cell);
            if pulse.hit {
                let block = Block::from_id(world.data().chunk_block(cell.x, cell.y, cell.z));
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
                self.dig_dust(world, cell, normal);
            }
        }
    }

    /// Per-frame particle update: a purely visual effect colliding against
    /// the replica. Everything that simulates the world or its entities runs
    /// on the server's fixed tick; the renderer interpolates between ticks.
    pub(super) fn tick_particles(&mut self, world: &ReplicaWorld, dt: f32) {
        self.particles.tick(dt, world);
    }

    /// Adopt the REPLICATED open-chest set (the server's viewer counts); see
    /// `BlockAnimations::set_open_chests`.
    pub(super) fn set_open_chests(&mut self, open: rustc_hash::FxHashSet<IVec3>) {
        self.block_animations.set_open_chests(open);
    }

    /// The chests someone is looking inside, as last replicated.
    #[cfg(test)]
    pub(super) fn open_chests(&self) -> &rustc_hash::FxHashSet<IVec3> {
        self.block_animations.open_chests()
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
    /// onto the replica by the cell-state deltas — or the replicated
    /// open-chest set), at its model's own speed.
    pub(super) fn advance_block_animations(&mut self, world: &ReplicaWorld, dt: f32) {
        self.block_animations.advance_poses(dt, |anchor| {
            let (model, pose) = world.animated_pose_at(anchor)?;
            Some((pose.open, model.open_speed))
        });
    }

    /// Ease the local body's bones toward this frame's resolved poses — the
    /// same rate as the item in its fist, so an arm never snaps while its
    /// shield glides.
    pub(super) fn ease_local_bones(&mut self, poses: &[petramond::player::BonePose], dt: f32) {
        self.local_bone_target.clear();
        super::render_bone_offsets(poses, &mut self.local_bone_target);
        self.local_bones.advance(&self.local_bone_target, dt);
    }

    /// The local body's eased bone offsets as of the last advance.
    pub(super) fn local_bones(&self) -> &[crate::animation::BoneOffset] {
        self.local_bones.current()
    }
}
