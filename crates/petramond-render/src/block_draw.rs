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
//! one matrix per instance (`BlockDrawInstance::transform`) — for a model
//! block, its footprint space turned by the placed facing. That is what lets a
//! mod compute geometry against the model it can see without knowing where its
//! machine was placed or which way it faces.

use glam::{Mat4, Vec3};

use petramond_world::item::ItemRenderKind;
use petramond_math::face::Face;
use petramond_mesh::Vertex;
use crate::BlockDrawInstance;
use petramond::world::draw::BlockDrawPrim;

use super::item_cube::{push_block_item_cube_lit, push_cell_local_face};
use super::item_model::ItemVertex;
use super::lighting::{DynLight, LightEnv};

/// The room a draw-set builder may take in a stream it SHARES with the dropped
/// item entities.
///
/// Those streams are fixed buffers whose bake is all-or-nothing: one vertex
/// past the cap and the whole stream draws nothing — every dropped item in the
/// world, not just the machine that overflowed it. So a set is appended and
/// then kept only if it still fits, and the first one that does not ends the
/// run. Enough machines in view stop drawing their liquid; a floor of dropped
/// items does not vanish because someone built a workshop.
#[derive(Clone, Copy)]
pub struct StreamBudget {
    pub verts: usize,
    pub indices: usize,
}

impl StreamBudget {
    fn fits<V>(&self, verts: &[V], indices: &[u32]) -> bool {
        verts.len() <= self.verts && indices.len() <= self.indices
    }
}

/// The instances a frame draws: the published list plus the INDICES this
/// frame's camera kept. Indices rather than a filtered copy of the rows —
/// re-culling used to clone every visible instance (an `Arc` refcount pair and
/// ~96 bytes each, per frame) to say which ones survived.
#[derive(Clone, Copy)]
pub struct VisibleDraws<'a> {
    pub all: &'a [BlockDrawInstance],
    pub visible: &'a [u32],
}

impl<'a> VisibleDraws<'a> {
    fn iter(&self) -> impl Iterator<Item = &'a BlockDrawInstance> {
        let all = self.all;
        self.visible.iter().map(move |&i| &all[i as usize])
    }
}

/// Roll one instance's geometry back off both scratch buffers.
fn rewind<V>(verts: &mut Vec<V>, indices: &mut Vec<u32>, mark: (usize, usize)) {
    verts.truncate(mark.0);
    indices.truncate(mark.1);
}

/// The light one prim draws under: an `emissive` prim ignores the cell's own,
/// because molten metal in a dark forge is a SOURCE and dimming it to the room
/// would read as cold.
fn prim_light(inst: &BlockDrawInstance, emissive: bool) -> DynLight {
    if emissive {
        DynLight::FULL
    } else {
        DynLight::new(inst.skylight, inst.blocklight)
    }
}

/// A prim-space point in the world. Prims are authored in the BLOCK'S OWN
/// space — for a model block, the footprint space its `.bbmodel` is authored
/// in, turned by the placed facing — which is what lets a mod compute geometry
/// against the model it can see and have it land right at any placement.
fn at(inst: &BlockDrawInstance, p: [f32; 3]) -> Vec3 {
    inst.transform.transform_point3(Vec3::from(p))
}

