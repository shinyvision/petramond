//! World-space geometry for dropped item-entities, baked each frame into a
//! reusable dynamic vbuf/ibuf and drawn by the **existing** opaque block pipeline
//! (no new pipeline). Each [`ItemEntityInstance`] becomes either:
//! - a small spinning + bobbing lit cube ([`block_model::cube_textured`])
//!   for `BlockCube` items (logs etc. keep their per-face tiles), or
//! - a spinning extruded pixel-perfect 3D slab for `Sprite` items (flowers /
//!   tools), baked by [`build_item_sprite_entities`] into the explicit-UV
//!   `ItemVertex` stream (block atlas) since its side walls sample single
//!   boundary texels the packed vertex cannot express.
//!
//! Geometry is built in WORLD space because it rides the opaque pipeline whose
//! vertex shader (`block.wgsl::vs_main`) transforms `pos` by `view_proj`. Verts
//! carry the instance skylight sampled from the world plus full AO.
//!
//! The builder appends into caller-owned `Vec`s (cleared, capacity reused) so the
//! renderer never reallocates when the per-frame instance count stays bounded.

use glam::{Mat4, Vec3};

use super::item_cube::push_block_item_cube_lit;
use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};
use super::ItemEntityInstance;
use crate::block::Block;
use crate::item::ItemRenderKind;
use crate::mesh::Vertex;

/// Side length (metres) of a dropped block-cube. Small so items read as loot, not
/// world blocks.
const ITEM_CUBE_SIZE: f32 = 0.4;
/// Side length (metres) of a dropped extruded sprite (flowers etc.).
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
/// Returns the number of indices written. Caller is responsible for frustum-culling
/// instances before calling (so culled items cost nothing here).
pub fn build_item_entities(
    instances: &[ItemEntityInstance],
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
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_spinning_cube(verts, indices, inst, block, offset);
                }
            }
            // Sprite items ride the explicit-UV block-atlas stream, baked by
            // `build_item_sprite_entities` (extruded 3D slabs) — skip here.
            ItemRenderKind::Sprite(_) => {}
            // bbmodel items ride the explicit-UV model stream (own atlas), baked by
            // `build_item_model_entities` and drawn by the model pipeline — skip here.
            ItemRenderKind::Model(_) => {}
        }
    }
    indices.len() as u32
}

/// Bake the sprite-kind dropped items as EXTRUDED, pixel-perfect 3D slabs into
/// `verts`/`indices` (cleared first, capacity reused): the sprite's alpha mask
/// gains one texel of depth (front + back faces plus per-texel boundary side
/// walls, see [`super::item_model::build_extruded_item_lit`]) and the slab
/// spins and bobs about Y exactly like a dropped block cube — no camera-facing
/// billboard. `scratch` holds one instance's extrusion in model space before
/// per-layer placement (cleared per instance, capacity reused). Returns the
/// index count. Drawn with the block ATLAS (2D): the wall UVs address single
/// texels.
pub fn build_item_sprite_entities(
    instances: &[ItemEntityInstance],
    env: LightEnv,
    scratch: &mut Vec<ItemVertex>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        let ItemRenderKind::Sprite(tile) = inst.item.render_kind() else {
            continue;
        };
        // One extrusion per instance (light is per-instance, folded into the
        // tint); the layered pile copies just re-place the same model-space mesh.
        let count =
            super::item_model::build_extruded_item_lit(tile, inst_light(inst), env, scratch);
        if count == 0 {
            continue;
        }
        let layers = (inst.count.max(1) as usize).min(STACK_MAX_LAYERS);
        let (s, c) = inst.spin.sin_cos();
        let bob_y = BOB_BASE + bob(inst.spin);
        for &offset in &STACK_LAYER_OFFSETS[..layers] {
            let base = verts.len() as u32;
            for v in scratch.iter() {
                // Scale the unit slab, offset within the pile, then Y-spin +
                // translate — the same order as `spin_into_world` so the pile
                // rotates as one body.
                let lx = v.pos[0] * ITEM_SPRITE_SIZE + offset.x;
                let ly = v.pos[1] * ITEM_SPRITE_SIZE + offset.y;
                let lz = v.pos[2] * ITEM_SPRITE_SIZE + offset.z;
                let rx = lx * c + lz * s;
                let rz = -lx * s + lz * c;
                verts.push(ItemVertex {
                    pos: [inst.pos.x + rx, inst.pos.y + bob_y + ly, inst.pos.z + rz],
                    ..*v
                });
            }
            // The extrusion is a non-indexed triangle list; sequential indices
            // let it ride the indexed ItemVertex draw.
            indices.extend(base..base + count);
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
    env: LightEnv,
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
                inst_light(inst),
                env,
                0,
                None,
                verts,
                indices,
            );
        }
    }
    indices.len() as u32
}

