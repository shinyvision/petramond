//! Block-break crack overlay geometry.
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
//! Geometry is in WORLD space (the break pipeline's vertex shader transforms by
//! `view_proj`, like the block pipeline) and full-bright. Built into a
//! caller-owned `Vec` whose capacity is reused frame to frame.

use glam::Vec3;

use super::item_cube::{push_box_faces_lit, push_cube_textured};
use super::BreakOverlayView;
use crate::atlas::Tile;
use crate::mesh::Vertex;

/// Skip cracking a bbmodel cube whose LARGEST dimension is below this (in blocks).
const MIN_CRACK_EXTENT: f32 = 1.0;

/// The destroy tile for crack `stage` (clamped 0..=9), as a [`Tile`].
#[inline]
fn destroy_tile(stage: u8) -> Tile {
    crate::atlas::engine().destroy_stages[stage.min(9) as usize]
}

/// Build the crack overlay geometry for `view` into `verts` / `indices` (cleared
/// first, capacity reused). Returns the index count. All faces use the same
/// destroy tile so the crack reads from every angle. A plain cube cracks over its
/// six cell faces; a stair or slab over its meshed cell-local quads; a chest over
/// its inset box; a bbmodel block over its structural model cubes.
///
/// The cube spans the block's exact `[block, block + 1]` cell with no inflation,
/// so each face lands on the same integer-coordinate plane the chunk mesh emitted
/// for that block. The pipeline's depth `LessEqual` + a small polygon offset
/// (`BREAK_DEPTH_BIAS`) put the crack on the surface without z-fighting (see the
/// module docs for why the offset is needed).
pub fn build_break_overlay(
    view: &BreakOverlayView,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    let tile = destroy_tile(view.stage);
    let base = Vec3::new(
        view.block.x as f32,
        view.block.y as f32,
        view.block.z as f32,
    );
    if let Some((kind, offset, facing)) = view.model {
        // A bbmodel block cracks over its WHOLE model's actual cube surfaces, so the crack
        // hugs the model (every leg, the top) instead of one coarse box hanging in the
        // cell's empty air. Boxes are footprint-space, so transform them through the
        // placed model's facing and rotated-footprint base. The multi-block breaks as one
        // object, so the whole piece cracks (MC-like).
        let model_base = crate::block_model::base_from_cell(view.block, kind, offset, facing);
        let placement = crate::block_model::placement_transform(model_base, kind, facing);
        for b in crate::block_model::model_render_boxes(kind) {
            // Skip very small surfaces (decoration specks) — crack only the structural cubes.
            let ext = [
                b.max[0] - b.min[0],
                b.max[1] - b.min[1],
                b.max[2] - b.min[2],
            ];
            if ext[0].max(ext[1]).max(ext[2]) < MIN_CRACK_EXTENT {
                continue;
            }
            let (min, max) = transform_box(placement, b.min, b.max);
            push_box_faces_lit(
                verts,
                indices,
                [tile; 6],
                min,
                max,
                super::lighting::DynLight::FULL,
            );
        }
    } else if let Some(shape) = view.stair_shape {
        // A stair cracks over the EXACT quads the chunk mesher emitted for it
        // (same plane merge, same cell-local UVs), so the crack is one
        // continuous decal over the cut-out shape — no per-box tile restarts,
        // no buried faces.
        for face in crate::mesh::face::Face::ALL {
            for outer in [true, false] {
                let (quads, n) = crate::mesh::stair::plane_quads(shape, face, outer);
                for &(min, max) in quads.iter().take(n) {
                    super::item_cube::push_cell_local_face(
                        verts,
                        indices,
                        tile,
                        base,
                        1.0,
                        min,
                        max,
                        face,
                        super::lighting::DynLight::FULL,
                    );
                }
            }
        }
    } else if let Some(state) = view.slab_state {
        // A slab cracks over its meshed per-layer quads with the same
        // cell-local UVs, so the decal is cropped to the occupied halves (a
        // bottom slab's side shows the lower half of the crack tile, not a
        // squashed full tile) and shared mid faces of a full stack stay buried.
        for (slot, _) in crate::slab::layer_slots(state) {
            for face in crate::mesh::face::Face::ALL {
                let (quads, n) = crate::mesh::slab::layer_quads(state, slot, face);
                for &(min, max) in quads.iter().take(n) {
                    super::item_cube::push_cell_local_face(
                        verts,
                        indices,
                        tile,
                        base,
                        1.0,
                        min,
                        max,
                        face,
                        super::lighting::DynLight::FULL,
                    );
                }
            }
        }
    } else if let Some(mask) = view.pane_mask {
        // A pane cracks over the SAME post/arm faces the chunk mesher emitted
        // (cell-local UVs), so the crack reads as a full block's decal with the
        // open parts cut out — like stairs and slabs, not a box around the cell.
        crate::mesh::pane::shape_faces(mask, |min, max, face, _, _, _| {
            super::item_cube::push_cell_local_face(
                verts,
                indices,
                tile,
                base,
                1.0,
                min,
                max,
                face,
                super::lighting::DynLight::FULL,
            );
        });
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
                    min,
                    max,
                    super::lighting::DynLight::FULL,
                );
            }
            None => push_cube_textured(verts, indices, [tile; 3], base, 1.0),
        }
    }
    indices.len() as u32
}

