//! Render-side scene adapter: bakes neutral per-frame presentation data into the
//! renderer's flat wire structs.
//!
//! [`Scene`] is the translation layer the App owns between the neutral game
//! presentation snapshot and the [`Renderer`]. Each frame the App builds the snapshot,
//! calls [`Scene::bake`], then [`Scene::upload`] to hand the baked instances to the
//! renderer. Keeping the wire structs (`ItemEntityInstance` / `ParticleInstance` /
//! `ChestInstance`) and their reused buffers here keeps renderer presentation types out
//! of simulation code.
//!
//! The buffers are cleared + refilled (capacity reused) so a bounded per-frame count
//! never reallocs.

use glam::Vec3;

use super::{
    ChestInstance, DoorInstance, ItemEntityInstance, MobRenderInstance, ParticleEmitterInstance,
    ParticleInstance, PlayerRenderInstance, RemotePlayerRender, Renderer, SolidParticleInstance,
};
use crate::game::body_pose::lerp_angle;
use crate::game::presentation::{
    ChestPresentation, DoorPresentation, DroppedItemPresentation, GamePresentation,
    MobPresentation, ParticleAtlas, ParticleEmitterPresentation, ParticlePresentation,
};

/// Per-frame presentation translation state, owned by the App. Holds the renderer's
/// flat instance buffers reused across frames, plus the held-item skylight sampled
/// this frame.
#[derive(Default)]
pub(crate) struct Scene {
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
    /// Baked placed-chest instances for this frame.
    chests: Vec<ChestInstance>,
    /// Baked placed-door instances for this frame.
    doors: Vec<DoorInstance>,
    /// Baked (interpolated) mob instances for this frame.
    mobs: Vec<MobRenderInstance>,
    /// The third-person player body for this frame (`None` in first person).
    /// Player state is per-frame already, so it passes through uninterpolated.
    player: Option<PlayerRenderInstance>,
    /// Every other connected player's body + held item (already interpolated
    /// by the presentation layer — a pass-through here, like the local body).
    remote_players: Vec<RemotePlayerRender>,
    /// Two-channel light + warm-tint amount for the first-person hand / held
    /// item, sampled at the camera each frame so it brightens AND warms near
    /// torches (and torch light keeps it lit at night).
    held_item_skylight: u8,
    held_item_blocklight: u8,
    held_item_warm: u8,
}

impl Scene {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn clear(&mut self) {
        self.item_entities.clear();
        self.particles.clear();
        self.model_particles.clear();
        self.solid_particles.clear();
        self.particle_emitters.clear();
        self.chests.clear();
        self.doors.clear();
        self.mobs.clear();
        self.player = None;
        self.remote_players.clear();
        self.held_item_skylight = 0;
        self.held_item_blocklight = 0;
        self.held_item_warm = 0;
    }

    /// Translate the current presentation snapshot into this scene's reused buffers.
    /// Dropped items' cached skylight is kept fresh by the sim's per-tick light refresh,
    /// so baking just reads it here. `player_hurt` is the App's hurt-flash envelope
    /// (`0..1`, the same one driving the screen vignette) applied to the third-person
    /// body so taking damage flashes it red like a hurt mob.
    pub(crate) fn bake(&mut self, presentation: &GamePresentation<'_>, player_hurt: f32) {
        // Items and mobs both simulate on the fixed game tick; `alpha` blends the
        // previous and current tick poses so they move smoothly at any frame rate.
        let alpha = presentation.tick_alpha;
        bake_item_entities(presentation.item_entities, alpha, &mut self.item_entities);
        bake_particles(
            presentation.particles,
            &mut self.particles,
            &mut self.model_particles,
            &mut self.solid_particles,
        );
        bake_particle_emitters(presentation.particle_emitters, &mut self.particle_emitters);
        self.bake_chests(presentation.chests);
        self.bake_doors(presentation.doors);
        bake_mobs(presentation.mobs, alpha, &mut self.mobs);
        self.player = presentation.player.map(|p| PlayerRenderInstance {
            pos: p.pos,
            body_yaw: p.body_yaw,
            head_yaw: p.head_yaw,
            head_pitch: p.head_pitch,
            anim_time: p.anim_time,
            walk_weight: p.walk_weight,
            sneak_weight: p.sneak_weight,
            sleeping: p.sleeping,
            seated: p.seated,
            hurt: player_hurt,
            skylight: p.skylight,
            blocklight: p.blocklight,
        });
        self.remote_players.clear();
        self.remote_players
            .extend_from_slice(presentation.remote_players);
        (
            self.held_item_skylight,
            self.held_item_blocklight,
            self.held_item_warm,
        ) = presentation.held_item_light;
    }

