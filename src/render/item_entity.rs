//! World-space geometry for dropped item-entities, baked each frame into a
//! reusable dynamic vbuf/ibuf and drawn by the **existing** opaque block pipeline
//! (no new pipeline). Each [`ItemEntityInstance`] becomes either:
//! - a small spinning + bobbing lit cube ([`block_model::cube_textured`])
//!   for `BlockCube` items (logs etc. keep their per-face tiles), or
//! - a small camera-facing billboard ([`billboard quad in world space`]) for
//!   `Sprite` items (flowers / cross-plants).
//!
//! Geometry is built in WORLD space because it rides the opaque pipeline whose
//! vertex shader (`block.wgsl::vs_main`) transforms `pos` by `view_proj`. Verts
//! carry the instance skylight sampled from the world plus full AO.
//!
//! The builder appends into caller-owned `Vec`s (cleared, capacity reused) so the
//! renderer never reallocates when the per-frame instance count stays bounded.

use glam::Vec3;

use super::block_model::{push_cube_textured_lit, BillboardBasis};
use super::ItemEntityInstance;
use crate::item::ItemRenderKind;
use crate::mesh::Vertex;

/// Side length (metres) of a dropped block-cube. Small so items read as loot, not
/// world blocks.
const ITEM_CUBE_SIZE: f32 = 0.4;
/// Side length (metres) of a dropped sprite billboard (flowers etc.).
const ITEM_SPRITE_SIZE: f32 = 0.45;
/// Vertical bob amplitude (metres) — a gentle hover.
const BOB_AMP: f32 = 0.08;
/// Centre height (metres) the item floats above its `pos`, before bob.
const BOB_BASE: f32 = 0.25;

/// Bake all `instances` into `verts` / `indices` (cleared first, capacity reused).
/// `basis` supplies the camera right/up vectors for sprite billboards. Returns the
/// number of indices written. Caller is responsible for frustum-culling instances
/// before calling (so culled items cost nothing here).
pub fn build_item_entities(
    instances: &[ItemEntityInstance],
    basis: BillboardBasis,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        match inst.item.render_kind() {
            ItemRenderKind::BlockCube(block) => {
                push_spinning_cube(verts, indices, inst, block.tiles());
            }
            ItemRenderKind::Sprite(tile) => {
                let center = inst.pos + Vec3::new(0.0, BOB_BASE + bob(inst.spin), 0.0);
                super::block_model::push_billboard_world_lit(
                    verts,
                    indices,
                    tile,
                    center,
                    ITEM_SPRITE_SIZE,
                    basis,
                    inst.skylight,
                );
            }
        }
    }
    indices.len() as u32
}

/// A gentle sinusoidal bob derived from the per-instance spin phase so it needs no
/// separate stored time (spin already advances with `dt` in the App).
#[inline]
fn bob(spin: f32) -> f32 {
    spin.sin() * BOB_AMP
}

/// Append a small Y-spun, bobbing textured cube for `inst`, centred on its `pos`.
/// The cube is built in model space (centred on origin), rotated about Y by
/// `inst.spin`, then translated into the world. We rotate the four positions of
/// each vertex on the CPU since the opaque pipeline has no per-draw model matrix.
fn push_spinning_cube(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &ItemEntityInstance,
    tiles: [crate::atlas::Tile; 3],
) {
    let half = ITEM_CUBE_SIZE * 0.5;
    // Append the cube centred on the origin (model space) directly into the
    // caller's buffers (no temporary Vec), then rotate the just-appended verts in
    // place about Y and translate them to the world centre.
    let start = verts.len();
    push_cube_textured_lit(
        verts,
        indices,
        tiles,
        Vec3::splat(-half),
        ITEM_CUBE_SIZE,
        inst.skylight,
    );
    let (s, c) = inst.spin.sin_cos();
    let center = inst.pos + Vec3::new(0.0, BOB_BASE + bob(inst.spin), 0.0);
    for v in verts[start..].iter_mut() {
        let [x, y, z] = v.pos;
        // Rotate about Y, then translate to the world centre.
        let rx = x * c + z * s;
        let rz = -x * s + z * c;
        v.pos = [center.x + rx, center.y + y, center.z + rz];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::ItemType;

    fn basis() -> BillboardBasis {
        BillboardBasis {
            right: Vec3::X,
            up: Vec3::Y,
        }
    }

    #[test]
    fn empty_instances_produce_no_geometry() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_item_entities(&[], basis(), &mut v, &mut i);
        assert_eq!(n, 0);
        assert!(v.is_empty() && i.is_empty());
    }

    #[test]
    fn block_cube_item_bakes_a_cube() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: Vec3::new(10.0, 64.0, -5.0),
            item: ItemType::Stone,
            spin: 0.0,
            skylight: super::super::lighting::FULL_SKYLIGHT,
        };
        let n = build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(n, 36, "one textured cube = 36 indices");
        // Cube is centred near pos (+ bob base), not at the origin.
        let cx: f32 = v.iter().map(|vert| vert.pos[0]).sum::<f32>() / v.len() as f32;
        assert!((cx - 10.0).abs() < 0.01, "cube centred on pos.x, got {cx}");
    }

    #[test]
    fn sprite_item_bakes_a_double_sided_billboard() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        // Poppy is a cross-plant -> Sprite render kind.
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Poppy,
            spin: 1.0,
            skylight: super::super::lighting::FULL_SKYLIGHT,
        };
        let n = build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);
        // Camera-facing billboard, emitted double-sided (two windings) so a basis
        // sign change can't cull the sprite: 8 verts / 12 indices.
        assert_eq!(v.len(), 8);
        assert_eq!(n, 12);
    }

    #[test]
    fn reuses_buffers_across_calls() {
        let mut v = Vec::with_capacity(64);
        let mut i = Vec::with_capacity(64);
        let cap_v = v.capacity();
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Dirt,
            spin: 0.5,
            skylight: super::super::lighting::FULL_SKYLIGHT,
        };
        build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);
        // Second call clears + refills; capacity is retained (no shrink/regrow).
        build_item_entities(&[], basis(), &mut v, &mut i);
        assert!(v.is_empty());
        assert!(v.capacity() >= cap_v);
    }

    #[test]
    fn item_entity_packs_instance_skylight() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Stone,
            spin: 0.0,
            skylight: 12,
        };

        build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);

        for vert in &v {
            assert_eq!((vert.packed >> 23) & 0x3F, 12);
        }
    }
}
