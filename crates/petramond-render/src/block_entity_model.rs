//! World-space geometry for ANIMATED block models (chest lids, door swings, trapdoor panels, a
//! pack's own), drawn outside the chunk mesh from their data model
//! ([`petramond_world::animated_model`]). One builder serves them all; nothing here knows which
//! block it is drawing.
//!
//! Models are authored in a unit cell with the closed edge on `+Z` (south). Each part swings about
//! its joint by the eased open fraction (a CPU vertex transform, like the item-entity spin), then
//! the whole model turns to the pose's `facing` and moves into the world. The result is baked into
//! a dynamic vbuf/ibuf and drawn by the existing opaque block pipeline, like
//! [`item_entity`](super::item_entity).
//!
//! Parts move every frame while swinging, and re-meshing the chunk per frame would be far too
//! expensive. So animated rows skip chunk meshing (`mesh_emitter` is `Nothing`) and are drawn
//! here.

use glam::Vec3;

use super::item_cube::{orient_faces_to_block, push_box_faces_lit_mirrored, thin_face_slice_modes};
use super::lighting::DynLight;
use super::BlockEntityInstance;
use petramond_mesh::Vertex;
use petramond_world::animated_model::{AnimatedModelDef, ModelPart};
use petramond_world::block::Block;

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

fn push_part(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    part: &ModelPart,
    block: Block,
    min: Vec3,
    max: Vec3,
    light: DynLight,
) {
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

pub(super) fn item_model(block: Block) -> Option<&'static AnimatedModelDef> {
    block.animated_model().filter(|m| m.item)
}

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
