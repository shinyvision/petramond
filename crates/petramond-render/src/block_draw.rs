//! Baking mod-submitted per-block DRAW SETS (`world::draw`) into the
//! item-entity opaque stream.
//!
//! They ride that stream on purpose: it is already the block atlas, already
//! double-sided, already CPU-lit per instance, and already rebuilt from
//! scratch every frame. That last part is the whole point of the surface — a
//! mod may replace a block's drawing every tick, and nothing here re-meshes a
//! chunk section to show it.
//!
//! Prims are authored in the BLOCK'S OWN space and carried into the world by
//! one frame per instance (`BlockDrawInstance::frame`) — for a model
//! block, its footprint space turned by the placed facing. That is what lets a
//! mod compute geometry against the model it can see without knowing where its
//! machine was placed or which way it faces.

use glam::{Mat4, Vec3};

use crate::BlockDrawInstance;
use petramond::world::draw::BlockDrawPrim;
use petramond_math::face::Face;
use petramond_mesh::Vertex;
use petramond_world::item::ItemRenderKind;

use super::item_cube::{push_block_item_cube_lit, push_cell_local_face};
use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};

#[derive(Clone, Copy)]
pub struct VisibleDraws<'a> {
    pub all: &'a [BlockDrawInstance],
    pub visible: &'a [u32],
    pub origin: glam::IVec3,
    pub time: f32,
    pub eye: Vec3,
}

impl<'a> VisibleDraws<'a> {
    fn iter(&self) -> impl Iterator<Item = &'a BlockDrawInstance> {
        let all = self.all;
        self.visible.iter().map(move |&i| &all[i as usize])
    }
}

fn prim_light(inst: &BlockDrawInstance, emissive: bool) -> DynLight {
    if emissive {
        DynLight::FULL
    } else {
        DynLight::new(inst.skylight, inst.blocklight)
    }
}

fn at(inst: &BlockDrawInstance, origin: glam::IVec3, p: [f32; 3]) -> Vec3 {
    inst.frame
        .relative_to(origin)
        .transform_point3(Vec3::from(p))
}

/// Append every instance's cuboid prims, plus the BLOCK-CUBE half of its item
/// prims (both are packed `Vertex` on the block atlas). Sprite- and
/// model-kind items ride their own explicit-UV streams — see
/// [`build_block_draw_sprites`] and [`build_block_draw_models`].
pub fn build_block_draws(draws: VisibleDraws<'_>, verts: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
    for inst in draws.iter() {
        for prim in &inst.set.resolved {
            match prim {
                BlockDrawPrim::Cuboid {
                    min,
                    max,
                    tile,
                    tint,
                    emissive,
                } => {
                    let light = prim_light(inst, *emissive);
                    let start = verts.len();
                    // Cell-local UV is a position INSIDE one cell, so the box
                    // is built from the corner of the cell it starts in rather
                    // than from the footprint origin. A multi-cell model's
                    // prims are authored in footprint blocks (the forge's basin
                    // pool starts at x = 1.22), and feeding that straight in
                    // asked for u = 19/16ths: a debug assert in the packer, and
                    // in a release build a face sampling a quarter of a tile
                    // past its own art.
                    let cell = [min[0].floor(), min[1].floor(), min[2].floor()];
                    let rel = |p: &[f32; 3]| [p[0] - cell[0], p[1] - cell[1], p[2] - cell[2]];
                    for face in Face::ALL {
                        push_cell_local_face(
                            verts,
                            indices,
                            *tile,
                            Vec3::from(cell),
                            1.0,
                            rel(min),
                            rel(max),
                            face,
                            light,
                        );
                    }
                    let to_render = inst.frame.relative_to(draws.origin);
                    for v in verts[start..].iter_mut() {
                        v.pos = to_render.transform_point3(Vec3::from(v.pos)).to_array();
                    }
                    multiply_tint(&mut verts[start..], *tint);
                }
                BlockDrawPrim::Item {
                    at: local,
                    scale,
                    yaw,
                    pitch,
                    item,
                    tint,
                } => {
                    let ItemRenderKind::BlockCube(block) = item.render_kind() else {
                        continue;
                    };
                    let start = verts.len();
                    let centre = at(inst, draws.origin, *local);
                    push_block_item_cube_lit(
                        verts,
                        indices,
                        block,
                        centre - Vec3::splat(*scale * 0.5),
                        *scale,
                        prim_light(inst, false),
                        false,
                    );
                    let m = spin_of(inst, centre) * orient(centre, *yaw, *pitch);
                    for v in verts[start..].iter_mut() {
                        v.pos = m.transform_point3(Vec3::from(v.pos)).to_array();
                    }
                    multiply_tint(&mut verts[start..], *tint);
                }
                BlockDrawPrim::Sprite { .. } => {}
            }
        }
    }
}

struct SpriteSlab {
    at: [f32; 3],
    scale: f32,
    yaw: f32,
    pitch: f32,
    tile: petramond_world::tile::Tile,
    tint: [u8; 3],
    emissive: bool,
    motion: Option<SlabMotion>,
}

