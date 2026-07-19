use glam::{IVec3, Vec3};

use crate::block_model::{self, BlockModelKind};
use crate::facing::Facing;
use crate::torch::warm_tint;

use super::super::vertex::{ContactShadowVertex, ModelVertex};

/// Mesh-time brightness for a bbmodel-block face from the cell's combined 6-bit light.
/// Mirrors `block.wgsl`'s skylight curve (the block pipeline applies this in the shader;
/// the model pass shader just multiplies, so we bake the curve in here) — keep the
/// constants in sync.
#[inline]
/// Stream one bbmodel-block cell's geometry into the `model` buffers: copy the cell's
/// startup-baked template (positions already taken through the cube rotation + placement
/// facing) translated to the world base, carrying the cell's (sky, block) light
/// separately so the world-model shader applies the day/night scale at draw time,
/// plus the warm block-light tint. No matrices / quaternions / face-bias work
/// happens per remesh — it's all resolved once in [`block_model::ModelInstance`],
/// so meshing a placed model is a translate + scale + copy.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_model_block(
    verts: &mut Vec<ModelVertex>,
    indices: &mut Vec<u32>,
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
    wx: i32,
    wy: i32,
    wz: i32,
    sky6: u32,
    block6: u32,
    warm: f32,
) {
    let inst = block_model::instance(kind);
    let Some(tmpl) = inst.cell_template(offset, facing) else {
        return;
    };
    // The chunk stores the authored cell offset + placed facing; together those resolve the
    // rotated footprint base. The template's vertices are baked relative to that base, so
    // placing the cell is one translate per vertex.
    let base = block_model::base_from_cell(IVec3::new(wx, wy, wz), kind, offset, facing);
    let basef = Vec3::new(base.x as f32, base.y as f32, base.z as f32);
    let light = [
        (sky6 as f32 / 63.0).clamp(0.0, 1.0),
        (block6 as f32 / 63.0).clamp(0.0, 1.0),
    ];
    let tint = warm_tint([1.0, 1.0, 1.0], warm);
    let start = verts.len() as u32;
    verts.extend(tmpl.verts.iter().map(|v| ModelVertex {
        pos: (basef + v.pos).to_array(),
        uv: v.uv,
        shade: v.shade,
        tint,
        light,
    }));
    indices.extend(tmpl.indices.iter().map(|&i| start + i));
}

/// Stream one bottom footprint cell's contact-shadow stamp: the startup-baked
/// single-cell pieces translated to the world base, coincident with the top
/// face of the supported floor (the contact pass's coplanar bias resolves the
/// depth tie). Each piece — the cell's own floor AND its owned spill onto the
/// dilation ring — is gated INDIVIDUALLY through `supports_stamp(x, z)` on the
/// stamped cell's own column, which is what lets the shadow cross onto the
/// grass next to the model while an unsupported neighbouring cell still clips
/// it. Every stamped cell is within ±1 of `(wx, wz)` by construction, so the
/// gate's reads stay inside the mesh pad.
pub(super) fn emit_model_contact(
    contact: &mut Vec<ContactShadowVertex>,
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
    wx: i32,
    wy: i32,
    wz: i32,
    supports_stamp: impl Fn(i32, i32) -> bool,
) {
    let inst = block_model::instance(kind);
    let Some(tmpl) = inst.contact_template(offset, facing) else {
        return;
    };
    let base = block_model::base_from_cell(IVec3::new(wx, wy, wz), kind, offset, facing);
    let basef = Vec3::new(base.x as f32, base.y as f32, base.z as f32);
    for piece in &tmpl.pieces {
        if !supports_stamp(wx + piece.cell_delta[0], wz + piece.cell_delta[1]) {
            continue;
        }
        contact.extend(piece.verts.iter().map(|v| ContactShadowVertex {
            pos: (basef + Vec3::from(v.pos)).to_array(),
            darken: v.darken,
        }));
    }
}
