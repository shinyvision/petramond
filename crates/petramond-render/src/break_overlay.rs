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
use petramond_mesh::Vertex;
use petramond_world::tile::Tile;

/// Skip cracking a bbmodel cube whose LARGEST dimension is below this (in blocks).
const MIN_CRACK_EXTENT: f32 = 1.0;

/// The destroy tile for crack `stage` (clamped 0..=9), as a [`Tile`].
#[inline]
fn destroy_tile(stage: u8) -> Tile {
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
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    for view in views {
        append_break_overlay(view, verts, indices);
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
    build_break_overlays(std::slice::from_ref(view), verts, indices)
}

/// Append the crack overlay geometry for `view` (indices are vert-relative, so
/// composition is plain concatenation). All faces use the same destroy tile so
/// the crack reads from every angle. A plain cube cracks over its six cell
/// faces; a stair or slab over its meshed cell-local quads; a chest over its
/// inset box; a bbmodel block over its structural model cubes.
///
/// The cube spans the block's exact `[block, block + 1]` cell with no inflation,
/// so each face lands on the same integer-coordinate plane the chunk mesh emitted
/// for that block. The pipeline's depth `LessEqual` + a small polygon offset
/// (`BREAK_DEPTH_BIAS`) put the crack on the surface without z-fighting (see the
/// module docs for why the offset is needed).
fn append_break_overlay(view: &BreakOverlayView, verts: &mut Vec<Vertex>, indices: &mut Vec<u32>) {
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
        let model_base =
            petramond_world::block_model::base_from_cell(view.block, kind, offset, facing);
        let placement = petramond_world::block_model::placement_transform(model_base, kind, facing);
        for b in petramond_world::block_model::model_render_boxes(kind) {
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
    } else if let Some(cb) = view.shape_boxes {
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
                super::item_cube::push_cell_local_face(
                    verts,
                    indices,
                    tile,
                    base,
                    1.0,
                    b.min,
                    b.max,
                    face,
                    super::lighting::DynLight::FULL,
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
                    min,
                    max,
                    super::lighting::DynLight::FULL,
                );
            }
            None => push_cube_textured(verts, indices, [tile; 3], base, 1.0),
        }
    }
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
        }; MAX_CRACK_BOXES];
        for (dst, &(min, max, faces)) in arr.iter_mut().zip(boxes) {
            *dst = CrackBox { min, max, faces };
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
    fn model_overlay_cracks_each_cube_surface_within_the_outline() {
        use petramond_world::block_model::BlockModelKind;
        // Mining a workbench cell: the crack must paint the whole model's actual cube
        // surfaces (many boxes — legs/body/top), not one coarse box, and every quad must
        // sit inside the model's world outline (never floating in the cell's empty air).
        // Targeting a non-zero authored cell (offset [1,1,0]) also pins anchoring.
        let kind = BlockModelKind::FurnitureWorkbench;
        let offset = [1u8, 1, 0];
        let block = IVec3::new(10, 64, -3);
        let all = petramond_world::block_model::model_render_boxes(kind);
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
            shape_boxes: None,
            model: Some((
                kind,
                offset,
                petramond_world::block_model::DEFAULT_MODEL_FACING,
            )),
            stage: 3,
        };
        let mut v = Vec::new();
        let mut i = Vec::new();
        let n = build_break_overlay(&view, &mut v, &mut i);
        // One textured box (24 verts / 36 indices) per cracked (structural) cube surface.
        assert_eq!(v.len(), cracked * 24);
        assert_eq!(n as usize, cracked * 36);

        // Every crack vertex lies within the model's world-space outline box — i.e. on
        // the model, never out in the air.
        let (omn, omx) = petramond_world::block_model::outline_bounds(kind);
        let base = petramond_world::block_model::base_from_cell(
            block,
            kind,
            offset,
            petramond_world::block_model::DEFAULT_MODEL_FACING,
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
        use glam::{Mat4, Vec3};
        use petramond_math::face::Face;
        use petramond_mesh::SHADES;
        use petramond_world::bbmodel::{display_euler_quat, euler_quat};
        use petramond_world::block_model::{self, BlockModelKind};

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
        let rotm = Mat4::from_quat(display_euler_quat(Vec3::new(28.0, 330.0, 0.0)));
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
                let Some(face_uv) = cube.faces[slot] else {
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
                    face_uv.corner_uv(),
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
