//! Block-break crack overlay geometry for CELL-SHAPED blocks.
//!
//! A bbmodel block is NOT built here: its crack is a decal drawn over the
//! model's own triangles by [`crate::model_break`], so nothing re-derives the
//! model's form from boxes.
//!
//! From a [`BreakOverlayView`] (target block + crack stage 0..9), builds the six
//! faces of that block's **exact** unit cube, each textured with the matching
//! the stage's `destroy_stage_{stage}` tile. The cube is built at the block's integer world
//! coordinates with no inflation, so every face is *coincident* with the chunk
//! mesh's face for that block — same `quad_for` corners, same world positions.
//! The dedicated `break_overlay.wgsl` pipeline draws it depth `LessEqual` /
//! no-write so the crack lands on the block surface (no inflation to misalign the
//! decal at glancing angles).
//!
//! Coincident corners are *not* enough on their own: the chunk mesher flips each
//! face's triangulation diagonal per-AO (`should_flip` in `mesh::face`) while this
//! cube always splits 0->2, so the two surfaces interpolate depth a ULP apart per
//! pixel and would speckle-fight. The break pipeline therefore applies a small
//! polygon offset toward the camera (`BREAK_DEPTH_BIAS`) so the crack wins that tie
//! everywhere.
//!
//! Geometry is relative to the render origin (the break pipeline's vertex shader transforms by
//! `view_proj`, like the block pipeline) and full-bright. Built into a
//! caller-owned `Vec` whose capacity is reused frame to frame.

use glam::Vec3;

use super::item_cube::{push_box_faces_lit, push_cube_textured};
use super::BreakOverlayView;
use petramond_mesh::Vertex;
use petramond_world::tile::Tile;

/// The destroy tile for crack `stage` (clamped 0..=9), as a [`Tile`]. The one
/// stage->tile answer: the model decal pass reads it too.
#[inline]
pub(crate) fn destroy_tile(stage: u8) -> Tile {
    petramond_world::tile::engine().destroy_stages[stage.min(9) as usize]
}

/// Build the crack overlay geometry for every view in `views` into `verts` /
/// `indices` (cleared first, capacity reused) — ONE combined stream, since
/// every overlay shares the break pipeline. Returns the index count. The
/// slice is small and bounded (the local miner's own crack plus the capped
/// nearest remotes); each entry bakes exactly as the single overlay always
/// did, so the single-player path is geometry-identical.
pub fn build_break_overlays(
    views: &[BreakOverlayView],
    render_origin: glam::IVec3,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for view in views {
        // A model block's crack is drawn over the model's own triangles by the
        // decal pass, so it contributes no geometry here.
        if view.model.is_none() {
            append_break_overlay(view, render_origin, verts, indices);
        }
    }
    indices.len() as u32
}

/// Build one crack overlay's geometry into `verts` / `indices` (cleared
/// first). Returns the index count. See [`build_break_overlays`] for the
/// multi-overlay frame path; this single-view form is the unit the tests pin.
#[cfg(test)]
pub fn build_break_overlay(
    view: &BreakOverlayView,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    build_break_overlays(
        std::slice::from_ref(view),
        glam::IVec3::ZERO,
        verts,
        indices,
    )
}

