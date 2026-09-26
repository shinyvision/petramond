use glam::{IVec3, Vec3};

use petramond_world::block_model::{self, BlockModelKind};
use petramond_world::facing::Facing;

use super::super::face::Face;
use super::super::vertex::{ContactShadowVertex, ModelVertex};

/// The model streams a bbmodel cell lands in: one shared vertex buffer and
/// its opaque and alpha-blend index streams.
pub(super) struct ModelStreams<'m> {
    pub(super) verts: &'m mut Vec<ModelVertex>,
    pub(super) indices: &'m mut Vec<u32>,
    pub(super) blend_indices: &'m mut Vec<u32>,
}

/// One placed bbmodel cell: which model, which of its footprint cells (the
/// authored offset), how it is turned, and where it sits — its world cell and
/// the mesh-space origin positions are relative to.
#[derive(Copy, Clone)]
pub(super) struct PlacedModelCell {
    pub(super) kind: BlockModelKind,
    pub(super) offset: [u8; 3],
    pub(super) facing: Facing,
    pub(super) cell: IVec3,
    pub(super) anchor: IVec3,
}

impl PlacedModelCell {
    /// The rotated footprint base in mesh space: the chunk stores the authored
    /// cell offset + placed facing, and together those resolve it. Templates
    /// are baked relative to that base, so placing a cell is one translate per
    /// vertex.
    fn mesh_base(self) -> Vec3 {
        let base = block_model::base_from_cell(self.cell, self.kind, self.offset, self.facing);
        (base - self.anchor).as_vec3()
    }
}

/// How a model cell's copied runs are finished: the mesh-space translate, the
/// packed cell light, and the tint its tinted faces take.
#[derive(Copy, Clone)]
struct RunStyle {
    basef: Vec3,
    light: u32,
    tint: u32,
}

/// Stream one bbmodel-block cell's geometry into the `model` buffers: copy the cell's
/// startup-baked template (positions already taken through the cube rotation + placement
/// facing) translated to its base in mesh space (relative to `anchor`), carrying the cell's sky light and its
/// COLOURED block light separately so the world-model shader applies the
/// day/night scale at draw time. No matrices / quaternions / face-bias work
/// happens per remesh — it's all resolved once in [`block_model::ModelInstance`],
/// so meshing a placed model is a translate + scale + copy.
///
/// Each template segment is gated before copying: an optional parts-mask bit,
/// then an optional cullface world direction tested through `cull` (true = the
/// neighbour is opaque and the run is skipped). Blend-routed segments (faces
/// with semi-transparent texels) index into `blend_indices` — the same shared
/// vertex buffer, drawn later by the alpha-blend pass.
pub(super) fn emit_model_block(
    out: ModelStreams<'_>,
    at: PlacedModelCell,
    sky6: u32,
    block: petramond_world::light::BlockLight6,
    parts: u32,
    tint: u32,
    cull: impl Fn(Face) -> bool,
) {
    let inst = block_model::instance(at.kind);
    let Some(tmpl) = inst.cell_template(at.offset, at.facing) else {
        return;
    };
    let style = RunStyle {
        basef: at.mesh_base(),
        light: super::super::vertex::pack_model_light(sky6, block),
        tint,
    };
    emit_segments(tmpl, style, parts, &cull, out);
}

/// The gate + route core of [`emit_model_block`]: each segment is gated on its
/// optional parts-mask bit and its optional cullface direction, then copied
/// into the shared vertex buffer with its indices landing in the opaque or
/// blend index stream. The segments are contiguous runs in the baked template,
/// so an arbitrary parts mask / neighbour configuration is a handful of slice
/// copies — and every index is rebased onto this emission's own vertex
/// numbering, since the runs are no longer adjacent.
fn emit_segments(
    tmpl: &block_model::ModelCellTemplate,
    style: RunStyle,
    parts: u32,
    cull: &dyn Fn(Face) -> bool,
    out: ModelStreams<'_>,
) {
    let ModelStreams {
        verts,
        indices,
        blend_indices,
    } = out;
    for seg in &tmpl.segments {
        if let Some(p) = seg.part {
            if parts & (1 << p) == 0 {
                continue;
            }
        }
        if let Some(f) = seg.cull {
            if cull(f) {
                continue;
            }
        }
        let dst = if seg.blend {
            &mut *blend_indices
        } else {
            &mut *indices
        };
        copy_run(tmpl, &seg.run, style, verts, dst);
    }
}