#[inline]
fn inst_light(inst: &ItemEntityInstance) -> DynLight {
    DynLight {
        sky: inst.skylight,
        block: inst.blocklight,
    }
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
    block: Block,
    offset: Vec3,
) {
    let half = ITEM_CUBE_SIZE * 0.5;
    // Append the cube centred on the origin (model space) directly into the
    // caller's buffers (no temporary Vec), then spin + place it in the world.
    let start = verts.len();
    push_block_item_cube_lit(
        verts,
        indices,
        block,
        Vec3::splat(-half),
        ITEM_CUBE_SIZE,
        inst_light(inst),
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
        inst_light(inst),
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

    #[test]
    fn empty_instances_produce_no_geometry() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_item_entities(&[], &mut v, &mut i);
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
            blocklight: 0,
        };
        let n = build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(n, 36, "one textured cube = 36 indices");
        // Cube is centred near pos (+ bob base), not at the origin.
        let cx: f32 = v.iter().map(|vert| vert.pos[0]).sum::<f32>() / v.len() as f32;
        assert!((cx - 10.0).abs() < 0.01, "cube centred on pos.x, got {cx}");
    }

    #[test]
    fn sprite_item_bakes_an_extruded_slab_not_a_billboard() {
        // Poppy is a cross-plant -> Sprite render kind: it must emit NOTHING on
        // the packed stream and an extruded 3D slab on the ItemVertex stream.
        let inst = ItemEntityInstance {
            pos: Vec3::new(3.0, 10.0, -2.0),
            item: ItemType::Poppy,
            count: 1,
            spin: 1.0,
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: 0,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
        assert_eq!(n, 0, "sprites no longer bake on the packed stream");

        let mut scratch = Vec::new();
        let mut sv = Vec::new();
        let mut si = Vec::new();
        let n = build_item_sprite_entities(
            std::slice::from_ref(&inst),
            LightEnv::IDENTITY,
            &mut scratch,
            &mut sv,
            &mut si,
        );
        // Front + back faces are 12 verts; a real flower silhouette adds side
        // walls on top. Sequential indices (non-indexed list riding the draw).
        assert!(n > 12, "expected extruded front+back+walls, got {n}");
        assert_eq!(n as usize, sv.len());
        assert_eq!(n as usize, si.len());
        // The slab is placed at the instance position (plus bob), not the origin.
        // Bounds midpoint, not vertex mean: wall quads cluster on the silhouette.
        let (min_x, max_x) = sv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
            (lo.min(vert.pos[0]), hi.max(vert.pos[0]))
        });
        let cx = (min_x + max_x) * 0.5;
        assert!((cx - 3.0).abs() < 0.01, "slab centred on pos.x, got {cx}");
        // Spun about Y (spin = 1.0), the flat sprite gains real Z extent.
        let (min_z, max_z) = sv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
            (lo.min(vert.pos[2]), hi.max(vert.pos[2]))
        });
        assert!(
            max_z - min_z > 0.1,
            "spun slab spans Z, got {}",
            max_z - min_z
        );
    }

    #[test]
    fn sprite_stack_bakes_layered_copies() {
        let inst = ItemEntityInstance {
            pos: Vec3::ZERO,
            item: ItemType::Poppy,
            count: 3,
            spin: 0.0,
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: 0,
        };
        let mut scratch = Vec::new();
        let mut sv = Vec::new();
        let mut si = Vec::new();
        build_item_sprite_entities(
            std::slice::from_ref(&inst),
            LightEnv::IDENTITY,
            &mut scratch,
            &mut sv,
            &mut si,
        );
        let per_layer = scratch.len();
        assert!(per_layer > 12);
        assert_eq!(sv.len(), per_layer * 3, "3-stack = 3 layered slabs");

        // A huge count is capped at 5 layered copies, not 64.
        let huge = ItemEntityInstance { count: 64, ..inst };
        build_item_sprite_entities(
            std::slice::from_ref(&huge),
            LightEnv::IDENTITY,
            &mut scratch,
            &mut sv,
            &mut si,
        );
        assert_eq!(sv.len(), per_layer * 5, "count capped at 5 layers");
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
            blocklight: 0,
        };
        build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
        let (cap_v, cap_i) = (v.capacity(), i.capacity());
        // Same input -> identical vert/index count, so the cleared+refilled
        // buffers keep their capacity: rebuilding to the same size never reallocs.
        build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
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
            blocklight: 7,
        };

        build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);

        for vert in &v {
            assert_eq!((vert.packed >> 23) & 0x3F, 12, "sky channel in word 1");
            assert_eq!(vert.packed2 & 0x3F, 7, "block channel in word 2");
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
            blocklight: 0,
        };
        let n = build_item_entities(std::slice::from_ref(&three), &mut v, &mut i);
        assert_eq!(v.len(), 24 * 3, "3-stack = 3 layered cubes");
        assert_eq!(n, 36 * 3);

        // A huge count is capped at 5 layered copies, not 64.
        let huge = ItemEntityInstance { count: 64, ..three };
        build_item_entities(std::slice::from_ref(&huge), &mut v, &mut i);
        assert_eq!(v.len(), 24 * 5, "count capped at 5 layers");

        // count 0 is treated as a single layer (never zero geometry).
        let zero = ItemEntityInstance { count: 0, ..three };
        build_item_entities(std::slice::from_ref(&zero), &mut v, &mut i);
        assert_eq!(v.len(), 24, "count 0 still draws one layer");
    }
}
