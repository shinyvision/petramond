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

use glam::{Mat4, Vec3};

use super::block_model::{block_icon_faces, push_cube_faces_lit, BillboardBasis};
use super::item_model::ItemVertex;
use super::ItemEntityInstance;
use crate::block::Block;
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

/// Most geometries a dropped stack ever bakes, no matter how big the count: a
/// 64-stack still draws only 5 layered copies (a bigger pile reads the same).
const STACK_MAX_LAYERS: usize = 5;

/// Per-layer model-space offsets (metres) for a layered stack, applied BEFORE the
/// Y-spin so the little pile rotates as one body. A tight clustered scatter
/// (mostly horizontal, a slight rise) so the copies read as a heap, not a tower.
const STACK_LAYER_OFFSETS: [Vec3; STACK_MAX_LAYERS] = [
    Vec3::new(0.00, 0.000, 0.00),
    Vec3::new(0.07, 0.012, 0.05),
    Vec3::new(-0.06, 0.024, 0.04),
    Vec3::new(0.05, 0.036, -0.06),
    Vec3::new(-0.05, 0.048, -0.04),
];

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
        // A stack draws several offset copies so a pile reads as loot; capped so a
        // big count never bakes a wall of geometry. Always at least one layer.
        let layers = (inst.count.max(1) as usize).min(STACK_MAX_LAYERS);
        match inst.item.render_kind() {
            ItemRenderKind::BlockCube(Block::Chest) => {
                // A dropped chest spins as its full inset 3D model, not a plain cube.
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_spinning_chest(verts, indices, inst, offset);
                }
            }
            ItemRenderKind::BlockCube(block) => {
                let faces = block_icon_faces(block);
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_spinning_cube(verts, indices, inst, faces, offset);
                }
            }
            ItemRenderKind::Sprite(tile) => {
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    let center = inst.pos
                        + Vec3::new(offset.x, BOB_BASE + bob(inst.spin) + offset.y, offset.z);
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
            // bbmodel items ride the explicit-UV model stream (own atlas), baked by
            // `build_item_model_entities` and drawn by the model pipeline — skip here.
            ItemRenderKind::Model(_) => {}
        }
    }
    indices.len() as u32
}

