//! World-space geometry for dropped item-entities, baked each frame into a
//! reusable dynamic vbuf/ibuf and drawn by the **existing** opaque block pipeline
//! (no new pipeline). Each [`ItemEntityInstance`] becomes either:
//! - a small spinning + bobbing lit cube (`block_model::cube_textured`)
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
//! renderer never reallocates once the per-frame instance count has plateaued.

use glam::{Mat4, Vec3};

use super::item_cube::push_block_item_cube_lit;
use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};
use super::{ItemEntityInstance, ItemEntityPose};
use petramond_mesh::Vertex;
use petramond_world::block::Block;
use petramond_world::item::ItemRenderKind;

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

/// How many layered copies a stack draws: a loose pile spreads up to
/// [`STACK_MAX_LAYERS`] copies (always at least one), an aimed item is one
/// piece whatever its count.
fn layers(inst: &ItemEntityInstance) -> usize {
    match inst.pose {
        ItemEntityPose::Spin(_) => (inst.count.max(1) as usize).min(STACK_MAX_LAYERS),
        ItemEntityPose::Aimed { .. } => 1,
    }
}

/// Where a pose puts an item's model-space geometry (origin-centred, already
/// scaled and pile-offset): a world origin plus the world axes model X/Y/Z
/// land on. One frame for all three render kinds, so a cube, a slab and a
/// bbmodel answer the same pose the same way.
#[derive(Copy, Clone)]
struct Placement {
    origin: Vec3,
    x: Vec3,
    y: Vec3,
    z: Vec3,
}

impl Placement {
    /// A loose stack Y-spins about its hover centre (`BOB_BASE` + bob above
    /// `pos`); an aimed item lies in its heading's basis about the entity's
    /// own centre — pitched as well as yawed, never bobbing.
    fn of(inst: &ItemEntityInstance) -> Self {
        match inst.pose {
            ItemEntityPose::Spin(spin) => {
                let (s, c) = spin.sin_cos();
                Placement {
                    origin: inst.pos + Vec3::new(0.0, BOB_BASE + bob(spin), 0.0),
                    x: Vec3::new(c, 0.0, -s),
                    y: Vec3::Y,
                    z: Vec3::new(s, 0.0, c),
                }
            }
            ItemEntityPose::Aimed { yaw, pitch, .. } => {
                let (forward, up, across) = aim_basis(yaw, pitch);
                Placement {
                    origin: inst.pos,
                    x: forward,
                    y: up,
                    z: across,
                }
            }
        }
    }

    #[inline]
    fn apply(&self, p: Vec3) -> Vec3 {
        self.origin + self.x * p.x + self.y * p.y + self.z * p.z
    }

    fn matrix(&self) -> Mat4 {
        Mat4::from_cols(
            self.x.extend(0.0),
            self.y.extend(0.0),
            self.z.extend(0.0),
            self.origin.extend(1.0),
        )
    }
}

/// Speed (m/s) below which a flying item trails nothing: a lob is easy to
/// follow, a fast one is a streak the eye would otherwise lose.
const TRAIL_SPEED_MIN: f32 = 12.0;
/// The trail covers this many TICKS of travel behind the item — it is the
/// path just flown, so it scales with speed rather than a fixed length.
const TRAIL_TICKS: f32 = 1.5;
/// Longest trail (blocks), and its width at the item (blocks); it tapers
/// to nothing at the tail.
const TRAIL_MAX: f32 = 7.0;
const TRAIL_WIDTH: f32 = 0.07;
/// How dim the tail end of the trail is next to the item.
const TRAIL_TAIL_DIM: f32 = 0.25;

/// How long a trail `speed` earns, in blocks (`0` = none).
fn trail_length(speed: f32) -> f32 {
    if speed < TRAIL_SPEED_MIN {
        return 0.0;
    }
    (speed * TRAIL_TICKS / 20.0).min(TRAIL_MAX)
}

