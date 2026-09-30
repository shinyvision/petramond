//! Dropped items. Each frame they're rebuilt into a reusable vbuf/ibuf and drawn by the opaque
//! block pipeline, which is why the geometry is in world space.
//!
//! A block item is a spinning cube with its per-face tiles. A sprite item is an extruded slab;
//! [`build_item_sprite_entities`] bakes it into the explicit-UV `ItemVertex` stream, since the
//! packed vertex can't express the single boundary texels its side walls sample.

use glam::{Mat3, Mat4, Vec3};

use super::item_cube::push_block_item_cube_lit;
use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};
use super::{ItemEntityInstance, ItemEntityPose};
use petramond_mesh::Vertex;
use petramond_world::block::Block;
use petramond_world::item::ItemRenderKind;

const ITEM_CUBE_SIZE: f32 = 0.4;
const ITEM_SPRITE_SIZE: f32 = 0.45;
const BOB_AMP: f32 = 0.08;
const BOB_BASE: f32 = 0.25;

const STACK_MAX_LAYERS: usize = 5;

fn layers(inst: &ItemEntityInstance) -> usize {
    match inst.pose {
        ItemEntityPose::Spin(_) => (inst.count.max(1) as usize).min(STACK_MAX_LAYERS),
        ItemEntityPose::Aimed { .. } => 1,
    }
}

#[derive(Copy, Clone)]
struct Placement {
    origin: Vec3,
    x: Vec3,
    y: Vec3,
    z: Vec3,
}

