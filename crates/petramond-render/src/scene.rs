use super::{
    BlockEntityInstance, EntityShadow, ItemEntityInstance, MobRenderInstance,
    ParticleEmitterInstance, ParticleInstance, PlayerBodyRender, Renderer, SolidParticleInstance,
};
use crate::views::{
    BlockEntityPresentation, DroppedItemPresentation, GamePresentation, MobPresentation,
    ParticleAtlas, ParticlePresentation,
};
use petramond_math::math::lerp_angle;

#[derive(Default)]
pub struct Scene {
    item_entities: Vec<ItemEntityInstance>,
    particles: Vec<ParticleInstance>,
    model_particles: Vec<ParticleInstance>,
    solid_particles: Vec<SolidParticleInstance>,
    particle_emitters: Vec<ParticleEmitterInstance>,
    block_entities: Vec<BlockEntityInstance>,
    block_draws: Vec<crate::BlockDrawInstance>,
    shadows: Vec<EntityShadow>,
    mobs: Vec<MobRenderInstance>,
    mob_arena: crate::MobArena,
    anim_names: crate::AnimNames,
    bodies: Vec<PlayerBodyRender>,
    body_poses: Vec<glam::Mat4>,
    held_item_skylight: u8,
    held_item_blocklight: petramond_world::light::BlockLight6,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.item_entities.clear();
        self.particles.clear();
        self.model_particles.clear();
        self.solid_particles.clear();
        self.particle_emitters.clear();
        self.block_entities.clear();
        self.block_draws.clear();
        self.mobs.clear();
        self.mob_arena.clear();
        self.anim_names = crate::AnimNames::default();
        self.shadows.clear();
        self.bodies.clear();
        self.body_poses.clear();
        self.held_item_skylight = 0;
        self.held_item_blocklight = petramond_world::light::BlockLight6::DARK;
    }

    pub fn bake(&mut self, presentation: &GamePresentation<'_>) {
        let alpha = presentation.tick_alpha;
        bake_item_entities(presentation.item_entities, alpha, &mut self.item_entities);
        self.block_draws.clear();
        self.block_draws.extend_from_slice(presentation.block_draws);
        bake_particles(
            presentation.particles,
            &mut self.particles,
            &mut self.model_particles,
            &mut self.solid_particles,
        );
        self.particle_emitters.clear();
        self.particle_emitters
            .extend_from_slice(presentation.particle_emitters);
        self.bake_block_entities(presentation.block_entities);
        bake_mobs(presentation.mobs, alpha, &mut self.mobs);
        self.mob_arena.copy_from(presentation.mob_arena);
        self.anim_names.adopt(presentation.anim_names);
        self.shadows.clear();
        self.shadows.extend_from_slice(presentation.shadows);
        self.bodies.clear();
        self.bodies.extend_from_slice(presentation.bodies);
        self.body_poses.clear();
        self.body_poses.extend_from_slice(presentation.body_poses);
        (self.held_item_skylight, self.held_item_blocklight) = presentation.held_item_light;
    }

    fn bake_block_entities(&mut self, rows: &[BlockEntityPresentation]) {
        self.block_entities.clear();
        for row in rows {
            let raw = row.open_progress;
            let b = row.block;
            self.block_entities.push(BlockEntityInstance {
                pos: b.pos,
                block: b.block,
                facing: b.pose.facing,
                variant: b.pose.variant,
                open01: raw * raw * (3.0 - 2.0 * raw),
                skylight: b.skylight,
                blocklight: b.blocklight,
            });
        }
    }

    pub fn upload(&mut self, renderer: &mut Renderer) {
        renderer.set_held_item_light(self.held_item_skylight, self.held_item_blocklight);
        renderer.swap_item_entities(&mut self.item_entities);
        renderer.swap_block_entities(&mut self.block_entities);
        renderer.swap_block_draws(&mut self.block_draws);
        renderer.swap_mobs(&mut self.mobs, &mut self.mob_arena, &self.anim_names);
        renderer.swap_shadows(&mut self.shadows);
        renderer.swap_particles(&mut self.particles);
        renderer.swap_model_particles(&mut self.model_particles);
        renderer.swap_solid_particles(&mut self.solid_particles);
        renderer.swap_particle_emitters(&mut self.particle_emitters);
        self.item_entities.clear();
        self.block_entities.clear();
        self.block_draws.clear();
        self.mobs.clear();
        self.mob_arena.clear();
        self.shadows.clear();
        self.particles.clear();
        self.model_particles.clear();
        self.solid_particles.clear();
        self.particle_emitters.clear();
        renderer.swap_player_bodies(&mut self.bodies, &mut self.body_poses);
        self.bodies.clear();
        self.body_poses.clear();
    }
}