/// The path-just-flown streak behind a fast aimed item of ANY render kind:
/// two crossed tapering ribbons from the item's centre back along its
/// heading, fading toward the tail. Samples the engine's flat white trail
/// tile in the block atlas, so the streak is disturbed air whatever is
/// flying — the light and the taper are the geometry's — and rides the
/// block-atlas [`ItemVertex`] stream whether the item itself is a cube, a
/// slab or a bbmodel. A non-indexed triangle list, indexed sequentially.
/// Nothing for a loose stack or a slow flight.
fn push_flight_trail(
    inst: &ItemEntityInstance,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    let ItemEntityPose::Aimed { speed, .. } = inst.pose else {
        return;
    };
    let length = trail_length(speed);
    if length <= 0.0 {
        return;
    }
    let placement = Placement::of(inst);
    let light = super::lighting::fold_tint([1.0; 3], inst_light(inst), env);
    let [u0, v0, u1, v1] = crate::atlas::tile_uv(petramond_world::tile::engine().item_trail);
    let uv = [(u0 + u1) * 0.5, (v0 + v1) * 0.5];
    let centre = placement.origin;
    let tail = centre - placement.x * length;
    let head_tint = light;
    let tail_tint = light.map(|c| c * TRAIL_TAIL_DIM);
    let vert = |p: Vec3, tint: [f32; 3]| ItemVertex {
        pos: [p.x, p.y, p.z],
        uv,
        shade: 1.0,
        tint,
    };
    let base = verts.len() as u32;
    for side in [placement.y, placement.z] {
        let half = side * (TRAIL_WIDTH * 0.5);
        let (a, b) = (centre - half, centre + half);
        // Both windings, so the ribbon reads from either side.
        for (p, q) in [(a, b), (b, a)] {
            verts.push(vert(p, head_tint));
            verts.push(vert(q, head_tint));
            verts.push(vert(tail, tail_tint));
        }
    }
    indices.extend(base..verts.len() as u32);
}

/// The world basis an aimed item is laid into: model X along the heading,
/// Y up out of that line, Z across.
fn aim_basis(yaw: f32, pitch: f32) -> (Vec3, Vec3, Vec3) {
    let (sp, cp) = pitch.sin_cos();
    let (sy, cy) = yaw.sin_cos();
    let forward = Vec3::new(sy * cp, sp, cy * cp);
    let across = Vec3::new(cy, 0.0, -sy);
    let up = across.cross(forward).normalize_or_zero();
    let up = if up.y < 0.0 { -up } else { up };
    let across = forward.cross(up);
    (forward, up, across)
}

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
        let layers = layers(inst);
        match inst.item.render_kind() {
            ItemRenderKind::BlockCube(Block::Chest) => {
                // A dropped chest spins as its full inset 3D model, not a plain cube.
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_posed_chest(verts, indices, inst, offset);
                }
            }
            ItemRenderKind::BlockCube(block) => {
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_posed_cube(verts, indices, inst, block, offset);
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
/// walls, see `super::item_model::build_extruded_item_lit`) and the slab is
/// posed exactly like a dropped block cube — spinning + bobbing loose, laid
/// along its heading aimed — no camera-facing billboard. `scratch` holds one
/// instance's extrusion in model space before per-layer placement (cleared
/// per instance, capacity reused). Returns the index count. Drawn with the
/// block ATLAS (2D): the wall UVs address single texels.
///
/// This stream also carries the flight trail of EVERY fast aimed instance,
/// whatever its render kind — the trail tile lives in the block atlas, so a
/// flying cube or bbmodel streaks from here too.
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
        push_flight_trail(inst, env, verts, indices);
        let ItemRenderKind::Sprite(tile) = inst.item.render_kind() else {
            continue;
        };
        // One extrusion per instance (light is per-instance, folded into the
        // tint); the layered pile copies just re-place the same model-space mesh.
        let count = super::item_model::build_extruded_stack_lit(
            tile,
            inst.variant,
            inst_light(inst),
            env,
            scratch,
        );
        if count == 0 {
            continue;
        }
        // A sprite has an art axis of its own: aimed, the slab is rolled in
        // its plane so that axis lies along model X — the heading — and it
        // flies point-first. A loose one keeps its upright art.
        let roll = match inst.pose {
            ItemEntityPose::Aimed { .. } => inst.item.sprite_axis_roll(),
            ItemEntityPose::Spin(_) => 0.0,
        };
        let (rs, rc) = roll.sin_cos();
        let placement = Placement::of(inst);
        for &offset in &STACK_LAYER_OFFSETS[..layers(inst)] {
            let base = verts.len() as u32;
            for v in scratch.iter() {
                // Roll + scale the unit slab, offset within the pile, then
                // place — the same order as `place_into_world`, so the pile
                // turns as one body.
                let local = Vec3::new(
                    (v.pos[0] * rc - v.pos[1] * rs) * ITEM_SPRITE_SIZE,
                    (v.pos[0] * rs + v.pos[1] * rc) * ITEM_SPRITE_SIZE,
                    v.pos[2] * ITEM_SPRITE_SIZE,
                ) + offset;
                let p = placement.apply(local);
                verts.push(ItemVertex {
                    pos: [p.x, p.y, p.z],
                    ..*v
                });
            }
            // The extrusion is a non-indexed triangle list; sequential
            // indices let it ride the indexed ItemVertex draw.
            indices.extend(base..base + count);
        }
    }
    indices.len() as u32
}