/// Copy one baked run into the emission, rebasing its indices.
///
/// The runs are contiguous in the template but NOT adjacent in the emission —
/// a mask that skips part 0 puts part 1's vertices where part 0's would have
/// been — so every index has to move by the gap between where its run starts
/// in the template and where it starts here. Getting that wrong does not
/// crash; it silently draws another part's triangles.
fn copy_run(
    tmpl: &block_model::ModelCellTemplate,
    run: &block_model::PartRun,
    style: RunStyle,
    verts: &mut Vec<ModelVertex>,
    indices: &mut Vec<u32>,
) {
    let RunStyle { basef, light, tint } = style;
    if run.vert_len == 0 {
        return;
    }
    let (vs, vl) = (run.vert_start as usize, run.vert_len as usize);
    let (is, il) = (run.index_start as usize, run.index_len as usize);
    let start = verts.len() as u32;
    verts.extend(tmpl.verts[vs..vs + vl].iter().map(|v| ModelVertex {
        pos: (basef + v.pos).to_array(),
        uv: v.uv,
        shade: v.shade,
        light: light | (u32::from(v.appearance.unlit) << 31),
        tint: v.appearance.packed(v.tinted.then_some(tint)),
    }));
    indices.extend(
        tmpl.indices[is..is + il]
            .iter()
            .map(|&i| start + i - run.vert_start),
    );
}

