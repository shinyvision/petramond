//! Render-side scene adapter: bakes neutral per-frame presentation data into the
//! renderer's flat wire structs.
//!
//! [`Scene`] is the translation layer the App owns between the neutral game
//! presentation snapshot and the [`Renderer`]. Each frame the App builds the snapshot,
//! calls [`Scene::bake`], then [`Scene::upload`] to hand the baked instances to the
//! renderer. Keeping the wire structs (`ItemEntityInstance` / `ParticleInstance` /
//! `BlockEntityInstance`) and their reused buffers here keeps renderer presentation types out
//! of simulation code.
//!
//! The buffers are cleared + refilled (capacity reused) so a bounded per-frame count
//! never reallocs.

use super::{
    BlockEntityInstance, EntityShadow, ItemEntityInstance, MobRenderInstance,
    ParticleEmitterInstance, ParticleInstance, PlayerBodyRender, Renderer, SolidParticleInstance,
};
use crate::views::{
    BlockEntityPresentation, DroppedItemPresentation, GamePresentation, MobPresentation,
    ParticleAtlas, ParticlePresentation,
};
use petramond_math::math::lerp_angle;

/// Per-frame presentation translation state, owned by the App. Holds the renderer's
/// flat instance buffers reused across frames, plus the held-item skylight sampled
/// this frame.
#[derive(Default)]
pub struct Scene {
    /// Baked dropped-item cubes/extruded sprites for this frame.
    item_entities: Vec<ItemEntityInstance>,
    /// Baked block-atlas particle cubes for this frame.
    particles: Vec<ParticleInstance>,
    /// Baked model-atlas particle cubes (bbmodel-block flecks) for this frame — drawn in
    /// the same pass but bound to the model atlas.
    model_particles: Vec<ParticleInstance>,
    /// Baked solid-color simulated particles (emitter-burst droplets) for this
    /// frame — drawn alpha-blended with the looping-emitter cubes.
    solid_particles: Vec<SolidParticleInstance>,
    /// Baked block-row particle emitters for this frame.
    particle_emitters: Vec<ParticleEmitterInstance>,
    /// Baked animated-block instances (chests, doors, trapdoors, ...) for this
    /// frame.
    block_entities: Vec<BlockEntityInstance>,
    /// Mod draw sets for this frame — a copy of the gather, already view-culled
    /// there (`World::collect_block_draws`), which is where the rule puts it:
    /// a presentation gather scales with what is VISIBLE, not what is loaded.
    block_draws: Vec<crate::BlockDrawInstance>,
    /// Entity blob-shadow rows for this frame — the gather's own rows, a copy
    /// (they are already resolved; nothing to interpolate).
    shadows: Vec<EntityShadow>,
    /// Baked (interpolated) mob instances for this frame.
    mobs: Vec<MobRenderInstance>,
    /// The arena the mob rows' ranges address (the gather's, copied once), and
    /// the session's animation-name table their layer ids index.
    mob_arena: crate::MobArena,
    anim_names: crate::AnimNames,
    /// Every posed player body in view (already posed by the client's
    /// animation — a pass-through here), and the pose arena their
    /// `PlayerRenderInstance::pose` ranges index into.
    bodies: Vec<PlayerBodyRender>,
    body_poses: Vec<glam::Mat4>,
    /// Two-channel light for the first-person hand / held item, sampled at the
    /// camera each frame so it brightens AND takes the colour of nearby block
    /// light (which keeps it lit at night).
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

    /// Translate the current presentation snapshot into this scene's reused buffers.
    /// Dropped items' cached skylight is kept fresh by the sim's per-tick light refresh,
    /// so baking just reads it here.
    pub fn bake(&mut self, presentation: &GamePresentation<'_>) {
        // Items and mobs both simulate on the fixed game tick; `alpha` blends the
        // previous and current tick poses so they move smoothly at any frame rate.
        let alpha = presentation.tick_alpha;
        bake_item_entities(presentation.item_entities, alpha, &mut self.item_entities);
        // Same row type end to end, so this is a clear + extend of an owned
        // copy (the presentation borrow ends before `upload`), not a rebuild.
        self.block_draws.clear();
        self.block_draws.extend_from_slice(presentation.block_draws);
        bake_particles(
            presentation.particles,
            &mut self.particles,
            &mut self.model_particles,
            &mut self.solid_particles,
        );
        // Already the render row, already culled by the gather — a copy into
        // the frame's own buffer, not a translation.
        self.particle_emitters.clear();
        self.particle_emitters
            .extend_from_slice(presentation.particle_emitters);
        self.bake_block_entities(presentation.block_entities);
        bake_mobs(presentation.mobs, alpha, &mut self.mobs);
        // The rows' ranges index the gather's arena as it stands, so it rides
        // along verbatim.
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

    /// The animated blocks to draw this frame, gathered from the loaded chunks.
    /// The linear open progress is smoothstepped so every lid and panel
    /// accelerates and decelerates instead of swinging at a constant rate.
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

    /// Hand the baked instances + held-item light to the renderer for this
    /// frame. The row lists and per-body arenas are SWAPPED in rather than
    /// copied: the renderer's last-frame buffers come back here, and the row
    /// lists are emptied so a second upload without a bake hands over nothing
    /// rather than last frame's rows.
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

/// Map each mob presentation row to one interpolated [`MobRenderInstance`] (cleared +
/// refilled, capacity reused). The simulation advances mobs on the fixed game tick;
/// `alpha` (`0..1`, the fraction into the next tick) blends the previous and current
/// tick poses so motion stays smooth at any frame rate. The arena ranges pass
/// through unchanged: the scene carries the gather's arena verbatim.
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

/// Map each dropped-item row to one [`ItemEntityInstance`] (cleared + refilled,
/// capacity reused). `alpha` (`0..1`, the fraction into the next tick) blends the
/// previous and current tick pose so a falling/drifting drop moves smoothly, exactly
/// like a mob. The skylight rides through from the row's cached value.
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
            // A heading that just appeared (launched this tick) has no
            // previous turn to blend from, so it snaps to where it points —
            // intentional: flight is replicated, never predicted, and a
            // blend from the loose spin would turn a fresh launch mid-air.
            (prev, Some([yaw, pitch, speed])) => {
                let [py, pp, ps] = prev.unwrap_or([yaw, pitch, speed]);
                crate::ItemEntityPose::Aimed {
                    yaw: lerp_angle(py, yaw, alpha),
                    pitch: lerp_angle(pp, pitch, alpha),
                    speed: ps + (speed - ps) * alpha,
                }
            }
            (_, None) => crate::ItemEntityPose::Spin(lerp_angle(d.prev_spin, d.spin, alpha)),
        },
        skylight: d.skylight,
        blocklight: d.blocklight,
    }));
}

/// Map each particle row to one [`ParticleInstance`], split by atlas: BLOCK-atlas
/// flecks into `block_out`, bbmodel-block (MODEL-atlas) flecks into `model_out`
/// (both cleared + refilled, capacity reused). The two are drawn in one pass with the
/// matching texture bound, so a broken workbench's flecks sample its own texture.
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
        // A solid-color particle (emitter-burst droplet) has no atlas patch:
        // it joins the alpha-blended cube pass instead of the cutout one.
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
