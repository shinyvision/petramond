//! Render-side scene adapter: bakes the simulation's per-frame world-render data
//! into the renderer's flat wire structs.
//!
//! [`Scene`] is the translation layer the App owns between [`Game`](crate::game::Game)
//! (the simulation) and the [`Renderer`]. Each frame the App calls [`Scene::bake`]
//! with the read-only `Game`, then [`Scene::upload`] to hand the baked instances to
//! the renderer. Keeping the wire structs (`ItemEntityInstance` / `ParticleInstance`
//! / `ChestInstance`) and their reused scratch buffers here keeps the renderer's
//! presentation types out of `Game`.
//!
//! The instances are baked from DOMAIN accessors on `Game` (the dropped items, the
//! loaded chests' render data, the particle system, the chest-lid angles, the held
//! item's skylight) — never from `World`/`ParticleSystem` internals. The buffers are
//! cleared + refilled (capacity reused) so a bounded per-frame count never reallocs.

use glam::{IVec3, Vec3};

use super::{ChestInstance, ItemEntityInstance, ParticleInstance, Renderer};
use crate::furnace::Facing;
use crate::game::Game;

/// Per-frame world-render translation state, owned by the App. Holds the renderer's
/// flat instance buffers (reused across frames) and the scratch buffer for gathering
/// chest render data, plus the held-item skylight sampled this frame.
#[derive(Default)]
pub struct Scene {
    /// Baked dropped-item billboards/cubes for this frame.
    item_entities: Vec<ItemEntityInstance>,
    /// Baked particle billboards for this frame.
    particles: Vec<ParticleInstance>,
    /// Baked placed-chest instances for this frame.
    chests: Vec<ChestInstance>,
    /// Reusable scratch for gathering chest render data (world pos, facing, skylight)
    /// from the loaded chunks; the lid angle is paired in from `Game::chest_lid_angle`.
    chest_scratch: Vec<(IVec3, Facing, u8)>,
    /// World skylight to apply to the first-person hand / held item, sampled at the
    /// camera each frame.
    held_item_skylight: u8,
}

impl Scene {
    pub fn new() -> Self {
        Self::default()
    }

    /// Translate the sim's current world-render data into this scene's reused
    /// buffers. Read-only over `Game`: the dropped items' cached skylight is kept
    /// fresh by the sim's per-tick light refresh, so baking just reads it here.
    pub fn bake(&mut self, game: &Game) {
        bake_item_entities(game.item_entities(), &mut self.item_entities);
        bake_particles(game.particles(), &mut self.particles);
        self.bake_chests(game);
        self.held_item_skylight = game.held_item_skylight();
    }

    /// The placed chests to draw this frame (world pos, facing, lid angle, skylight),
    /// gathered from the loaded chunks. The linear open progress is smoothstepped so
    /// the lid accelerates and decelerates instead of swinging at a constant rate.
    fn bake_chests(&mut self, game: &Game) {
        game.collect_chest_render_data(&mut self.chest_scratch);
        self.chests.clear();
        for &(pos, facing, skylight) in &self.chest_scratch {
            let raw = game.chest_lid_angle(pos);
            let lid01 = raw * raw * (3.0 - 2.0 * raw);
            self.chests.push(ChestInstance {
                pos: Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32),
                facing,
                lid01,
                skylight,
            });
        }
    }

    /// Hand the baked instances + held-item light to the renderer for this frame.
    pub fn upload(&self, renderer: &mut Renderer) {
        renderer.set_held_item_light(self.held_item_skylight);
        renderer.set_item_entities(&self.item_entities);
        renderer.set_chests(&self.chests);
        renderer.set_particles(&self.particles);
    }
}

/// Map each dropped item to one [`ItemEntityInstance`] (cleared + refilled, capacity
/// reused). The skylight rides through from the item's cached value.
fn bake_item_entities(items: &[crate::entity::DroppedItem], out: &mut Vec<ItemEntityInstance>) {
    out.clear();
    out.extend(items.iter().map(|d| ItemEntityInstance {
        pos: d.pos,
        item: d.stack.item,
        count: d.stack.count,
        spin: d.spin,
        skylight: d.skylight,
    }));
}

/// Map each alive particle to one [`ParticleInstance`] (cleared + refilled, capacity
/// reused), resolving its atlas patch, alpha, and render size.
fn bake_particles(particles: &crate::entity::ParticleSystem, out: &mut Vec<ParticleInstance>) {
    out.clear();
    out.extend(particles.particles().iter().map(|p| {
        let (uv_min, uv_size) = p.atlas_uv();
        ParticleInstance {
            pos: p.pos,
            uv_min,
            uv_size,
            tint: p.tint,
            alpha: p.alpha(),
            size: p.render_size(),
            skylight: p.skylight,
        }
    }));
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
            DroppedItem::new(Vec3::new(1.0, 2.0, 3.0), ItemStack::new(ItemType::Dirt, 1), 1),
            DroppedItem::new(Vec3::new(4.0, 5.0, 6.0), ItemStack::new(ItemType::Stone, 1), 2),
        ];
        let mut out = Vec::new();
        bake_item_entities(&drops, &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].pos, drops[0].pos);
        assert_eq!(out[0].item, ItemType::Dirt);
        assert_eq!(out[0].spin, drops[0].spin);
        assert_eq!(out[1].item, ItemType::Stone);
    }

    #[test]
    fn bake_item_entities_reuses_the_vec_without_growth() {
        let drops: Vec<_> = (0..8)
            .map(|i| DroppedItem::new(Vec3::splat(i as f32), ItemStack::new(ItemType::Dirt, 1), i))
            .collect();
        let mut out = Vec::new();
        bake_item_entities(&drops, &mut out);
        let cap = out.capacity();
        // Fewer drops -> identical-or-smaller count, so the cleared+refilled buffer
        // keeps its capacity: rebuilding never reallocs.
        bake_item_entities(&drops[..2], &mut out);
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
        bake_particles(&particles, &mut out);
        assert_eq!(out.len(), alive);
        let (uv_min, uv_size) = particles.particles()[0].atlas_uv();
        assert_eq!(out[0].uv_min, uv_min);
        assert_eq!(out[0].uv_size, uv_size);
        assert_eq!(out[0].size, particles.particles()[0].size);
    }
}