/// Stream one bottom footprint cell's contact-shadow stamp: the startup-baked
/// single-cell pieces translated to the base in mesh space, coincident with the top
/// face of the supported floor (the contact pass's coplanar bias resolves the
/// depth tie). Each piece — the cell's own floor AND its owned spill onto the
/// dilation ring — is gated INDIVIDUALLY through `supports_stamp(x, z)` on the
/// stamped cell's own column, which is what lets the shadow cross onto the
/// grass next to the model while an unsupported neighbouring cell still clips
/// it. Every stamped cell is within ±1 of the cell's column by construction,
/// so the gate's reads stay inside the mesh pad.
pub(super) fn emit_model_contact(
    contact: &mut Vec<ContactShadowVertex>,
    at: PlacedModelCell,
    supports_stamp: impl Fn(i32, i32) -> bool,
) {
    let inst = block_model::instance(at.kind);
    let Some(tmpl) = inst.contact_template(at.offset, at.facing) else {
        return;
    };
    let basef = at.mesh_base();
    for piece in &tmpl.pieces {
        if !supports_stamp(at.cell.x + piece.cell_delta[0], at.cell.z + piece.cell_delta[1]) {
            continue;
        }
        contact.extend(piece.verts.iter().map(|v| ContactShadowVertex {
            pos: (basef + Vec3::from(v.pos)).to_array(),
            darken: v.darken,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vertex::MODEL_TINT_NONE;
    use block_model::{ModelCellTemplate, ModelTemplateVertex, PartRun, TemplateSegment};

    /// A three-segment template: 2 always-on verts, then two optional parts of
    /// 2 verts each, every run's indices numbered against the WHOLE template.
    fn template() -> ModelCellTemplate {
        let v = |x: f32, tinted: bool| ModelTemplateVertex {
            appearance: block_model::FaceAppearance::default(),
            pos: Vec3::new(x, 0.0, 0.0),
            uv: [0.0, 0.0],
            shade: 1.0,
            tinted,
        };
        let seg = |run: PartRun, part: Option<u8>| TemplateSegment {
            run,
            blend: false,
            part,
            cull: None,
        };
        ModelCellTemplate {
            verts: vec![
                v(0.0, false),
                v(1.0, false),
                v(2.0, true),
                v(3.0, true),
                v(4.0, false),
                v(5.0, false),
            ],
            indices: vec![0, 1, 2, 3, 4, 5],
            segments: vec![
                seg(
                    PartRun {
                        vert_start: 0,
                        vert_len: 2,
                        index_start: 0,
                        index_len: 2,
                    },
                    None,
                ),
                seg(
                    PartRun {
                        vert_start: 2,
                        vert_len: 2,
                        index_start: 2,
                        index_len: 2,
                    },
                    Some(0),
                ),
                seg(
                    PartRun {
                        vert_start: 4,
                        vert_len: 2,
                        index_start: 4,
                        index_len: 2,
                    },
                    Some(1),
                ),
            ],
        }
    }

    /// The copy/gate core of [`emit_model_block`] without the `BlockModelKind`
    /// lookup: emit `mask`'s segments, beside the TEMPLATE vertices their
    /// indices should have resolved to — the x coordinate identifies each
    /// uniquely.
    fn emit(mask: u32) -> (Vec<ModelVertex>, Vec<u32>, Vec<f32>) {
        let tmpl = template();
        let (mut verts, mut indices) = (Vec::new(), Vec::new());
        let mut want = Vec::new();
        for seg in &tmpl.segments {
            if let Some(p) = seg.part {
                if mask & (1 << p) == 0 {
                    continue;
                }
            }
            let (is, il) = (seg.run.index_start as usize, seg.run.index_len as usize);
            want.extend(
                tmpl.indices[is..is + il]
                    .iter()
                    .map(|&i| tmpl.verts[i as usize].pos.x),
            );
            let style = RunStyle {
                basef: Vec3::ZERO,
                light: 0,
                tint: 0,
            };
            copy_run(&tmpl, &seg.run, style, &mut verts, &mut indices);
        }
        (verts, indices, want)
    }

    /// Every mask must emit indices that address ITS OWN vertices. Skipping a
    /// run shifts everything after it, and an index left pointing at the
    /// template's numbering draws the wrong part rather than failing.
    #[test]
    fn every_part_mask_emits_indices_addressing_its_own_vertices() {
        for mask in 0..4u32 {
            let (verts, indices, want) = emit(mask);
            assert!(
                indices.iter().all(|&i| (i as usize) < verts.len()),
                "mask {mask:#b} emitted an out-of-range index"
            );
            let got: Vec<f32> = indices.iter().map(|&i| verts[i as usize].pos[0]).collect();
            assert_eq!(got, want, "mask {mask:#b} points at the wrong vertices");
        }
    }

    /// Only the cubes a row named tintable may carry the tint; the mask must
    /// not shift which vertices those are.
    #[test]
    fn the_tint_lane_follows_the_vertex_not_the_slot() {
        let (verts, ..) = emit(0b01);
        assert_eq!(verts[0].tint, MODEL_TINT_NONE);
        assert_eq!(verts[2].tint, 0, "part 0's cubes are the tinted ones");
    }

    /// One quad per segment: ungated-opaque, cull-gated, blend-routed — each
    /// vert's x identifies its segment.
    fn gated_template() -> ModelCellTemplate {
        let quad = |x: f32| {
            (0..4)
                .map(|i| ModelTemplateVertex {
                    appearance: block_model::FaceAppearance::default(),
                    pos: Vec3::new(x + i as f32 * 0.01, 0.0, 0.0),
                    uv: [0.0, 0.0],
                    shade: 1.0,
                    tinted: false,
                })
                .collect::<Vec<_>>()
        };
        let seg = |run: PartRun, blend: bool, cull: Option<Face>| TemplateSegment {
            run,
            blend,
            part: None,
            cull,
        };
        ModelCellTemplate {
            verts: [quad(0.0), quad(1.0), quad(2.0)].concat(),
            indices: (0..3u32)
                .flat_map(|q| [q * 4, q * 4 + 1, q * 4 + 2])
                .collect(),
            segments: vec![
                seg(
                    PartRun {
                        vert_start: 0,
                        vert_len: 4,
                        index_start: 0,
                        index_len: 3,
                    },
                    false,
                    None,
                ),
                seg(
                    PartRun {
                        vert_start: 4,
                        vert_len: 4,
                        index_start: 3,
                        index_len: 3,
                    },
                    false,
                    Some(Face::NegY),
                ),
                seg(
                    PartRun {
                        vert_start: 8,
                        vert_len: 4,
                        index_start: 6,
                        index_len: 3,
                    },
                    true,
                    None,
                ),
            ],
        }
    }

    /// A cullface-gated segment draws only while its world neighbour is
    /// non-opaque: the predicate suppresses exactly that run, and remeshing
    /// after the neighbour leaves must restore it byte-identically.
    #[test]
    fn cull_gated_segments_follow_the_neighbour_predicate() {
        let tmpl = gated_template();
        let run = |cull: &dyn Fn(Face) -> bool| {
            let (mut verts, mut indices, mut blend) = (Vec::new(), Vec::new(), Vec::new());
            emit_segments(
                &tmpl,
                RunStyle {
                    basef: Vec3::ZERO,
                    light: 0,
                    tint: 0,
                },
                0,
                cull,
                ModelStreams {
                    verts: &mut verts,
                    indices: &mut indices,
                    blend_indices: &mut blend,
                },
            );
            (
                indices
                    .iter()
                    .map(|&i| verts[i as usize].pos[0].floor() as i32)
                    .collect::<Vec<_>>(),
                blend
                    .iter()
                    .map(|&i| verts[i as usize].pos[0].floor() as i32)
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(run(&|_| false), (vec![0, 0, 0, 1, 1, 1], vec![2, 2, 2]));
        assert_eq!(
            run(&|f| f == Face::NegY),
            (vec![0, 0, 0], vec![2, 2, 2]),
            "an opaque block below suppresses the down-cullface run only"
        );
    }

    /// Blend-routed segments index into the BLEND stream but address the SAME
    /// vertex buffer as the opaque runs — a split vertex stream would leave
    /// the blend indices pointing at ghosts.
    #[test]
    fn blend_segments_share_the_vertex_buffer() {
        let tmpl = gated_template();
        let (mut verts, mut indices, mut blend) = (Vec::new(), Vec::new(), Vec::new());
        emit_segments(
            &tmpl,
            RunStyle {
                basef: Vec3::ZERO,
                light: 0,
                tint: 0,
            },
            0,
            &|_| false,
            ModelStreams {
                verts: &mut verts,
                indices: &mut indices,
                blend_indices: &mut blend,
            },
        );
        assert_eq!(verts.len(), 12, "both streams share one vertex emission");
        assert!(blend.iter().all(|&i| (i as usize) < verts.len()));
        assert!(
            blend
                .iter()
                .all(|&i| verts[i as usize].pos[0].floor() == 2.0),
            "the blend stream holds exactly the blend segment's quad"
        );
        assert!(indices
            .iter()
            .all(|&i| verts[i as usize].pos[0].floor() < 2.0));
    }
}