    /// The placed chests to draw this frame (world pos, facing, lid angle, skylight),
    /// gathered from the loaded chunks. The linear open progress is smoothstepped so
    /// the lid accelerates and decelerates instead of swinging at a constant rate.
    fn bake_chests(&mut self, chests: &[ChestPresentation]) {
        self.chests.clear();
        for chest in chests {
            let pos = chest.pos;
            let raw = chest.lid_progress;
            let lid01 = raw * raw * (3.0 - 2.0 * raw);
            self.chests.push(ChestInstance {
                pos: Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32),
                facing: chest.facing,
                lid01,
                skylight: chest.skylight,
                blocklight: chest.blocklight,
            });
        }
    }

    /// The placed doors to draw this frame (lower pos, facing, swing angle, the two
    /// halves' tiles, skylight), gathered from the loaded chunks. The linear swing is
    /// smoothstepped so it accelerates and decelerates instead of turning at a constant
    /// rate — exactly like the chest lid.
    fn bake_doors(&mut self, doors: &[DoorPresentation]) {
        self.doors.clear();
        for door in doors {
            let pos = door.pos;
            let [bottom_tile, top_tile, side_tile] = door.tiles;
            let raw = door.swing_progress;
            let open01 = raw * raw * (3.0 - 2.0 * raw);
            self.doors.push(DoorInstance {
                pos: Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32),
                facing: door.state.facing,
                open01,
                bottom_tile,
                top_tile,
                side_tile,
                skylight: door.skylight,
                blocklight: door.blocklight,
            });
        }
    }

    /// Hand the baked instances + held-item light to the renderer for this frame.
    pub(crate) fn upload(&self, renderer: &mut Renderer) {
        renderer.set_held_item_light(
            self.held_item_skylight,
            self.held_item_blocklight,
            self.held_item_warm,
        );
        renderer.set_item_entities(&self.item_entities);
        renderer.set_chests(&self.chests);
        renderer.set_doors(&self.doors);
        renderer.set_mobs(&self.mobs);
        renderer.set_player(self.player);
        renderer.set_remote_players(&self.remote_players);
        renderer.set_particles(&self.particles);
        renderer.set_model_particles(&self.model_particles);
        renderer.set_solid_particles(&self.solid_particles);
        renderer.set_particle_emitters(&self.particle_emitters);
    }
}