/// Bake the bbmodel dropped-items into `verts`/`indices` (cleared first, capacity reused)
/// as world-space [`ItemVertex`] geometry sampling the MODEL atlas — the explicit-UV
/// counterpart of [`build_item_entities`], drawn by the model pipeline. Each shows its
/// real baked model (spinning + bobbing like any dropped stack), not a stand-in cube.
pub fn build_item_model_entities(
    instances: &[ItemEntityInstance],
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        let ItemRenderKind::Model(kind) = inst.item.render_kind() else {
            continue;
        };
        let layers = (inst.count.max(1) as usize).min(STACK_MAX_LAYERS);
        for &offset in &STACK_LAYER_OFFSETS[..layers] {
            let center =
                inst.pos + Vec3::new(offset.x, BOB_BASE + bob(inst.spin) + offset.y, offset.z);
            let transform = Mat4::from_translation(center)
                * Mat4::from_rotation_y(inst.spin)
                * Mat4::from_scale(Vec3::splat(ITEM_CUBE_SIZE));
            super::item_model::build_block_model_item(
                kind,
                transform,
                inst.skylight,
                0,
                None,
                verts,
                indices,
            );
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

/// Append a small Y-spun, bobbing textured cube for `inst`, centred on its `pos`
/// plus a model-space `offset` (the pile-layer displacement). The cube is built
/// in model space (centred on origin), offset within the pile, rotated about Y by
/// `inst.spin`, then translated into the world. We rotate the positions of each
/// vertex on the CPU since the opaque pipeline has no per-draw model matrix.
fn push_spinning_cube(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &ItemEntityInstance,
    faces: [crate::atlas::Tile; 6],
    offset: Vec3,
) {
    let half = ITEM_CUBE_SIZE * 0.5;
    // Append the cube centred on the origin (model space) directly into the
    // caller's buffers (no temporary Vec), then spin + place it in the world.
    let start = verts.len();
    push_cube_faces_lit(
        verts,
        indices,
        faces,
        Vec3::splat(-half),
        ITEM_CUBE_SIZE,
        inst.skylight,
    );
    spin_into_world(verts, start, inst, offset);
}

/// Like [`push_spinning_cube`] but bakes the chest's full inset 3D model (body + lid
/// + latch) instead of a cube, so a dropped chest reads as a tiny chest.
fn push_spinning_chest(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &ItemEntityInstance,
    offset: Vec3,
) {
    let half = ITEM_CUBE_SIZE * 0.5;
    let start = verts.len();
    super::chest_model::push_chest_item(
        verts,
        indices,
        Vec3::splat(-half),
        ITEM_CUBE_SIZE,
        inst.skylight,
    );
    spin_into_world(verts, start, inst, offset);
}

/// Rotate the just-appended verts `[start..]` about Y by `inst.spin` (offset within
/// the pile first so layered copies spin coherently) and translate them to the
/// world bob centre. Shared by the dropped cube and chest builders.
fn spin_into_world(verts: &mut [Vertex], start: usize, inst: &ItemEntityInstance, offset: Vec3) {
    let (s, c) = inst.spin.sin_cos();
    let center = inst.pos + Vec3::new(0.0, BOB_BASE + bob(inst.spin), 0.0);
    for v in verts[start..].iter_mut() {
        let [x, y, z] = v.pos;
        let (lx, ly, lz) = (x + offset.x, y + offset.y, z + offset.z);
        let rx = lx * c + lz * s;
        let rz = -lx * s + lz * c;
        v.pos = [center.x + rx, center.y + ly, center.z + rz];
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
            count: 1,
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
            count: 1,
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
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Dirt,
            count: 1,
            spin: 0.5,
            skylight: super::super::lighting::FULL_SKYLIGHT,
        };
        build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);
        let (cap_v, cap_i) = (v.capacity(), i.capacity());
        // Same input -> identical vert/index count, so the cleared+refilled
        // buffers keep their capacity: rebuilding to the same size never reallocs.
        build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(v.capacity(), cap_v, "vert buffer reused");
        assert_eq!(i.capacity(), cap_i, "index buffer reused");
    }

    #[test]
    fn item_entity_packs_instance_skylight() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Stone,
            count: 1,
            spin: 0.0,
            skylight: 12,
        };

        build_item_entities(std::slice::from_ref(&inst), basis(), &mut v, &mut i);

        for vert in &v {
            assert_eq!((vert.packed >> 23) & 0x3F, 12);
        }
    }

    #[test]
    fn stack_count_bakes_layered_copies_capped_at_five() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        // A 3-stack cube bakes 3 layered cubes = 72 verts / 108 indices.
        let three = ItemEntityInstance {
            pos: Vec3::new(2.0, 5.0, 2.0),
            item: ItemType::Stone,
            count: 3,
            spin: 0.0,
            skylight: super::super::lighting::FULL_SKYLIGHT,
        };
        let n = build_item_entities(std::slice::from_ref(&three), basis(), &mut v, &mut i);
        assert_eq!(v.len(), 24 * 3, "3-stack = 3 layered cubes");
        assert_eq!(n, 36 * 3);

        // A huge count is capped at 5 layered copies, not 64.
        let huge = ItemEntityInstance { count: 64, ..three };
        build_item_entities(std::slice::from_ref(&huge), basis(), &mut v, &mut i);
        assert_eq!(v.len(), 24 * 5, "count capped at 5 layers");

        // count 0 is treated as a single layer (never zero geometry).
        let zero = ItemEntityInstance { count: 0, ..three };
        build_item_entities(std::slice::from_ref(&zero), basis(), &mut v, &mut i);
        assert_eq!(v.len(), 24, "count 0 still draws one layer");
    }
}
