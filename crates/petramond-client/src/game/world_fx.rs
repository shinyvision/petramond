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
    pub(super) cloth: super::cloth::ClothSystem,
    mining_feedback: DigFeedback,
    mob_digging: HashMap<u64, DigFeedback>,
    block_animations: BlockAnimations,
    local_bones: BoneEase,
    local_bone_target: Vec<crate::animation::BoneOffset>,
}

impl WorldFx {
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
                    self.burst(
                        crate::particle::BLOCK_BREAK,
                        crate::particle::BurstEvent::broken(pos, block, tint, sky, blk),
                    );
                }
                WorldEvent::PanelToggled { anchor, open } => {
                    self.block_animations.begin(anchor, !open);
                }
                WorldEvent::EmitterBurst {
                    emitter,
                    pos,
                    intensity,
                    direction,
                    look,
                } => {
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
                WorldEvent::BlockPlaced { .. }
                | WorldEvent::ChestOpened { .. }
                | WorldEvent::ChestClosed { .. }
                | WorldEvent::ItemPickedUp { .. } => {}
            }
        }
    }

    pub(super) fn burst(&mut self, key: &str, event: crate::particle::BurstEvent) {
        if let Some(spec) = petramond_world::particle_emitters::by_key(key).and_then(|b| b.burst) {
            self.particles.spawn_burst(&spec, event);
        }
    }

    pub(super) fn dig_dust(&mut self, world: &ReplicaWorld, cell: IVec3, normal: IVec3) {
        let block = Block::from_id(world.data().chunk_block(cell.x, cell.y, cell.z));
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
        let Some(h) = look.filter(|h| h.block == cell) else {
            return;
        };
        if self.mining_feedback.advance(dt).dust {
            self.dig_dust(world, cell, h.normal);
        }
    }

    pub(super) fn tick_mob_digging(
        &mut self,
        world: &ReplicaWorld,
        mobs: &ReplicatedMobs,
        dt: f32,
        sounds: &mut Vec<petramond::events::tick::SoundEvent>,
    ) {
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

    pub(super) fn tick_particles(&mut self, world: &ReplicaWorld, dt: f32) {
        self.particles.tick(dt, world);
    }

    pub(super) fn set_open_chests(&mut self, open: rustc_hash::FxHashSet<IVec3>) {
        self.block_animations.set_open_chests(open);
    }

    pub(super) fn clear_moment(&mut self) {
        self.particles.clear();
        self.cloth.clear();
        self.mining_feedback = Default::default();
        self.mob_digging.clear();
        self.block_animations = Default::default();
    }

    pub(super) fn open_chests(&self) -> &rustc_hash::FxHashSet<IVec3> {
        self.block_animations.open_chests()
    }

    #[inline]
    pub(super) fn block_open_progress(&self, anchor: IVec3, pose_open: bool) -> f32 {
        self.block_animations.open_progress(anchor, pose_open)
    }

    pub(super) fn advance_block_animations(&mut self, world: &ReplicaWorld, dt: f32) {
        self.block_animations.advance_poses(dt, |anchor| {
            let (model, pose) = world.animated_pose_at(anchor)?;
            Some((pose.open, model.open_speed))
        });
    }

    pub(super) fn ease_local_bones(&mut self, poses: &[petramond::player::BonePose], dt: f32) {
        self.local_bone_target.clear();
        super::render_bone_offsets(poses, &mut self.local_bone_target);
        self.local_bones.advance(&self.local_bone_target, dt);
    }

    pub(super) fn local_bones(&self) -> &[crate::animation::BoneOffset] {
        self.local_bones.current()
    }
}