/// Bake the bbmodel dropped-items into `verts`/`indices` (cleared first, capacity reused)
/// as world-space [`ItemVertex`] geometry sampling the MODEL atlas — the explicit-UV
/// counterpart of [`build_item_entities`], drawn by the model pipeline. Each shows its
/// real baked model, posed like any dropped stack, not a stand-in cube.
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
        let placement = Placement::of(inst).matrix();
        for &offset in &STACK_LAYER_OFFSETS[..layers(inst)] {
            let transform = placement
                * Mat4::from_translation(offset)
                * Mat4::from_scale(Vec3::splat(ITEM_CUBE_SIZE));
            super::item_model::build_block_model_item(
                kind,
                transform,
                inst_light(inst),
                env,
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
    DynLight::new(inst.skylight, inst.blocklight)
}

/// A gentle sinusoidal bob derived from the per-instance spin phase so it needs no
/// separate stored time (spin already advances with `dt` in the App).
#[inline]
fn bob(spin: f32) -> f32 {
    spin.sin() * BOB_AMP
}

/// Append a small posed textured cube for `inst`, centred on its placement
/// plus a model-space `offset` (the pile-layer displacement). The cube is built
/// in model space (centred on origin), offset within the pile, then placed per
/// `inst.pose`. We move the positions of each vertex on the CPU since the
/// opaque pipeline has no per-draw model matrix.
fn push_posed_cube(
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
        false,
    );
    // Instance-data tint (`petramond:tint`): one multiply over the fresh verts.
    super::item_model::dye_block_verts(&mut verts[start..], inst.variant);
    place_into_world(verts, start, inst, offset);
}

/// Like [`push_posed_cube`] but bakes the chest's full inset 3D model (body + lid
/// + latch) instead of a cube, so a dropped chest reads as a tiny chest.
fn push_posed_chest(
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
    place_into_world(verts, start, inst, offset);
}

/// Place the just-appended model-space verts `[start..]` per `inst.pose`
/// (offset within the pile first so layered copies turn coherently). Shared
/// by the dropped cube and chest builders.
fn place_into_world(verts: &mut [Vertex], start: usize, inst: &ItemEntityInstance, offset: Vec3) {
    let placement = Placement::of(inst);
    for v in verts[start..].iter_mut() {
        v.pos = placement.apply(Vec3::from(v.pos) + offset).to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::item::ItemType;

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
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let n = build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(n, 36, "one textured cube = 36 indices");
        // Cube is centred near pos (+ bob base), not at the origin.
        let cx: f32 = v.iter().map(|vert| vert.pos[0]).sum::<f32>() / v.len() as f32;
        assert!((cx - 10.0).abs() < 0.01, "cube centred on pos.x, got {cx}");
    }

    /// An aimed pose is honoured by the cube kind, not just the sprite: the
    /// cube sits on the entity centre (no hover), is PITCHED about it, and
    /// its trail rides the block-atlas sprite stream.
    #[test]
    fn an_aimed_cube_pitches_about_its_centre_and_trails() {
        let pos = Vec3::new(4.0, 70.0, -3.0);
        let inst = ItemEntityInstance {
            pos,
            item: ItemType::Stone,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Aimed {
                yaw: 0.0,
                pitch: std::f32::consts::FRAC_PI_4,
                speed: TRAIL_SPEED_MIN * 2.0,
            },
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);
        assert_eq!(v.len(), 24, "one piece, whatever the count would layer");
        let mean = v.iter().map(|vert| Vec3::from(vert.pos)).sum::<Vec3>() / v.len() as f32;
        assert!(
            mean.distance(pos) < 1e-3,
            "aimed cube centred on the entity, not hovering: {mean} vs {pos}"
        );
        let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
            (lo.min(vert.pos[1]), hi.max(vert.pos[1]))
        });
        let expect = ITEM_CUBE_SIZE * std::f32::consts::SQRT_2;
        assert!(
            (hi - lo - expect).abs() < 1e-3,
            "a cube pitched 45° spans √2 of its side vertically, got {}",
            hi - lo
        );

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
        assert!(n > 0, "a fast aimed cube trails on the sprite stream");
        assert_eq!(n as usize, sv.len());
    }

    #[test]
    fn sprite_item_bakes_an_extruded_slab_not_a_billboard() {
        // Poppy is a cross-plant -> Sprite render kind: it must emit NOTHING on
        // the packed stream and an extruded 3D slab on the ItemVertex stream.
        let inst = ItemEntityInstance {
            pos: Vec3::new(3.0, 10.0, -2.0),
            item: ItemType::Poppy,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(1.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
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
            variant: petramond_world::item::VariantId::NONE,
            count: 3,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
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
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.5),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
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
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: 12,
            blocklight: petramond_world::light::BlockLight6::grey(7),
        };

        build_item_entities(std::slice::from_ref(&inst), &mut v, &mut i);

        for vert in &v {
            assert_eq!(
                (vert.packed >> petramond_mesh::vertex::SKY_SHIFT) & 0x3F,
                12,
                "sky channel in word 1"
            );
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
            variant: petramond_world::item::VariantId::NONE,
            count: 3,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
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