#[derive(Clone, Copy)]
struct SlabMotion {
    bob: [f32; 2],
    faces_viewer: bool,
}

impl SpriteSlab {
    fn of(prim: &BlockDrawPrim, time: f32) -> Option<Self> {
        match *prim {
            BlockDrawPrim::Item {
                at,
                scale,
                yaw,
                pitch,
                item,
                tint,
            } => {
                let ItemRenderKind::Sprite(tile) = item.render_kind() else {
                    return None;
                };
                Some(Self {
                    at,
                    scale,
                    yaw,
                    pitch,
                    tile,
                    tint,
                    emissive: false,
                    motion: None,
                })
            }
            BlockDrawPrim::Sprite {
                at,
                scale,
                yaw,
                pitch,
                spin,
                bob,
                faces_viewer,
                tile,
                tint,
                emissive,
            } => Some(Self {
                at,
                scale,
                yaw: (yaw + spin * time) % std::f32::consts::TAU,
                pitch,
                tile,
                tint,
                emissive,
                motion: Some(SlabMotion { bob, faces_viewer }),
            }),
            BlockDrawPrim::Cuboid { .. } => None,
        }
    }
}

pub fn build_block_draw_sprites(
    draws: VisibleDraws<'_>,
    env: LightEnv,
    scratch: &mut Vec<ItemVertex>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for inst in draws.iter() {
        for prim in &inst.set.resolved {
            let Some(sprite) = SpriteSlab::of(prim, draws.time) else {
                continue;
            };
            let count = super::item_model::build_extruded_item_lit(
                sprite.tile,
                prim_light(inst, sprite.emissive),
                env,
                scratch,
            );
            if count == 0 {
                continue;
            }
            let mut centre = at(inst, draws.origin, sprite.at);
            let mut turn = block_rotation(inst);
            if let Some(SlabMotion {
                bob: [height, seconds],
                faces_viewer,
            }) = sprite.motion
            {
                if height != 0.0 && seconds != 0.0 {
                    centre.y += height * (draws.time * std::f32::consts::TAU / seconds).sin();
                }
                if faces_viewer {
                    let to = draws.eye - centre;
                    turn = Mat4::from_rotation_y(to.x.atan2(to.z));
                }
            }
            let m = Mat4::from_translation(centre)
                * turn
                * Mat4::from_rotation_y(sprite.yaw)
                * Mat4::from_rotation_x(sprite.pitch);
            let base = verts.len() as u32;
            let t = tint_floats(sprite.tint);
            for v in scratch.iter() {
                let local = Vec3::from(v.pos) * sprite.scale;
                verts.push(ItemVertex {
                    pos: m.transform_point3(local).to_array(),
                    tint: [v.tint[0] * t[0], v.tint[1] * t[1], v.tint[2] * t[2]],
                    ..*v
                });
            }
            indices.extend(base..base + count);
        }
    }
}

pub fn build_block_draw_models(
    draws: VisibleDraws<'_>,
    env: LightEnv,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for inst in draws.iter() {
        for prim in &inst.set.resolved {
            let BlockDrawPrim::Item {
                at: local,
                scale,
                yaw,
                pitch,
                item,
                tint,
            } = prim
            else {
                continue;
            };
            let ItemRenderKind::Model(kind) = item.render_kind() else {
                continue;
            };
            let transform = Mat4::from_translation(at(inst, draws.origin, *local))
                * block_rotation(inst)
                * Mat4::from_rotation_y(*yaw)
                * Mat4::from_rotation_x(*pitch)
                * Mat4::from_scale(Vec3::splat(*scale));
            let start = verts.len();
            super::item_model::build_block_model_item(
                kind,
                transform,
                prim_light(inst, false),
                env,
                None,
                verts,
                indices,
            );
            let t = tint_floats(*tint);
            for v in verts[start..].iter_mut() {
                v.tint = [v.tint[0] * t[0], v.tint[1] * t[1], v.tint[2] * t[2]];
            }
        }
    }
}

fn block_rotation(inst: &BlockDrawInstance) -> Mat4 {
    let mut m = inst.frame.transform;
    m.w_axis = glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
    m
}

fn spin_of(inst: &BlockDrawInstance, centre: Vec3) -> Mat4 {
    Mat4::from_translation(centre) * block_rotation(inst) * Mat4::from_translation(-centre)
}

fn tint_floats(tint: [u8; 3]) -> [f32; 3] {
    [
        tint[0] as f32 / 255.0,
        tint[1] as f32 / 255.0,
        tint[2] as f32 / 255.0,
    ]
}

fn orient(centre: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(centre)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_translation(-centre)
}

fn multiply_tint(verts: &mut [Vertex], tint: [u8; 3]) {
    if tint == [255, 255, 255] {
        return;
    }
    let t = [
        tint[0] as f32 / 255.0,
        tint[1] as f32 / 255.0,
        tint[2] as f32 / 255.0,
    ];
    for v in verts.iter_mut() {
        let base = petramond_mesh::unpack_tint(v.tint);
        v.tint = petramond_mesh::retint(v.tint, [base[0] * t[0], base[1] * t[1], base[2] * t[2]]);
    }
}