impl Placement {
    fn of(inst: &ItemEntityInstance, render_origin: glam::IVec3) -> Self {
        let pos = inst.pos.relative_to(render_origin);
        match inst.pose {
            ItemEntityPose::Spin(spin) => {
                let (s, c) = spin.sin_cos();
                Placement {
                    origin: pos + Vec3::new(0.0, BOB_BASE + bob(spin), 0.0),
                    x: Vec3::new(c, 0.0, -s),
                    y: Vec3::Y,
                    z: Vec3::new(s, 0.0, c),
                }
            }
            ItemEntityPose::Aimed { yaw, pitch, .. } => {
                let (forward, up, across) = aim_basis(yaw, pitch);
                Placement {
                    origin: pos,
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

const TRAIL_SPEED_MIN: f32 = 12.0;
const TRAIL_TICKS: f32 = 1.5;
const TRAIL_MAX: f32 = 7.0;
const TRAIL_WIDTH: f32 = 0.07;
const TRAIL_TAIL_DIM: f32 = 0.25;

fn trail_length(speed: f32) -> f32 {
    if speed < TRAIL_SPEED_MIN {
        return 0.0;
    }
    (speed * TRAIL_TICKS / 20.0).min(TRAIL_MAX)
}

fn push_flight_trail(
    inst: &ItemEntityInstance,
    render_origin: glam::IVec3,
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
    let placement = Placement::of(inst, render_origin);
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
        for (p, q) in [(a, b), (b, a)] {
            verts.push(vert(p, head_tint));
            verts.push(vert(q, head_tint));
            verts.push(vert(tail, tail_tint));
        }
    }
    indices.extend(base..verts.len() as u32);
}

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

fn sprite_frame(roll: f32, tilt: f32) -> Mat3 {
    Mat3::from_rotation_x(tilt) * Mat3::from_rotation_z(roll)
}

const STACK_LAYER_OFFSETS: [Vec3; STACK_MAX_LAYERS] = [
    Vec3::new(0.00, 0.000, 0.00),
    Vec3::new(0.07, 0.012, 0.05),
    Vec3::new(-0.06, 0.024, 0.04),
    Vec3::new(0.05, 0.036, -0.06),
    Vec3::new(-0.05, 0.048, -0.04),
];

pub fn build_item_entities(
    instances: &[ItemEntityInstance],
    render_origin: glam::IVec3,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        let layers = layers(inst);
        match inst.item.render_kind() {
            ItemRenderKind::BlockCube(block) => {
                for &offset in &STACK_LAYER_OFFSETS[..layers] {
                    push_posed_cube(verts, indices, inst, render_origin, block, offset);
                }
            }
            ItemRenderKind::Sprite(_) => {}
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
    render_origin: glam::IVec3,
    env: LightEnv,
    scratch: &mut Vec<ItemVertex>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for inst in instances {
        push_flight_trail(inst, render_origin, env, verts, indices);
        let ItemRenderKind::Sprite(tile) = inst.item.render_kind() else {
            continue;
        };
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
        let roll = match inst.pose {
            ItemEntityPose::Aimed { spin, .. } => inst.item.sprite_axis_roll() + spin,
            ItemEntityPose::Spin(_) => 0.0,
        };
        let placement = Placement::of(inst, render_origin);
        let tilt = match inst.pose {
            ItemEntityPose::Aimed { .. } => inst.item.projectile().sprite_tilt,
            ItemEntityPose::Spin(_) => 0.0,
        };
        let sprite = sprite_frame(roll, tilt);
        for &offset in &STACK_LAYER_OFFSETS[..layers(inst)] {
            let base = verts.len() as u32;
            for v in scratch.iter() {
                let local = sprite * Vec3::from_array(v.pos) * ITEM_SPRITE_SIZE + offset;
                let p = placement.apply(local);
                verts.push(ItemVertex {
                    pos: [p.x, p.y, p.z],
                    ..*v
                });
            }
            indices.extend(base..base + count);
        }
    }
    indices.len() as u32
}

pub fn build_item_model_entities(
    instances: &[ItemEntityInstance],
    render_origin: glam::IVec3,
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
        let placement = Placement::of(inst, render_origin).matrix();
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

#[inline]
fn bob(spin: f32) -> f32 {
    spin.sin() * BOB_AMP
}

fn push_posed_cube(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &ItemEntityInstance,
    render_origin: glam::IVec3,
    block: Block,
    offset: Vec3,
) {
    let half = ITEM_CUBE_SIZE * 0.5;
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
    super::item_model::dye_block_verts(&mut verts[start..], inst.variant);
    place_into_world(verts, start, inst, render_origin, offset);
}

fn place_into_world(
    verts: &mut [Vertex],
    start: usize,
    inst: &ItemEntityInstance,
    render_origin: glam::IVec3,
    offset: Vec3,
) {
    let placement = Placement::of(inst, render_origin);
    for v in verts[start..].iter_mut() {
        v.pos = placement.apply(Vec3::from(v.pos) + offset).to_array();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::item::ItemType;

    #[test]
    fn a_tilted_sprite_spins_in_the_flight_plane_and_leaves_with_its_ends_behind() {
        use std::f32::consts::{FRAC_PI_2, PI};
        for (yaw, pitch) in [(0.0, 0.0), (1.3, 0.6), (-2.4, -0.8)] {
            let (forward, up, across) = aim_basis(yaw, pitch);
            let place = |p: Vec3| forward * p.x + up * p.y + across * p.z;
            let frame = sprite_frame(-FRAC_PI_2, FRAC_PI_2);
            assert!(
                place(frame * Vec3::Y).dot(forward) > 0.999,
                "apex faces the aim"
            );
            for end in [Vec3::new(-0.5, -0.5, 0.0), Vec3::new(0.5, -0.5, 0.0)] {
                assert!(
                    place(frame * end).dot(forward) < 0.0,
                    "both ends face the throwing hand"
                );
            }
            for spin in [0.0, 0.3, FRAC_PI_2, PI, 5.6] {
                let frame = sprite_frame(-FRAC_PI_2 + spin, FRAC_PI_2);
                for axis in [Vec3::X, Vec3::Y] {
                    assert!(
                        place(frame * axis).dot(up).abs() < 1e-6,
                        "spin stays flat rather than tumbling upright"
                    );
                }
            }
            let upright = sprite_frame(-FRAC_PI_2, 0.0);
            assert!(
                place(upright * Vec3::X).dot(up).abs() > 0.999,
                "zero tilt preserves upright projectiles"
            );
        }
    }

    #[test]
    fn empty_instances_produce_no_geometry() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_item_entities(&[], petramond_math::math::IVec3::ZERO, &mut v, &mut i);
        assert_eq!(n, 0);
        assert!(v.is_empty() && i.is_empty());
    }

    #[test]
    fn block_cube_item_bakes_a_cube() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: WorldPos::new(10.0, 64.0, -5.0),
            item: ItemType::Stone,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let n = build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(n, 36, "one textured cube = 36 indices");
        let cx: f32 = v.iter().map(|vert| vert.pos[0]).sum::<f32>() / v.len() as f32;
        assert!((cx - 10.0).abs() < 0.01, "cube centred on pos.x, got {cx}");
    }

    #[test]
    fn an_aimed_cube_pitches_about_its_centre_and_trails() {
        let pos = WorldPos::new(4.0, 70.0, -3.0);
        let inst = ItemEntityInstance {
            pos,
            item: ItemType::Stone,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Aimed {
                yaw: 0.0,
                pitch: std::f32::consts::FRAC_PI_4,
                speed: TRAIL_SPEED_MIN * 2.0,
                spin: 0.0,
            },
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24, "one piece, whatever the count would layer");
        let mean = v.iter().map(|vert| Vec3::from(vert.pos)).sum::<Vec3>() / v.len() as f32;
        assert!(
            mean.distance(pos.relative_to(glam::IVec3::ZERO)) < 1e-3,
            "aimed cube centred on the entity, not hovering: {mean} vs {pos:?}"
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
            petramond_math::math::IVec3::ZERO,
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
        let inst = ItemEntityInstance {
            pos: WorldPos::new(3.0, 10.0, -2.0),
            item: ItemType::Poppy,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(1.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(n, 0, "sprites no longer bake on the packed stream");

        let mut scratch = Vec::new();
        let mut sv = Vec::new();
        let mut si = Vec::new();
        let n = build_item_sprite_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            LightEnv::IDENTITY,
            &mut scratch,
            &mut sv,
            &mut si,
        );
        assert!(n > 12, "expected extruded front+back+walls, got {n}");
        assert_eq!(n as usize, sv.len());
        assert_eq!(n as usize, si.len());
        let (min_x, max_x) = sv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
            (lo.min(vert.pos[0]), hi.max(vert.pos[0]))
        });
        let cx = (min_x + max_x) * 0.5;
        assert!((cx - 3.0).abs() < 0.01, "slab centred on pos.x, got {cx}");
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
            pos: WorldPos::ZERO,
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
            petramond_math::math::IVec3::ZERO,
            LightEnv::IDENTITY,
            &mut scratch,
            &mut sv,
            &mut si,
        );
        let per_layer = scratch.len();
        assert!(per_layer > 12);
        assert_eq!(sv.len(), per_layer * 3, "3-stack = 3 layered slabs");

        let huge = ItemEntityInstance { count: 64, ..inst };
        build_item_sprite_entities(
            std::slice::from_ref(&huge),
            petramond_math::math::IVec3::ZERO,
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
            pos: WorldPos::ZERO,
            item: ItemType::Dirt,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.5),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        let (cap_v, cap_i) = (v.capacity(), i.capacity());
        build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24, "one textured cube = 24 verts");
        assert_eq!(v.capacity(), cap_v, "vert buffer reused");
        assert_eq!(i.capacity(), cap_i, "index buffer reused");
    }

    #[test]
    fn item_entity_packs_instance_skylight() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let inst = ItemEntityInstance {
            pos: WorldPos::ZERO,
            item: ItemType::Stone,
            variant: petramond_world::item::VariantId::NONE,
            count: 1,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: 12,
            blocklight: petramond_world::light::BlockLight6::grey(7),
        };

        build_item_entities(
            std::slice::from_ref(&inst),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );

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
        let three = ItemEntityInstance {
            pos: WorldPos::new(2.0, 5.0, 2.0),
            item: ItemType::Stone,
            variant: petramond_world::item::VariantId::NONE,
            count: 3,
            pose: crate::ItemEntityPose::Spin(0.0),
            skylight: super::super::lighting::FULL_SKYLIGHT,
            blocklight: petramond_world::light::BlockLight6::DARK,
        };
        let n = build_item_entities(
            std::slice::from_ref(&three),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24 * 3, "3-stack = 3 layered cubes");
        assert_eq!(n, 36 * 3);

        let huge = ItemEntityInstance { count: 64, ..three };
        build_item_entities(
            std::slice::from_ref(&huge),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24 * 5, "count capped at 5 layers");

        let zero = ItemEntityInstance { count: 0, ..three };
        build_item_entities(
            std::slice::from_ref(&zero),
            petramond_math::math::IVec3::ZERO,
            &mut v,
            &mut i,
        );
        assert_eq!(v.len(), 24, "count 0 still draws one layer");
    }
}