/// Append the crack overlay geometry for `view` (indices are vert-relative, so
/// composition is plain concatenation). All faces use the same destroy tile so
/// the crack reads from every angle. A plain cube cracks over its six cell
/// faces; a stair or slab over its meshed cell-local quads; a chest over its
/// inset box.
///
/// The cube spans the block's exact `[block, block + 1]` cell with no inflation,
/// so each face lands on the same integer-coordinate plane the chunk mesh emitted
/// for that block. The pipeline's depth `LessEqual` + a small polygon offset
/// (`BREAK_DEPTH_BIAS`) put the crack on the surface without z-fighting (see the
/// module docs for why the offset is needed).
fn append_break_overlay(
    view: &BreakOverlayView,
    render_origin: glam::IVec3,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) {
    let tile = destroy_tile(view.stage);
    let base = (view.block - render_origin).as_vec3();
    if let Some(cb) = view.shape_boxes {
        // Every box family cracks the same way: over the boxes its shape
        // resolved to, with cell-local UVs, emitting only the faces the family
        // emits. A stair's steps, a slab's occupied halves, a fence's post and
        // rails, a ladder's panel (minus the face buried in the wall) and a
        // chair's legs are all this one loop — the crack cannot disagree with
        // the meshed form because it reads the same producer.
        for b in cb.boxes.iter().take(cb.len as usize) {
            for (fi, face) in petramond_math::face::Face::ALL.into_iter().enumerate() {
                if !b.faces[fi] {
                    continue;
                }
                super::item_cube::push_cell_local_face_styled(
                    verts,
                    indices,
                    tile,
                    base,
                    1.0,
                    b.min,
                    b.max,
                    face,
                    super::lighting::DynLight::FULL,
                    super::item_cube::FaceArt {
                        pose: b.pose,
                        ..Default::default()
                    },
                );
            }
        }
    } else {
        match view.visual_box {
            // A non-full-cube block (the chest) cracks over its inset visual box.
            Some((mn, mx)) => {
                let min = base + Vec3::new(mn[0], mn[1], mn[2]);
                let max = base + Vec3::new(mx[0], mx[1], mx[2]);
                push_box_faces_lit(
                    verts,
                    indices,
                    [tile; 6],
                    [0; 6],
                    min,
                    max,
                    super::lighting::DynLight::FULL,
                );
            }
            None => push_cube_textured(verts, indices, [tile; 3], base, 1.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    #[test]
    fn destroy_tile_maps_stage_and_clamps() {
        assert_eq!(destroy_tile(0), Tile::from_name("destroy_stage_0").unwrap());
        assert_eq!(destroy_tile(9), Tile::from_name("destroy_stage_9").unwrap());
        // Out-of-range stages clamp to the last stage.
        assert_eq!(
            destroy_tile(42),
            Tile::from_name("destroy_stage_9").unwrap()
        );
    }

    #[test]
    fn builds_one_coincident_cube_with_the_stage_tile() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = BreakOverlayView {
            block: IVec3::new(3, 64, -7),
            // A full cube (Stone) has no special visual box, so the crack spans the cell.
            visual_box: None,
            shape_boxes: None,
            model: None,
            stage: 4,
        };
        let n = build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert_eq!(n, 36);
        // Every face uses DestroyStage4 (the tile id is `packed`'s low field).
        let want = Tile::from_name("destroy_stage_4").unwrap().index() as u32;
        for vert in &v {
            assert_eq!(vert.packed & petramond_mesh::vertex::TILE_MASK, want);
        }
        // Coincident, not inflated: the cube spans the block cell [3,4] on x
        // *exactly*, so its faces sit on the chunk mesh's faces and the crack wins
        // the depth tie via LessEqual instead of poking proud of the surface.
        let min_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(min_x, 3.0, "cube min lands exactly on the block boundary");
        assert_eq!(max_x, 4.0, "cube max lands exactly on the block boundary");
    }

    /// Build a view whose cell resolved to `boxes` — `(min, max, faces)` in
    /// canonical face order (`+X, -X, +Y, -Y, +Z, -Z`).
    fn boxes_view(
        block: IVec3,
        boxes: &[([f32; 3], [f32; 3], [bool; 6])],
        stage: u8,
    ) -> BreakOverlayView {
        use crate::views::{CrackBox, CrackBoxes, MAX_CRACK_BOXES};
        let mut arr = [CrackBox {
            min: [0.0; 3],
            max: [0.0; 3],
            faces: [false; 6],
            pose: None,
        }; MAX_CRACK_BOXES];
        for (dst, &(min, max, faces)) in arr.iter_mut().zip(boxes) {
            *dst = CrackBox {
                min,
                max,
                faces,
                pose: None,
            };
        }
        BreakOverlayView {
            block,
            visual_box: None,
            shape_boxes: Some(CrackBoxes {
                boxes: arr,
                len: boxes.len() as u8,
            }),
            model: None,
            stage,
        }
    }

    /// The crack traces the cell's RESOLVED boxes with cell-local UVs, so the
    /// decal is CROPPED to each box (a half-height box's side shows the lower
    /// half of the destroy tile) instead of a full tile squashed onto it.
    #[test]
    fn crack_crops_the_tile_to_the_resolved_box() {
        let block = IVec3::new(-3, 70, 8);
        let view = boxes_view(block, &[([0.0; 3], [1.0, 0.5, 1.0], [true; 6])], 6);
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_break_overlay(&view, &mut v, &mut i);

        assert_eq!(v.len(), 6 * 4, "six faces of the one resolved box");
        for vert in &v {
            assert_eq!(
                (vert.packed >> petramond_mesh::UV_MODE_SHIFT) & 0x7,
                petramond_mesh::UV_MODE_CELL_LOCAL,
                "crack quads must carry cell-local UVs"
            );
            assert!(
                vert.pos[1] <= 70.5 + 1e-6,
                "crack must stay on the resolved box"
            );
            // Side-face verts (X or Z shade groups) sit in the cell's lower
            // half, so their cell-local v spans 8..=16 — the lower half of the
            // tile — instead of restarting at 0 (which would stretch the decal).
            let shade = (vert.packed >> petramond_mesh::vertex::SHADE_SHIFT) & 0x3;
            if shade == 1 || shade == 2 {
                let v16 = (vert.packed2 >> 11) & 0x1F;
                assert!(
                    (8..=16).contains(&v16),
                    "side crack v16 = {v16} must be cropped to the lower tile half"
                );
            }
        }
    }

    /// A face the shape does not EMIT takes no destroy texture. This is what
    /// keeps a wall-mounted panel's crack off the coplanar wall face behind it
    /// and a fence rail's guaranteed-covered end cap clean — the emitted-face
    /// set comes from the same producer the mesher used, so the two agree by
    /// construction rather than by two hand-kept copies.
    #[test]
    fn crack_skips_faces_the_shape_does_not_emit() {
        let mut faces = [true; 6];
        faces[5] = false; // NegZ — buried in the supporting wall.
        let view = boxes_view(
            IVec3::new(1, 2, 3),
            &[([0.0, 0.0, 0.0], [1.0, 1.0, 0.125], faces)],
            3,
        );
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 5 * 4, "the unemitted face draws no crack");
        // No whole quad lies on the buried z == 3.0 plane. (Side faces span
        // that plane's edge, so per-vertex tests would false-positive; only a
        // NegZ quad has all four corners on it.)
        assert!(
            v.chunks(4)
                .all(|q| !q.iter().all(|vert| (vert.pos[2] - 3.0).abs() < 1e-6)),
            "no crack quad on the face the shape never emits"
        );
    }

    #[test]
    fn reuses_buffers() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = BreakOverlayView {
            block: IVec3::ZERO,
            visual_box: None,
            shape_boxes: None,
            model: None,
            stage: 0,
        };
        build_break_overlay(&view, &mut v, &mut i);
        let (cap_v, cap_i) = (v.capacity(), i.capacity());
        // Same view -> identical vert/index count, so the cleared+refilled
        // buffers keep their capacity: rebuilding to the same size never reallocs.
        build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert_eq!(v.capacity(), cap_v, "vert buffer reused");
        assert_eq!(i.capacity(), cap_i, "index buffer reused");
    }

    #[test]
    fn multi_part_shape_cracks_over_its_parts_not_the_empty_cell() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = boxes_view(
            IVec3::new(0, 0, 0),
            &[
                ([0.1, 0.0, 0.1], [0.3, 0.5, 0.3], [true; 6]),
                ([0.7, 0.0, 0.7], [0.9, 0.5, 0.9], [true; 6]),
            ],
            4,
        );
        let n = build_break_overlay(&view, &mut v, &mut i);
        // TWO resolved boxes × 6 cell-local faces × 4 verts — the crack hugs
        // the shape's parts (a chair's legs), NOT a single 24-vert cube
        // spanning the empty cell.
        assert_eq!(v.len(), 48);
        assert_eq!(n, 72);
    }
}