fn transform_box(m: glam::Mat4, min: [f32; 3], max: [f32; 3]) -> (Vec3, Vec3) {
    let mn = Vec3::from(min);
    let mx = Vec3::from(max);
    let mut out_min = Vec3::splat(f32::INFINITY);
    let mut out_max = Vec3::splat(f32::NEG_INFINITY);
    for x in [mn.x, mx.x] {
        for y in [mn.y, mx.y] {
            for z in [mn.z, mx.z] {
                let p = m.transform_point3(Vec3::new(x, y, z));
                out_min = out_min.min(p);
                out_max = out_max.max(p);
            }
        }
    }
    (out_min, out_max)
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
            stair_shape: None,
            slab_state: None,
            pane_mask: None,
            model: None,
            stage: 4,
        };
        let n = build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert_eq!(n, 36);
        // Every face uses DestroyStage4 (tile id in bits 0..8 of packed).
        let want = Tile::from_name("destroy_stage_4").unwrap().index() as u8;
        for vert in &v {
            assert_eq!((vert.packed & 0xFF) as u8, want);
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

    /// A stair cracks over its meshed plane quads with cell-local UVs: one
    /// continuous decal over the cut-out shape, not a full tile per box face.
    #[test]
    fn stair_crack_uses_cell_local_uvs_over_the_meshed_quads() {
        use crate::mesh::face::Face;

        let block = IVec3::new(2, 60, 5);
        let shape = crate::stair::shape(crate::block_state::StairState::new(
            crate::facing::Facing::South,
            Default::default(),
        ));
        let view = BreakOverlayView {
            block,
            visual_box: None,
            stair_shape: Some(shape),
            slab_state: None,
            pane_mask: None,
            model: None,
            stage: 5,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_break_overlay(&view, &mut v, &mut i);

        // Exactly the mesher's quads: 4 verts per plane quad, nothing else.
        let quads: usize = Face::ALL
            .into_iter()
            .flat_map(|f| {
                [true, false].map(|outer| crate::mesh::stair::plane_quads(shape, f, outer).1)
            })
            .sum();
        assert_eq!(v.len(), quads * 4);

        for vert in &v {
            assert_eq!(
                (vert.packed >> crate::mesh::UV_MODE_SHIFT) & 0x7,
                crate::mesh::UV_MODE_CELL_LOCAL,
                "stair crack quads must carry cell-local UVs"
            );
            for a in 0..3 {
                let base = [block.x, block.y, block.z][a] as f32;
                assert!(
                    vert.pos[a] >= base - 1e-6 && vert.pos[a] <= base + 1.0 + 1e-6,
                    "crack vertex must stay on the stair cell"
                );
            }
        }

        // The full underside is ONE quad spanning the whole destroy tile, so the
        // crack does not restart per quadrant.
        let bottom: Vec<_> = v
            .iter()
            .filter(|vert| {
                // NegY faces only (shade index 3): side faces also touch y == 60.
                (vert.packed >> 10) & 0x3 == 3 && (vert.pos[1] - 60.0).abs() < 1e-6
            })
            .collect();
        assert_eq!(bottom.len(), 4, "underside crack must be a single quad");
        let mut uvs: Vec<(u32, u32)> = bottom
            .iter()
            .map(|vert| ((vert.packed2 >> 6) & 0x1F, (vert.packed2 >> 11) & 0x1F))
            .collect();
        uvs.sort_unstable();
        assert_eq!(uvs, vec![(0, 0), (0, 16), (16, 0), (16, 16)]);
    }

    /// A slab cracks over its meshed quads with cell-local UVs: the decal on a
    /// bottom slab's side is CROPPED to the lower half of the destroy tile,
    /// never a full tile squashed onto the half-height face.
    #[test]
    fn slab_crack_crops_the_tile_to_the_occupied_half() {
        use crate::block::Block;
        use crate::block_state::{SlabSplit, SlabState};

        let block = IVec3::new(-3, 70, 8);
        let state = SlabState::single(SlabSplit::Y, 0, Block::DirtSlab);
        let view = BreakOverlayView {
            block,
            visual_box: None,
            stair_shape: None,
            slab_state: Some(state),
            pane_mask: None,
            model: None,
            stage: 6,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        build_break_overlay(&view, &mut v, &mut i);

        // A lone bottom slab is six merged quads (top, bottom, four sides).
        assert_eq!(v.len(), 6 * 4);
        for vert in &v {
            assert_eq!(
                (vert.packed >> crate::mesh::UV_MODE_SHIFT) & 0x7,
                crate::mesh::UV_MODE_CELL_LOCAL,
                "slab crack quads must carry cell-local UVs"
            );
            assert!(
                vert.pos[1] <= 70.5 + 1e-6,
                "bottom slab crack must stay on the lower half"
            );
            // Side-face verts (X or Z shade groups) sit in the cell's lower
            // half, so their cell-local v spans 8..=16 — the lower half of the
            // tile — instead of restarting at 0 (which would stretch the decal).
            let shade = (vert.packed >> 10) & 0x3;
            if shade == 1 || shade == 2 {
                let v16 = (vert.packed2 >> 11) & 0x1F;
                assert!(
                    (8..=16).contains(&v16),
                    "side crack v16 = {v16} must be cropped to the lower tile half"
                );
            }
        }
    }

    #[test]
    fn model_overlay_cracks_each_cube_surface_within_the_outline() {
        use crate::block_model::BlockModelKind;
        // Mining a workbench cell: the crack must paint the whole model's actual cube
        // surfaces (many boxes — legs/body/top), not one coarse box, and every quad must
        // sit inside the model's world outline (never floating in the cell's empty air).
        // Targeting a non-zero authored cell (offset [1,1,0]) also pins anchoring.
        let kind = BlockModelKind::FurnitureWorkbench;
        let offset = [1u8, 1, 0];
        let block = IVec3::new(10, 64, -3);
        let all = crate::block_model::model_render_boxes(kind);
        // Only the STRUCTURAL cubes crack — small decoration cubes are filtered out by
        // `MIN_CRACK_EXTENT`, so the cracked set is a non-empty strict subset.
        let cracked = all
            .iter()
            .filter(|b| {
                let e = [
                    b.max[0] - b.min[0],
                    b.max[1] - b.min[1],
                    b.max[2] - b.min[2],
                ];
                e[0].max(e[1]).max(e[2]) >= MIN_CRACK_EXTENT
            })
            .count();
        assert!(cracked > 1, "several structural surfaces still crack");
        assert!(cracked < all.len(), "tiny decoration cubes are skipped");

        let view = BreakOverlayView {
            block,
            visual_box: None,
            stair_shape: None,
            slab_state: None,
            pane_mask: None,
            model: Some((kind, offset, crate::block_model::DEFAULT_MODEL_FACING)),
            stage: 3,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_break_overlay(&view, &mut v, &mut i);
        // One textured box (24 verts / 36 indices) per cracked (structural) cube surface.
        assert_eq!(v.len(), cracked * 24);
        assert_eq!(n as usize, cracked * 36);
        // The whole-model crack geometry MUST fit the break-overlay GPU buffer, or the bake
        // overflows and the crack silently vanishes (the "not visible in-game" bug). Pins
        // the buffer is sized for a multi-cube model, not one cube.
        assert!(
            v.len() as u64 <= super::super::pipeline::MAX_BREAK_VERTICES,
            "model crack geometry ({} verts) overflows the break buffer ({})",
            v.len(),
            super::super::pipeline::MAX_BREAK_VERTICES
        );
        assert!(n as u64 <= super::super::pipeline::MAX_BREAK_INDICES);

        // Every crack vertex lies within the model's world-space outline box — i.e. on
        // the model, never out in the air.
        let (omn, omx) = crate::block_model::outline_bounds(kind);
        let base = crate::block_model::base_from_cell(
            block,
            kind,
            offset,
            crate::block_model::DEFAULT_MODEL_FACING,
        );
        let origin = [base.x as f32, base.y as f32, base.z as f32];
        for vert in &v {
            for a in 0..3 {
                assert!(
                    vert.pos[a] >= origin[a] + omn[a] - 1e-3
                        && vert.pos[a] <= origin[a] + omx[a] + 1e-3,
                    "crack vertex axis {a} = {} outside the model outline",
                    vert.pos[a]
                );
            }
        }
    }

    /// Visual preview (NOT an assertion): rasterizes the workbench model + its FILTERED
    /// break-crack boxes (real destroy texture, shared z-buffer, LessEqual+bias multiply —
    /// the in-game relationship) so the `MIN_CRACK_EXTENT` threshold can be tuned by eye.
    /// Run: `cargo test --lib -- --ignored --nocapture render_break_overlay_preview`.
    /// Writes /tmp/break_overlay.png.
    #[test]
    #[ignore = "visual preview harness; writes /tmp/break_overlay.png"]
    fn render_break_overlay_preview() {
        use crate::bbmodel::euler_quat;
        use crate::block_model::{self, BlockModelKind};
        use crate::mesh::face::Face;
        use crate::mesh::SHADES;
        use glam::{Mat4, Vec3};

        let kind = BlockModelKind::FurnitureWorkbench;
        const W: usize = 480;
        let inst = block_model::instance(kind);
        let (atlas, aw, ah) = block_model::atlas().texture();
        let destroy = image::open(format!(
            "{}/assets/textures/destroy_stage_5.png",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("destroy texture")
        .to_rgba8();
        let (dw, dh) = destroy.dimensions();

        // iso view of the footprint (front-3/4 so the desk top / legs / board read).
        let fp = block_model::footprint(kind);
        let center = Vec3::new(fp[0] as f32, fp[1] as f32, fp[2] as f32) * 0.5;
        let rotm = Mat4::from_quat(euler_quat(Vec3::new(28.0, 330.0, 0.0)));
        let (mut half, mut half_z) = (1e-3f32, 1e-3f32);
        for &x in &[0.0, fp[0] as f32] {
            for &y in &[0.0, fp[1] as f32] {
                for &z in &[0.0, fp[2] as f32] {
                    let p = rotm.transform_point3(Vec3::new(x, y, z) - center);
                    half = half.max(p.x.abs()).max(p.y.abs());
                    half_z = half_z.max(p.z.abs());
                }
            }
        }
        let mvp = Mat4::from_translation(Vec3::new(0.0, 0.0, 0.5))
            * Mat4::from_scale(Vec3::new(0.9 / half, 0.9 / half, 0.45 / half_z))
            * rotm
            * Mat4::from_translation(-center);

        let mut color = vec![0u8; W * W * 3];
        for px in color.chunks_mut(3) {
            px.copy_from_slice(&[120, 150, 170]); // sky-ish so dark cracks read
        }
        let mut zbuf = vec![f32::INFINITY; W * W];
        let project = |p: Vec3| -> [f32; 3] {
            let c = mvp * p.extend(1.0);
            [
                (c.x * 0.5 + 0.5) * W as f32,
                (1.0 - (c.y * 0.5 + 0.5)) * W as f32,
                c.z,
            ]
        };

        // 1) the model (model atlas), depth write.
        for cube in &inst.cubes {
            let m = Mat4::from_translation(cube.origin)
                * Mat4::from_quat(euler_quat(cube.rotation))
                * Mat4::from_translation(-cube.origin);
            for (slot, face) in Face::ALL.into_iter().enumerate() {
                let Some([u0, v0, u1, v1]) = cube.faces[slot] else {
                    continue;
                };
                let lc = face.quad_box(cube.from.to_array(), cube.to.to_array());
                let sc = lc.map(|p| project(m.transform_point3(Vec3::from(p))));
                let shade = SHADES[face.shade_idx() as usize];
                raster_quad(
                    &mut color,
                    &mut zbuf,
                    W,
                    sc,
                    [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
                    atlas,
                    aw,
                    ah,
                    shade,
                    false,
                    0.0,
                );
            }
        }
        // 2) the FILTERED crack boxes (destroy tile), LessEqual+bias multiply, no z write.
        for b in block_model::model_render_boxes(kind) {
            let e = [
                b.max[0] - b.min[0],
                b.max[1] - b.min[1],
                b.max[2] - b.min[2],
            ];
            if e[0].max(e[1]).max(e[2]) < MIN_CRACK_EXTENT {
                continue;
            }
            for face in Face::ALL {
                let lc = face.quad_box(b.min, b.max);
                let sc = lc.map(|p| project(Vec3::from(p)));
                let [du, dv] = [(dw - 1) as f32 / dw as f32, (dh - 1) as f32 / dh as f32];
                raster_quad(
                    &mut color,
                    &mut zbuf,
                    W,
                    sc,
                    [[0.0, dv], [du, dv], [du, 0.0], [0.0, 0.0]],
                    destroy.as_raw(),
                    dw,
                    dh,
                    1.0,
                    true,
                    0.004,
                );
            }
        }
        image::save_buffer(
            "/tmp/break_overlay.png",
            &color,
            W as u32,
            W as u32,
            image::ColorType::Rgb8,
        )
        .expect("save");
        let total = block_model::model_render_boxes(kind).len();
        let kept = block_model::model_render_boxes(kind)
            .iter()
            .filter(|b| {
                let e = [
                    b.max[0] - b.min[0],
                    b.max[1] - b.min[1],
                    b.max[2] - b.min[2],
                ];
                e[0].max(e[1]).max(e[2]) >= MIN_CRACK_EXTENT
            })
            .count();
        println!("wrote /tmp/break_overlay.png  (MIN_CRACK_EXTENT={MIN_CRACK_EXTENT}: {kept}/{total} cubes cracked)");
    }

    /// Rasterize one textured quad (4 screen corners + 4 UVs) into `color`/`zbuf` with an
    /// alpha cutout. `multiply=false` is the opaque model (depth `<`, writes z); `true` is
    /// the crack decal (depth `<=` with `bias` toward camera, MULTIPLY blend, no z write).
    #[allow(clippy::too_many_arguments)]
    fn raster_quad(
        color: &mut [u8],
        zbuf: &mut [f32],
        w: usize,
        s: [[f32; 3]; 4],
        uv: [[f32; 2]; 4],
        tex: &[u8],
        tw: u32,
        th: u32,
        shade: f32,
        multiply: bool,
        bias: f32,
    ) {
        for tri in [[0usize, 1, 2], [0, 2, 3]] {
            let (a, b, c) = (s[tri[0]], s[tri[1]], s[tri[2]]);
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if area.abs() < 1e-6 {
                continue;
            }
            let inv = 1.0 / area;
            let minx = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let maxx = a[0].max(b[0]).max(c[0]).ceil().min(w as f32 - 1.0) as usize;
            let miny = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let maxy = a[1].max(b[1]).max(c[1]).ceil().min(w as f32 - 1.0) as usize;
            for y in miny..=maxy {
                for x in minx..=maxx {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b[0] - px) * (c[1] - py) - (c[0] - px) * (b[1] - py)) * inv;
                    let w1 = ((c[0] - px) * (a[1] - py) - (a[0] - px) * (c[1] - py)) * inv;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                    let idx = y * w + x;
                    let pass = if multiply {
                        z - bias <= zbuf[idx]
                    } else {
                        z < zbuf[idx]
                    };
                    if !pass {
                        continue;
                    }
                    let tu = w0 * uv[tri[0]][0] + w1 * uv[tri[1]][0] + w2 * uv[tri[2]][0];
                    let tv = w0 * uv[tri[0]][1] + w1 * uv[tri[1]][1] + w2 * uv[tri[2]][1];
                    let sx = (tu * tw as f32).clamp(0.0, tw as f32 - 1.0) as u32;
                    let sy = (tv * th as f32).clamp(0.0, th as f32 - 1.0) as u32;
                    let ti = ((sy * tw + sx) * 4) as usize;
                    if tex[ti + 3] < 128 {
                        continue;
                    }
                    let o = idx * 3;
                    if multiply {
                        for k in 0..3 {
                            color[o + k] = (color[o + k] as f32 * tex[ti + k] as f32 / 255.0) as u8;
                        }
                    } else {
                        zbuf[idx] = z;
                        for k in 0..3 {
                            color[o + k] = (tex[ti + k] as f32 * shade).min(255.0) as u8;
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn reuses_buffers() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = BreakOverlayView {
            block: IVec3::ZERO,
            visual_box: None,
            stair_shape: None,
            slab_state: None,
            pane_mask: None,
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
}
