//! World-space + item-space geometry for ANIMATED block models — every block
//! drawn outside the chunk mesh (a chest's hinged lid, a door's swing, a
//! trapdoor's panel, a pack's own), from its data model
//! ([`petramond_world::animated_model`]). One builder serves them all: nothing
//! here knows which block it is drawing.
//!
//! Each model is authored in a canonical unit cell with its front / closed edge
//! on `+Z` (south). For a placed block every part is swung about its joint by
//! the eased open fraction (a CPU vertex transform, like the item-entity spin),
//! then the whole model is turned about the cell's vertical centre to the
//! pose's `facing` and translated to the world. The geometry is baked into a
//! reusable dynamic vbuf/ibuf and drawn by the **existing** opaque block
//! pipeline — exactly like [`item_entity`](super::item_entity).
//!
//! Why dynamic (not chunk-meshed): a model's parts move every frame while it
//! swings, and re-meshing the owning chunk per frame would be far too
//! expensive, so an animated row opts out of chunk meshing entirely (its
//! `mesh_emitter` is `Nothing`) and is drawn here.

use glam::Vec3;

use super::item_cube::{orient_faces_to_block, push_box_faces_lit_mirrored, thin_face_slice_modes};
use super::lighting::DynLight;
use super::BlockEntityInstance;
use petramond_mesh::Vertex;
use petramond_world::animated_model::{AnimatedModelDef, ModelPart};
use petramond_world::block::Block;

/// Append every instance's geometry to `verts`/`indices` (NOT cleared). The
/// caller frustum-culls first.
pub fn push_block_entities(
    instances: &[BlockEntityInstance],
    render_origin: glam::IVec3,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) {
    for inst in instances {
        push_block_entity_world(verts, indices, inst, render_origin);
    }
}

/// Append one part's six faces, textured for `block`, over `min..max`.
fn push_part(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    part: &ModelPart,
    block: Block,
    min: Vec3,
    max: Vec3,
    light: DynLight,
) {
    // A panel-thin part crops its edge faces to a matching strip of their
    // tile instead of squishing a whole tile flat; derived from the part's
    // own extent by the one rule the crack overlay reads too.
    let slice = if part.thin_edges {
        thin_face_slice_modes(min, max)
    } else {
        [0; 6]
    };
    push_box_faces_lit_mirrored(
        verts,
        indices,
        part.tiles(block),
        min,
        max,
        light,
        part.mirror,
        slice,
    );
}

/// Append one placed animated block for `inst`: its pose's variant, each part
/// swung by the open fraction, turned to `facing`, lit by its cell light, at
/// the world cell `pos`.
fn push_block_entity_world(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    inst: &BlockEntityInstance,
    render_origin: glam::IVec3,
) {
    let Some(model) = inst.block.animated_model() else {
        return;
    };
    let light = DynLight::new(inst.skylight, inst.blocklight);
    let start = verts.len();
    for part in model.variant(inst.variant).parts {
        let part_start = verts.len();
        push_part(
            verts,
            indices,
            part,
            inst.block,
            Vec3::from(part.min),
            Vec3::from(part.max),
            light,
        );
        let Some(joint) = part.joint else {
            continue;
        };
        let angle = joint.angle(inst.open01);
        if angle != 0.0 {
            let sin_cos = angle.sin_cos();
            for v in verts[part_start..].iter_mut() {
                v.pos = joint.rotate(v.pos, sin_cos);
            }
        }
    }
    orient_faces_to_block(
        verts,
        start,
        inst.facing,
        (inst.pos - render_origin).as_vec3(),
    );
}

/// The animated model a block's ITEM draws (icon, in hand, dropped), if its
/// row opts in.
pub(super) fn item_model(block: Block) -> Option<&'static AnimatedModelDef> {
    block.animated_model().filter(|m| m.item)
}

/// Build `model` CLOSED (variant `0`, no swing) for `block`, centred in the cube
/// `[origin, origin+size]`, front on `+Z`, lit by `light` — the item form of an
/// animated block, shared by the inventory icon, the held item and a dropped
/// stack so it reads as the block rather than a plain cube. The parts are
/// appended in authored order, which is also the painter order the DEPTHLESS
/// icon pass relies on (back to front).
pub(super) fn push_item(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    model: &AnimatedModelDef,
    block: Block,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let lift = Vec3::new(0.0, model.item_lift, 0.0);
    let map = |a: [f32; 3]| origin + (Vec3::from(a) + lift) * size;
    for part in model.variant(0).parts {
        push_part(
            verts,
            indices,
            part,
            block,
            map(part.min),
            map(part.max),
            light,
        );
    }
}

#[cfg(test)]
mod tests;