/// Map each mob presentation row to one interpolated [`MobRenderInstance`] (cleared +
/// refilled, capacity reused). The simulation advances mobs on the fixed game tick;
/// `alpha` (`0..1`, the fraction into the next tick) blends the previous and current
/// tick poses so motion stays smooth at any frame rate.
fn bake_mobs(mobs: &[MobPresentation], alpha: f32, out: &mut Vec<MobRenderInstance>) {
    out.clear();
    out.extend(mobs.iter().map(|m| MobRenderInstance {
        kind: m.kind,
        pos: m.prev_pos.lerp(m.pos, alpha),
        yaw: lerp_angle(m.prev_yaw, m.yaw, alpha),
        anim_time: m.prev_anim_time + (m.anim_time - m.prev_anim_time) * alpha,
        moving: m.moving,
        idle_anim: m.idle_anim,
        head_yaw: lerp_angle(m.prev_head_yaw, m.head_yaw, alpha),
        head_pitch: m.prev_head_pitch + (m.head_pitch - m.prev_head_pitch) * alpha,
        skylight: m.skylight,
        blocklight: m.blocklight,
        hurt: m.hurt_flash,
        shorn: m.shorn,
        emitter_tint: m.emitter_tint,
        anims: m.anims.clone(),
        ragdoll: m.ragdoll_pose.clone(),
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
        count: d.count,
        spin: lerp_angle(d.prev_spin, d.spin, alpha),
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
            pos: p.pos,
            uv_min: p.uv_min,
            uv_size: p.uv_size,
            tint: crate::torch::warm_tint(p.tint, p.warm as f32 / 255.0),
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

fn bake_particle_emitters(
    emitters: &[ParticleEmitterPresentation],
    out: &mut Vec<ParticleEmitterInstance>,
) {
    out.clear();
    out.extend(emitters.iter().map(|e| ParticleEmitterInstance {
        origin: e.origin,
        emitter: e.emitter,
        seed: e.seed,
        skylight: e.skylight,
        blocklight: e.blocklight,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    /// A settled drop with `prev_pos == pos`, so it bakes to `pos` at any alpha.
    fn fresh_drop(pos: Vec3, item: ItemType) -> DroppedItemPresentation {
        DroppedItemPresentation {
            prev_pos: pos,
            pos,
            item,
            count: 1,
            prev_spin: 0.0,
            spin: 0.0,
            skylight: 0,
            blocklight: 0,
        }
    }

    fn particle_row(atlas: ParticleAtlas) -> ParticlePresentation {
        ParticlePresentation {
            atlas,
            pos: Vec3::new(0.0, 64.0, 0.0),
            uv_min: [0.0, 0.0],
            uv_size: 0.0625,
            tint: [1.0, 1.0, 1.0],
            warm: 0,
            alpha: 1.0,
            size: 0.1,
            stretch: 1.0,
            skylight: 0,
            blocklight: 0,
        }
    }

    #[test]
    fn bake_item_entities_one_instance_per_drop() {
        let drops = vec![
            fresh_drop(Vec3::new(1.0, 2.0, 3.0), ItemType::Dirt),
            fresh_drop(Vec3::new(4.0, 5.0, 6.0), ItemType::Stone),
        ];
        let mut out = Vec::new();
        bake_item_entities(&drops, 1.0, &mut out);
        assert_eq!(out.len(), 2);
        // A fresh drop has prev_pos == pos, so any alpha bakes its live position.
        assert_eq!(out[0].pos, drops[0].pos);
        assert_eq!(out[0].item, ItemType::Dirt);
        assert_eq!(out[1].item, ItemType::Stone);
    }

    #[test]
    fn bake_item_entities_interpolates_between_ticks() {
        // A drop that moved last tick (prev_pos != pos) bakes at the blended position,
        // so it renders smoothly between the 20 TPS physics ticks.
        let drop = DroppedItemPresentation {
            prev_pos: Vec3::new(0.0, 64.0, 0.0),
            pos: Vec3::new(2.0, 64.0, 0.0),
            ..fresh_drop(Vec3::ZERO, ItemType::Dirt)
        };
        let mut out = Vec::new();
        bake_item_entities(std::slice::from_ref(&drop), 0.5, &mut out);
        assert_eq!(
            out[0].pos,
            Vec3::new(1.0, 64.0, 0.0),
            "halfway between prev and current"
        );
    }

    #[test]
    fn bake_item_entities_reuses_the_vec_without_growth() {
        let drops: Vec<_> = (0..8)
            .map(|i| fresh_drop(Vec3::splat(i as f32), ItemType::Dirt))
            .collect();
        let mut out = Vec::new();
        bake_item_entities(&drops, 1.0, &mut out);
        let cap = out.capacity();
        // Fewer drops -> identical-or-smaller count, so the cleared+refilled buffer
        // keeps its capacity: rebuilding never reallocs.
        bake_item_entities(&drops[..2], 1.0, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out.capacity(), cap, "instance buffer reused");
    }

    #[test]
    fn bake_particles_splits_rows_by_atlas() {
        // Each row routes by its atlas tag: BLOCK rows into the block list, MODEL rows
        // into the model list, with no cross-contamination. The block-vs-model decision
        // lives upstream in presentation::collect_particles; the bake only routes.
        let particles = vec![
            particle_row(ParticleAtlas::Block),
            particle_row(ParticleAtlas::Model),
            particle_row(ParticleAtlas::Block),
            particle_row(ParticleAtlas::Solid),
        ];
        let mut block_out = Vec::new();
        let mut model_out = Vec::new();
        let mut solid_out = Vec::new();
        bake_particles(&particles, &mut block_out, &mut model_out, &mut solid_out);
        assert_eq!(block_out.len(), 2, "block rows route to the block list");
        assert_eq!(model_out.len(), 1, "model rows route to the model list");
        assert_eq!(solid_out.len(), 1, "solid rows route to the blended list");
        assert_eq!(
            solid_out[0].color, block_out[0].tint,
            "a solid particle's tint IS its color"
        );
    }
}