/// Append every instance's cuboid prims, plus the BLOCK-CUBE half of its item
/// prims (both are packed `Vertex` on the block atlas). Sprite- and
/// model-kind items ride their own explicit-UV streams — see
/// [`build_block_draw_sprites`] and [`build_block_draw_models`].
pub fn build_block_draws(
    draws: VisibleDraws<'_>,
    budget: StreamBudget,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) {
    for inst in draws.iter() {
        let mark = (verts.len(), indices.len());
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
                    // Built axis-aligned at its own cell corner, then carried
                    // into prim space whole: a rotation of the box's CORNERS
                    // keeps it a box, and the tile UVs come out the same
                    // either way.
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
                    for v in verts[start..].iter_mut() {
                        v.pos = inst
                            .transform
                            .transform_point3(Vec3::from(v.pos))
                            .to_array();
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
                    let centre = at(inst, *local);
                    push_block_item_cube_lit(
                        verts,
                        indices,
                        block,
                        centre - Vec3::splat(*scale * 0.5),
                        *scale,
                        prim_light(inst, false),
                        false,
                    );
                    // The prim's own rotation composes UNDER the block's, or a
                    // mould lies flat at the right spot pointing the wrong way
                    // for three of four placements.
                    let m = spin_of(inst, centre) * orient(centre, *yaw, *pitch);
                    for v in verts[start..].iter_mut() {
                        v.pos = m.transform_point3(Vec3::from(v.pos)).to_array();
                    }
                    multiply_tint(&mut verts[start..], *tint);
                }
            }
        }
        if !budget.fits(verts, indices) {
            rewind(verts, indices, mark);
            break;
        }
    }
}

/// The SPRITE half of item prims: the item's own extruded pixel slab, the same
/// geometry it has in a hand or on the ground, scaled and yawed into place.
///
/// Drawing the ITEM — rather than an authored stand-in cube — is the point of
/// this prim: a mould in a basin and the mould in your inventory cannot drift
/// apart when the art changes, because they are the same sprite.
pub fn build_block_draw_sprites(
    draws: VisibleDraws<'_>,
    env: LightEnv,
    budget: StreamBudget,
    scratch: &mut Vec<ItemVertex>,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for inst in draws.iter() {
        let mark = (verts.len(), indices.len());
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
            let ItemRenderKind::Sprite(tile) = item.render_kind() else {
                continue;
            };
            let count = super::item_model::build_extruded_item_lit(
                tile,
                prim_light(inst, false),
                env,
                scratch,
            );
            if count == 0 {
                continue;
            }
            let centre = at(inst, *local);
            let m = Mat4::from_translation(centre)
                * block_rotation(inst)
                * Mat4::from_rotation_y(*yaw)
                * Mat4::from_rotation_x(*pitch);
            let base = verts.len() as u32;
            let t = tint_floats(*tint);
            for v in scratch.iter() {
                let local = Vec3::from(v.pos) * *scale;
                verts.push(ItemVertex {
                    pos: m.transform_point3(local).to_array(),
                    tint: [v.tint[0] * t[0], v.tint[1] * t[1], v.tint[2] * t[2]],
                    ..*v
                });
            }
            indices.extend(base..base + count);
        }
        if !budget.fits(verts, indices) {
            rewind(verts, indices, mark);
            break;
        }
    }
}

/// The BBMODEL half of item prims (their own atlas, so their own stream).
pub fn build_block_draw_models(
    draws: VisibleDraws<'_>,
    env: LightEnv,
    budget: StreamBudget,
    verts: &mut Vec<ItemVertex>,
    indices: &mut Vec<u32>,
) {
    for inst in draws.iter() {
        let mark = (verts.len(), indices.len());
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
            let transform = Mat4::from_translation(at(inst, *local))
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
        if !budget.fits(verts, indices) {
            rewind(verts, indices, mark);
            break;
        }
    }
}

/// The block's own rotation, with its translation dropped — an item prim is
/// placed by `at()` and then TURNED by this, so it follows the model it sits
/// in instead of the world axes.
fn block_rotation(inst: &BlockDrawInstance) -> Mat4 {
    let mut m = inst.transform;
    m.w_axis = glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
    m
}

/// [`block_rotation`] applied about a world point.
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

/// Pitch then yaw, about `centre`. Pitch comes first so `pitch = PI/2` lays a
/// standing item flat and `yaw` then turns it in the plane it is lying in —
/// the order a person would describe it in.
fn orient(centre: Vec3, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(centre)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_translation(-centre)
}

/// Multiply a straight RGB over the just-appended verts. Unlike the stack-dye
/// path this does NOT set the dyed flag: a draw prim names its own tile and
/// means the tint literally, where a dyed stack wants the desaturated twin.
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
