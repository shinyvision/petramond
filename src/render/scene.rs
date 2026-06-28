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
    ChestInstance, DoorInstance, ItemEntityInstance, MobRenderInstance, ParticleInstance, Renderer,
};
use crate::game::presentation::{
    ChestPresentation, DoorPresentation, DroppedItemPresentation, GamePresentation,
    MobPresentation, ParticleAtlas, ParticlePresentation,
};

/// Per-frame presentation translation state, owned by the App. Holds the renderer's
/// flat instance buffers reused across frames, plus the held-item skylight sampled
/// this frame.
#[derive(Default)]
pub(crate) struct Scene {
    /// Baked dropped-item billboards/cubes for this frame.
    item_entities: Vec<ItemEntityInstance>,
    /// Baked block-atlas particle cubes for this frame.
    particles: Vec<ParticleInstance>,
    /// Baked model-atlas particle cubes (bbmodel-block flecks) for this frame — drawn in
    /// the same pass but bound to the model atlas.
    model_particles: Vec<ParticleInstance>,
    /// Baked placed-chest instances for this frame.
    chests: Vec<ChestInstance>,
    /// Baked placed-door instances for this frame.
    doors: Vec<DoorInstance>,
    /// Baked (interpolated) mob instances for this frame.
    mobs: Vec<MobRenderInstance>,
    /// Combined sky+block light + warm-tint amount for the first-person hand / held
    /// item, sampled at the camera each frame so it brightens AND warms near torches.
    held_item_skylight: u8,
    held_item_warm: u8,
}

impl Scene {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Translate the current presentation snapshot into this scene's reused buffers.
    /// Dropped items' cached skylight is kept fresh by the sim's per-tick light refresh,
    /// so baking just reads it here.
    pub(crate) fn bake(&mut self, presentation: &GamePresentation<'_>) {
        // Items and mobs both simulate on the fixed game tick; `alpha` blends the
        // previous and current tick poses so they move smoothly at any frame rate.
        let alpha = presentation.tick_alpha;
        bake_item_entities(presentation.item_entities, alpha, &mut self.item_entities);
        bake_particles(
            presentation.particles,
            &mut self.particles,
            &mut self.model_particles,
        );
        self.bake_chests(presentation.chests);
        self.bake_doors(presentation.doors);
        bake_mobs(presentation.mobs, alpha, &mut self.mobs);
        (self.held_item_skylight, self.held_item_warm) = presentation.held_item_light;
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
            });
        }
    }

    /// Hand the baked instances + held-item light to the renderer for this frame.
    pub(crate) fn upload(&self, renderer: &mut Renderer) {
        renderer.set_held_item_light(self.held_item_skylight, self.held_item_warm);
        renderer.set_item_entities(&self.item_entities);
        renderer.set_chests(&self.chests);
        renderer.set_doors(&self.doors);
        renderer.set_mobs(&self.mobs);
        renderer.set_particles(&self.particles);
        renderer.set_model_particles(&self.model_particles);
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
        hurt: m.hurt_flash,
        ragdoll: m.ragdoll_pose.clone(),
    }));
}

/// Interpolate from angle `a` to `b` along the shortest arc (radians).
fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = (b - a) % TAU;
    if d > PI {
        d -= TAU;
    } else if d < -PI {
        d += TAU;
    }
    a + d * t
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
) {
    block_out.clear();
    model_out.clear();
    for p in particles {
        let inst = ParticleInstance {
            pos: p.pos,
            uv_min: p.uv_min,
            uv_size: p.uv_size,
            tint: crate::torch::warm_tint(p.tint, p.warm as f32 / 255.0),
            alpha: p.alpha,
            size: p.size,
            skylight: p.skylight,
        };
        match p.atlas {
            ParticleAtlas::Block => block_out.push(inst),
            ParticleAtlas::Model => model_out.push(inst),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::Block;
    use crate::entity::{DroppedItem, ParticleSystem};
    use crate::item::{ItemStack, ItemType};

    #[test]
    fn bake_item_entities_one_instance_per_drop() {
        let drops = vec![
            DroppedItem::new(
                Vec3::new(1.0, 2.0, 3.0),
                ItemStack::new(ItemType::Dirt, 1),
                1,
            ),
            DroppedItem::new(
                Vec3::new(4.0, 5.0, 6.0),
                ItemStack::new(ItemType::Stone, 1),
                2,
            ),
        ];
        let mut out = Vec::new();
        bake_item_entities(&drops, 1.0, &mut out);
        assert_eq!(out.len(), 2);
        // A fresh drop has prev_pos == pos, so any alpha bakes its live position.
        assert_eq!(out[0].pos, drops[0].pos);
        assert_eq!(out[0].item, ItemType::Dirt);
        assert_eq!(out[0].spin, drops[0].spin);
        assert_eq!(out[1].item, ItemType::Stone);
    }

    #[test]
    fn bake_item_entities_interpolates_between_ticks() {
        // A drop that moved last tick (prev_pos != pos) bakes at the blended position,
        // so it renders smoothly between the 20 TPS physics ticks.
        let mut drop = DroppedItem::new(
            Vec3::new(0.0, 64.0, 0.0),
            ItemStack::new(ItemType::Dirt, 1),
            1,
        );
        drop.prev_pos = Vec3::new(0.0, 64.0, 0.0);
        drop.pos = Vec3::new(2.0, 64.0, 0.0);
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
            .map(|i| DroppedItem::new(Vec3::splat(i as f32), ItemStack::new(ItemType::Dirt, 1), i))
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
    fn bake_particles_one_instance_per_alive_particle() {
        let mut particles = ParticleSystem::new();
        particles.spawn_break_burst(IVec3::new(0, 64, 0), Block::Dirt);
        let alive = particles.particles().len();
        assert!(alive > 0);
        let mut out = Vec::new();
        let mut model_out = Vec::new();
        bake_particles(&particles, &mut out, &mut model_out);
        // Dirt is a block-atlas block, so every fleck lands in the block list.
        assert_eq!(out.len(), alive);
        assert!(
            model_out.is_empty(),
            "dirt flecks are block-atlas, not model-atlas"
        );
        let (uv_min, uv_size) = particles.particles()[0].atlas_uv();
        assert_eq!(out[0].uv_min, uv_min);
        assert_eq!(out[0].uv_size, uv_size);
        assert_eq!(out[0].size, particles.particles()[0].size);
    }

    #[test]
    fn bbmodel_block_flecks_route_to_the_model_atlas_list() {
        // A bbmodel block's break flecks must bake into the MODEL list (drawn with the
        // model atlas bound), never the block list — otherwise they'd sample the wrong
        // texture (the crafting-table placeholder bug).
        let mut particles = ParticleSystem::new();
        particles.spawn_break_burst_model(
            IVec3::new(0, 64, 0),
            crate::block_model::BlockModelKind::FurnitureWorkbench,
            crate::render::lighting::FULL_SKYLIGHT,
            0,
        );
        let alive = particles.particles().len();
        assert!(alive > 0);
        let mut out = Vec::new();
        let mut model_out = Vec::new();
        bake_particles(&particles, &mut out, &mut model_out);
        assert_eq!(
            model_out.len(),
            alive,
            "every model fleck routes to the model list"
        );
        assert!(out.is_empty(), "no model fleck leaks into the block list");
    }
}