fn bake_mobs(mobs: &[MobPresentation], alpha: f32, out: &mut Vec<MobRenderInstance>) {
    out.clear();
    out.extend(mobs.iter().map(|m| MobRenderInstance {
        kind: m.kind,
        pos: m.prev_pos.lerp(m.pos, alpha),
        yaw: lerp_angle(m.prev_yaw, m.yaw, alpha),
        tilt: m.prev_tilt.lerp(m.tilt, alpha),
        anim_time: m.prev_anim_time + (m.anim_time - m.prev_anim_time) * alpha,
        moving: m.moving,
        idle_anim: m.idle_anim,
        gait_weight: m.gait_weight,
        gait_fades: m.gait_fades,
        head_yaw: lerp_angle(m.prev_head_yaw, m.head_yaw, alpha),
        head_pitch: m.prev_head_pitch + (m.head_pitch - m.prev_head_pitch) * alpha,
        skylight: m.skylight,
        blocklight: m.blocklight,
        hurt: m.hurt_flash,
        shorn: m.shorn,
        emitter_tint: m.emitter_tint,
        emitter_self_lit: m.emitter_self_lit,
        anims: m.anims,
        ragdoll: m.ragdoll_pose,
        held: if m.dead { [None; 2] } else { m.held },
    }));
}

fn bake_item_entities(
    items: &[DroppedItemPresentation],
    alpha: f32,
    out: &mut Vec<ItemEntityInstance>,
) {
    out.clear();
    out.extend(items.iter().map(|d| ItemEntityInstance {
        pos: d.prev_pos.lerp(d.pos, alpha),
        item: d.item,
        variant: d.variant,
        count: d.count,
        pose: match (d.prev_flight, d.flight) {
            (prev, Some([yaw, pitch, speed])) => {
                let [py, pp, ps] = prev.unwrap_or([yaw, pitch, speed]);
                crate::ItemEntityPose::Aimed {
                    yaw: lerp_angle(py, yaw, alpha),
                    pitch: lerp_angle(pp, pitch, alpha),
                    speed: ps + (speed - ps) * alpha,
                    spin: lerp_angle(d.prev_spin, d.spin, alpha),
                }
            }
            (_, None) => crate::ItemEntityPose::Spin(lerp_angle(d.prev_spin, d.spin, alpha)),
        },
        skylight: d.skylight,
        blocklight: d.blocklight,
    }));
}

fn bake_particles(
    particles: &[ParticlePresentation],
    block_out: &mut Vec<ParticleInstance>,
    model_out: &mut Vec<ParticleInstance>,
    solid_out: &mut Vec<SolidParticleInstance>,
) {
    block_out.clear();
    model_out.clear();
    solid_out.clear();
    for p in particles {
        if p.atlas == ParticleAtlas::Solid {
            solid_out.push(SolidParticleInstance {
                pos: p.pos,
                color: p.tint,
                alpha: p.alpha,
                size: p.size,
                stretch: p.stretch,
                skylight: p.skylight,
                blocklight: p.blocklight,
            });
            continue;
        }
        let inst = ParticleInstance {
            quad_axes: p.quad_axes,
            pos: p.pos,
            uv_min: p.uv_min,
            uv_size: p.uv_size,
            tint: p.tint,
            alpha: p.alpha,
            size: p.size,
            skylight: p.skylight,
            blocklight: p.blocklight,
        };
        match p.atlas {
            ParticleAtlas::Block => block_out.push(inst),
            ParticleAtlas::Model => model_out.push(inst),
            ParticleAtlas::Solid => unreachable!("handled above"),
        }
    }
}

#[cfg(test)]
mod tests;
