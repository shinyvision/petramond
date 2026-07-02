//! Per-slot item icon placement + stack-count digits.
//!
//! Item icons are no longer baked as live 3D geometry every frame: each item's
//! icon is rendered ONCE at renderer init into a cell of an icon-atlas texture
//! (see `render::renderer::icon_atlas`), and a slot then draws a single 2D
//! textured quad sampling its cell. So this module just RECORDS which item goes in
//! which slot rect (into [`super::UiBuild::icon_quads`]); the renderer resolves
//! each entry to its atlas cell and emits the quad. The MVP projections +
//! per-render-kind geometry builders that the one-time bake uses live here too
//! (exposed `pub(crate)` so the bake can call them), plus the tiny bitmap-font
//! count drawn over an icon.

use glam::{Mat4, Vec3};

use super::super::ui_text;
use super::{pixel_to_ndc, push_solid, UiBuild, UiVertex};
use crate::gui::SlotRect;
use crate::item::ItemType;

/// Record that `item` occupies slot pixel rect `r` this frame. The renderer
/// resolves it to the item's pre-baked icon-atlas cell and emits a textured quad
/// (no per-frame 3D geometry). Shared by every screen's slot draw + the drag
/// cursor. (`screen` is accepted for call-site symmetry with [`push_count`] but is
/// unused — the atlas cell UV is screen-independent; the slot→NDC mapping happens
/// in the renderer.)
pub(super) fn push_slot_icon(
    build: &mut UiBuild,
    _screen: (u32, u32),
    item: ItemType,
    r: SlotRect,
) {
    build.icon_quads.push((item, r));
}

/// Record `item` as a GREYED (semi-transparent) slot icon — a furniture-workbench
/// result the placed block can't yet make. Drawn like [`push_slot_icon`] but with a
/// reduced alpha by the renderer (see `UiBuild::dim_icon_quads`).
pub(super) fn push_dim_slot_icon(
    build: &mut UiBuild,
    _screen: (u32, u32),
    item: ItemType,
    r: SlotRect,
) {
    build.dim_icon_quads.push((item, r));
}

/// The slot's center in NDC (y up). Shared by the iso + flat icon MVPs so an icon
/// is always anchored at the geometric centre of its slot rect.
fn slot_ndc_center(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    pixel_to_ndc(screen, r.x + r.w * 0.5, r.y + r.h * 0.5)
}

/// The slot's ANISOTROPIC clip-space half-extents `[hx, hy]` in NDC. NDC spans 2
/// units across BOTH the framebuffer width and height, but the framebuffer is not
/// square (e.g. 16:9), so a pixel size `p` maps to a DIFFERENT NDC extent on each
/// axis: `p/w*2` horizontally vs `p/h*2` vertically. To draw an on-screen SQUARE
/// of `p` pixels the clip-space scale MUST therefore differ per axis — the
/// half-extents are `[p/w, p/h]` (a uniform single factor would render wider than
/// tall on a 16:9 screen, squishing the icon). The on-screen pixel extent is
/// `hx*w == hy*h == p` on both axes, so the icon is square at any aspect ratio.
/// Because every slot shares the same interior pixel size, this returns the SAME
/// pair for every slot — per-slot MVPs still differ only by the centre translation.
fn ndc_half_extents(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [r.w / w, r.h / h]
}

/// Isometric orthographic MVP mapping a unit cube (centered on origin, spanning
/// ±0.5) into the slot's NDC rect, back-face culled so the iso view reads with NO
/// depth buffer (front faces overdraw back faces in submission order). Called by
/// the one-time icon-atlas bake (with a 64×64 square cell `r`).
pub(crate) fn iso_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    // Classic MC item iso: rotate +30° about X then 45° about Y. With the model3d
    // pipeline's CCW-front / back-face cull, this makes the cube's TOP face plus
    // two sides (NegX + PosZ) face the +Z viewer and the BOTTOM cull away — the
    // camera looks DOWN at the cube. (A -30° X tilt would invert this and show the
    // BOTTOM + sides, which is the bug we are fixing.)
    let rot = Mat4::from_rotation_x(30f32.to_radians()) * Mat4::from_rotation_y(45f32.to_radians());
    // A unit cube rotated this way spans ~sqrt(2) ≈ 1.414 across; scale so it fills
    // ~0.9 of the slot. The clip-space scale is ANISOTROPIC (`sx != sy` on a
    // non-square framebuffer): each axis uses its own NDC half-extent so the cube
    // renders as an on-screen SQUARE of slot pixels at any aspect ratio. A single
    // uniform factor here would stretch the cube wider than tall on a 16:9 screen.
    let model_half = std::f32::consts::SQRT_2 * 0.5; // ~0.707
    let fill = 0.9;
    let [hx, hy] = ndc_half_extents(screen, r);
    let sx = hx * fill / model_half;
    let sy = hy * fill / model_half;
    // Map: clip = center + rotated_pos * scale, with y up. The cube has no depth
    // attachment, but the rasterizer still clips on clip-z in [0, 1] (wgpu), so
    // translate z to 0.5 and compress the rotated cube's z-extent into a tiny band
    // so it always stays inside [0, 1] regardless of slot size.
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(sx, sy, sx * 0.05))
        * rot
}

/// The GUI icon orientation for an authored Blockbench `display.gui` rotation, exactly
/// as Blockbench's GUI preview shows it. Blockbench display eulers act in a frame that
/// is horizontally MIRRORED relative to ours, so the authored rotation converts by
/// negating Y and Z; the preview camera then differs from our icon camera by a plain
/// 180° yaw. Verified against Blockbench per-model with the preview harness
/// ([`render_model_icon_preview`](tests::render_model_icon_preview)) — the icon is
/// DATA-DRIVEN: editing the `gui` pose in Blockbench (then recompiling, which re-bakes
/// the `.llblock`) moves the icon with no code change. The horizontal mirror to match
/// Blockbench's handedness is applied separately in [`icon_mvp_for_rot`] (the negative
/// X scale). NOTE: the first-person hand context does NOT mirror its euler — see
/// `render::hand::held_model`.
fn gui_rotation(kind: crate::block_model::BlockModelKind) -> glam::Quat {
    let r = crate::block_model::display(kind).gui.rotation;
    glam::Quat::from_rotation_y(std::f32::consts::PI)
        * crate::bbmodel::euler_quat(Vec3::new(r[0], -r[1], -r[2]))
}

/// Icon MVP for a bbmodel block: the authored Blockbench `display.gui` pose mapped through
/// [`gui_rotation`] into our icon camera, auto-framed to fill ~0.9 of the slot (square on
/// screen at any aspect, like [`iso_icon_mvp`]). `build_block_model_icon` bakes it into
/// the geometry; the model-icon pass depth-tests so panels/drawers order correctly.
/// Called by the one-time icon-atlas bake (with a 64×64 square cell `r`).
pub(crate) fn model_icon_mvp(
    screen: (u32, u32),
    r: SlotRect,
    kind: crate::block_model::BlockModelKind,
) -> Mat4 {
    icon_mvp_for_rot(screen, r, kind, gui_rotation(kind))
}

/// [`model_icon_mvp`] with an explicit orientation quaternion (factored out so the visual
/// preview harness can try candidate angles).
fn icon_mvp_for_rot(
    screen: (u32, u32),
    r: SlotRect,
    kind: crate::block_model::BlockModelKind,
    rot_quat: glam::Quat,
) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    let [hx, hy] = ndc_half_extents(screen, r);
    let rot = Mat4::from_quat(rot_quat);
    // The model's centred-unit bounds (matching `build_block_model_icon`'s centring:
    // subtract the footprint centre, divide by the largest footprint axis).
    let fp = crate::block_model::footprint(kind);
    let fpv = Vec3::new(fp[0] as f32, fp[1] as f32, fp[2] as f32);
    let span = fpv.max_element().max(1.0);
    let (bmn, bmx) = crate::block_model::outline_bounds(kind);
    // Largest |x|/|y| (slot fit) and |z| (depth fit) of the 8 rotated corners.
    let mut half = 1e-3f32;
    let mut half_z = 1e-3f32;
    for &cx in &[bmn[0], bmx[0]] {
        for &cy in &[bmn[1], bmx[1]] {
            for &cz in &[bmn[2], bmx[2]] {
                let centred = (Vec3::new(cx, cy, cz) - fpv * 0.5) / span;
                let p = rot.transform_point3(centred);
                half = half.max(p.x.abs()).max(p.y.abs());
                half_z = half_z.max(p.z.abs());
            }
        }
    }
    let fill = 1.0;
    let sx = hx * fill / half;
    let sy = hy * fill / half;
    // Map the model's depth into clip-z [0.1, 0.9] (centred at 0.5) so the icon's own
    // depth buffer resolves the draw order — the model is double-sided like the in-world
    // block, so without depth a painter sort can't order its panels/drawers correctly.
    let sz = 0.4 / half_z;
    // NEGATIVE X scale: mirror horizontally to match Blockbench's GUI preview handedness
    // (Minecraft's GUI base flips an axis; our render is otherwise the mirror image).
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(-sx, sy, sz))
        * rot
}

/// Orthographic MVP mapping the flat (X/Y plane) billboard quad (spanning ±0.5)
/// into the slot's NDC rect, facing the viewer. Called by the one-time icon-atlas
/// bake (with a 64×64 square cell `r`).
pub(crate) fn flat_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    // Flat sprite fills the slot. The billboard spans ±0.5 in model space, so a
    // per-axis scale of `2 × half_extent` maps it onto the full slot square. The
    // scale is ANISOTROPIC (`sx != sy` on a non-square framebuffer) so the sprite
    // renders as an on-screen SQUARE of slot pixels at any aspect ratio — a single
    // uniform factor would squish it wider than tall on a 16:9 screen.
    let [hx, hy] = ndc_half_extents(screen, r);
    let sx = hx * 2.0;
    let sy = hy * 2.0;
    // z translated to 0.5 so the flat quad sits inside the [0, 1] clip-z band.
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(sx, sy, 0.05))
}

/// Append the stack-count digits for `count` at the bottom-right of slot `r`.
/// Drawn as small solid white quads (one per lit font cell) with a 1px-offset
/// dark drop shadow for legibility, using the tiny 3×5 bitmap font.
pub(super) fn push_count(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    count: u32,
    r: SlotRect,
    scale: f32,
) {
    // Font "pixel" size: scale up so digits read at the chosen GUI scale.
    let fp = scale.max(1.0);
    let num_w = ui_text::number_width(count) as f32 * fp;
    let num_h = ui_text::GLYPH_H as f32 * fp;
    // Bottom-right corner of the slot, nudged in by ~1 font-pixel.
    let x0 = r.x + r.w - num_w - fp * 0.0;
    let y0 = r.y + r.h - num_h - fp * 0.0;
    let shadow = [0.0, 0.0, 0.0, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
    // Drop shadow first (offset by 1 font-pixel down-right), then the glyphs.
    ui_text::for_each_lit_cell(count, |px, py| {
        let cx = x0 + px as f32 * fp;
        let cy = y0 + py as f32 * fp;
        push_solid(out, screen, cx + fp, cy + fp, fp, fp, shadow);
    });
    ui_text::for_each_lit_cell(count, |px, py| {
        let cx = x0 + px as f32 * fp;
        let cy = y0 + py as f32 * fp;
        push_solid(out, screen, cx, cy, fp, fp, white);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::gui_scale;

    /// A simple on-screen slot rect for exercising the icon MVPs. Real slot
    /// positions come from baked manifests; the icon projection only needs a rect,
    /// distinct per index so the per-slot-translation test has two cells to compare.
    fn slot_rect(i: usize, _screen: (u32, u32), _open: bool, scale: f32) -> Option<SlotRect> {
        let s = super::super::SLOT_PX * scale;
        let col = (i % 9) as f32;
        let row = (i / 9) as f32;
        Some(SlotRect {
            x: 20.0 * scale + col * (s + 2.0),
            y: 20.0 * scale + row * (s + 2.0),
            w: s,
            h: s,
        })
    }

    #[test]
    fn iso_mvp_keeps_cube_within_clip_xy() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        let r = slot_rect(0, screen, false, scale).unwrap();
        let mvp = iso_icon_mvp(screen, r);
        // The 8 cube corners must land inside [-1, 1] in NDC x/y and clip-z [0, 1].
        for &x in &[-0.5f32, 0.5] {
            for &y in &[-0.5f32, 0.5] {
                for &z in &[-0.5f32, 0.5] {
                    let c = mvp * glam::Vec4::new(x, y, z, 1.0);
                    assert!(c.x.abs() <= 1.0 + 1e-3, "x {} out of clip", c.x);
                    assert!(c.y.abs() <= 1.0 + 1e-3, "y {} out of clip", c.y);
                    assert!((0.0..=1.0).contains(&c.z), "z {} out of clip-z band", c.z);
                }
            }
        }
    }

    /// A bbmodel-block icon bakes its REAL model (not a placeholder) framed inside the
    /// slot: the geometry must be non-empty and every vertex must land within the slot's
    /// NDC rect with clip-z in `[0, 1]` (a depthless pass still clips on z). Catches a
    /// mis-scaled / off-centre / empty icon without a GPU (the in-game look is confirmed
    /// by playtest).
    #[test]
    fn model_icon_bakes_the_real_model_into_the_slot() {
        use crate::block_model::BlockModelKind;
        let screen = (1280u32, 720u32);
        let r = slot_rect(0, screen, false, gui_scale(screen)).unwrap();
        let kind = BlockModelKind::FurnitureWorkbench;
        let mvp = model_icon_mvp(screen, r, kind);
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        super::super::super::item_model::build_block_model_icon(
            kind,
            mvp,
            &mut verts,
            &mut indices,
        );
        assert!(!verts.is_empty(), "the real model must bake into geometry");
        assert_eq!(indices.len() % 6, 0, "indexed quads");

        let center = slot_ndc_center(screen, r);
        let [hx, hy] = ndc_half_extents(screen, r);
        for v in &verts {
            // Positions are already clip space (the MVP is baked into them).
            assert!(
                (v.pos[0] - center[0]).abs() <= hx + 1e-3,
                "icon vertex x {} escapes the slot",
                v.pos[0]
            );
            assert!(
                (v.pos[1] - center[1]).abs() <= hy + 1e-3,
                "icon vertex y {} escapes the slot",
                v.pos[1]
            );
            assert!(
                (0.0..=1.0).contains(&v.pos[2]),
                "icon vertex z {} outside the clip-z band",
                v.pos[2]
            );
        }
    }

    /// Visual preview harness (NOT an assertion): rasterizes the bbmodel-block ICON via
    /// the REAL `model_icon_mvp` + `build_block_model_icon` geometry, with a z-buffer and
    /// model-atlas sampling — i.e. exactly what the depth-tested `model_icon` GPU pipeline
    /// does — to a PNG, so the orientation/draw-order can be confirmed without launching
    /// the game. Run: `cargo test --lib -- --ignored --nocapture render_model_icon_preview`.
    /// Writes /tmp/model_icon.png.
    #[test]
    #[ignore = "visual preview harness; run explicitly to regenerate /tmp/model_icon.png"]
    fn render_model_icon_preview() {
        use crate::block_model::{self, BlockModelKind};
        use glam::Quat;

        let (atlas_rgba, aw, ah) = block_model::atlas().texture();

        // The SHIPPED production rotation per BLOCK kind (item-only kinds have no gui
        // pose worth previewing): the authored `gui` pose mapped through
        // `gui_rotation`. `icon_mvp_for_rot` adds the horizontal mirror, so this
        // preview matches the in-game icon AND Blockbench's GUI display preview.
        // (Editing the `gui` pose in Blockbench moves it.) To compare CANDIDATE
        // mappings (e.g. when calibrating the hand pass — the icon shows
        // std(RotY180 · rot), so preview a hand rotation Q by passing
        // rot = RotY180 · Q), swap the quats below.
        let candidates: Vec<(String, BlockModelKind, Quat)> = [
            BlockModelKind::FurnitureWorkbench,
            BlockModelKind::BedFrame,
            BlockModelKind::Bed,
        ]
        .into_iter()
        .map(|kind| (format!("{kind:?}"), kind, gui_rotation(kind)))
        .collect();

        const CELL: usize = 460;
        let cols = 3usize;
        let rows = candidates.len().div_ceil(cols);
        let (gw, gh) = (cols * CELL, rows * CELL);
        let bg = [38u8, 38, 46];
        let mut color = vec![0u8; gw * gh * 3];
        for px in color.chunks_mut(3) {
            px.copy_from_slice(&bg);
        }

        for (i, (label, kind, q)) in candidates.iter().enumerate() {
            let kind = *kind;
            let (cx, cy) = ((i % cols) * CELL, (i / cols) * CELL);
            let r = SlotRect {
                x: 0.0,
                y: 0.0,
                w: CELL as f32,
                h: CELL as f32,
            };
            let mvp = icon_mvp_for_rot((CELL as u32, CELL as u32), r, kind, *q);
            let mut verts = Vec::new();
            let mut indices = Vec::new();
            super::super::super::item_model::build_block_model_icon(
                kind,
                mvp,
                &mut verts,
                &mut indices,
            );
            let mut zbuf = vec![f32::INFINITY; CELL * CELL];
            let project = |p: [f32; 3]| -> [f32; 3] {
                [
                    (p[0] * 0.5 + 0.5) * CELL as f32,
                    (1.0 - (p[1] * 0.5 + 0.5)) * CELL as f32,
                    p[2],
                ]
            };
            for tri in indices.chunks_exact(3) {
                let vtx = [
                    verts[tri[0] as usize],
                    verts[tri[1] as usize],
                    verts[tri[2] as usize],
                ];
                let s = [
                    project(vtx[0].pos),
                    project(vtx[1].pos),
                    project(vtx[2].pos),
                ];
                let (x0, y0, x1, y1, x2, y2) =
                    (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
                let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
                if area.abs() < 1e-6 {
                    continue;
                }
                let inv_area = 1.0 / area;
                let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
                let maxx = x0.max(x1).max(x2).ceil().min(CELL as f32 - 1.0) as usize;
                let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
                let maxy = y0.max(y1).max(y2).ceil().min(CELL as f32 - 1.0) as usize;
                for y in miny..=maxy {
                    for x in minx..=maxx {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) * inv_area;
                        let w1 = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) * inv_area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                        let li = y * CELL + x;
                        if z >= zbuf[li] {
                            continue;
                        }
                        let u = w0 * vtx[0].uv[0] + w1 * vtx[1].uv[0] + w2 * vtx[2].uv[0];
                        let v = w0 * vtx[0].uv[1] + w1 * vtx[1].uv[1] + w2 * vtx[2].uv[1];
                        let tx = (u * aw as f32).clamp(0.0, aw as f32 - 1.0) as u32;
                        let ty = (v * ah as f32).clamp(0.0, ah as f32 - 1.0) as u32;
                        let ti = ((ty * aw + tx) * 4) as usize;
                        if atlas_rgba[ti + 3] < 128 {
                            continue;
                        }
                        let shade = w0 * vtx[0].shade + w1 * vtx[1].shade + w2 * vtx[2].shade;
                        zbuf[li] = z;
                        let o = ((cy + y) * gw + (cx + x)) * 3;
                        color[o] = (atlas_rgba[ti] as f32 * shade).min(255.0) as u8;
                        color[o + 1] = (atlas_rgba[ti + 1] as f32 * shade).min(255.0) as u8;
                        color[o + 2] = (atlas_rgba[ti + 2] as f32 * shade).min(255.0) as u8;
                    }
                }
            }
            println!("cell {i}: {label}");
        }
        image::save_buffer(
            "/tmp/model_icon.png",
            &color,
            gw as u32,
            gh as u32,
            image::ColorType::Rgb8,
        )
        .expect("save png");
        println!("wrote /tmp/model_icon.png  ({cols}x{rows} grid)");
    }

    /// BUG 1: the iso view must show the cube's TOP face (camera looks DOWN at it),
    /// not the bottom. The fix flips the X tilt from -30° to +30°, which swaps which
    /// cube faces present toward the viewer. The model3d rasterizer decides facing
    /// by a normal's clip-z sign, so we assert (a) the SHIPPED iso orients the +Y
    /// (top) normal opposite to the -Y (bottom) normal, and (b) flipping the tilt
    /// sign swaps which of top/bottom carries the viewer-facing sign — pinning that
    /// the shipped +30° tilt presents the top where the old -30° presented the
    /// bottom. (Anchor-free: it only asserts the swap the fix makes.)
    #[test]
    fn iso_mvp_flips_to_show_top_face() {
        let screen = (1280u32, 720u32);
        let r = slot_rect(0, screen, false, gui_scale(screen)).unwrap();
        let shipped = iso_icon_mvp(screen, r); // +30° tilt (the fix)
        let z_of = |m: Mat4, n: Vec3| (m * n.extend(0.0)).z;
        // Within the shipped iso, top and bottom face opposite ways.
        assert!(
            z_of(shipped, Vec3::Y) * z_of(shipped, Vec3::NEG_Y) < 0.0,
            "top and bottom normals must point opposite ways in the iso view"
        );
        // The old (buggy) iso differed ONLY by the X-tilt sign. Rebuild it the same
        // way `iso_icon_mvp` composes, but with -30°, and confirm the tilt flip
        // swaps top/bottom: the shipped +30° top shares the old -30° bottom's sign.
        let center = slot_ndc_center(screen, r);
        let [hx, hy] = ndc_half_extents(screen, r);
        let model_half = std::f32::consts::SQRT_2 * 0.5;
        let sx = hx * 0.9 / model_half;
        let sy = hy * 0.9 / model_half;
        let rot_minus = Mat4::from_rotation_x(-(30f32.to_radians()))
            * Mat4::from_rotation_y(45f32.to_radians());
        let old = Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
            * Mat4::from_scale(Vec3::new(sx, sy, sx * 0.05))
            * rot_minus;
        assert_eq!(
            z_of(shipped, Vec3::Y).signum(),
            z_of(old, Vec3::NEG_Y).signum(),
            "the +30° fix must show the top where the old -30° showed the bottom"
        );
        assert_ne!(
            z_of(shipped, Vec3::Y).signum(),
            z_of(old, Vec3::Y).signum(),
            "flipping the tilt must flip the top face's viewer-facing sign"
        );
    }

    /// BUG 2: every slot's icon MVP must be IDENTICAL except for the per-slot
    /// centre translation — no accumulating drift, no per-slot scale/rotation
    /// difference. We strip the translation (set the last column to origin) from two
    /// different slots' MVPs and assert the remaining linear (rotation+scale) parts
    /// are equal, then assert the translations differ by exactly the slot-centre
    /// delta (so consecutive slots step by one slot pitch, not more).
    #[test]
    fn iso_mvp_differs_only_by_per_slot_translation() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        // Check across several slot pairs in both the closed hotbar and open grid.
        let check = |a: usize, b: usize, open: bool| {
            let ra = slot_rect(a, screen, open, scale).unwrap();
            let rb = slot_rect(b, screen, open, scale).unwrap();
            let ma = iso_icon_mvp(screen, ra);
            let mb = iso_icon_mvp(screen, rb);
            // Linear part (columns 0..3) must match exactly: identical scale + rot.
            for col in 0..3 {
                let ca = ma.col(col);
                let cb = mb.col(col);
                assert!(
                    (ca - cb).length() < 1e-6,
                    "linear column {col} differs between slots {a},{b} (open={open}): {ca:?} vs {cb:?}"
                );
            }
            // Translation column differs by exactly the slot-centre NDC delta.
            let ca = slot_ndc_center(screen, ra);
            let cb = slot_ndc_center(screen, rb);
            let dx = mb.col(3).x - ma.col(3).x;
            let dy = mb.col(3).y - ma.col(3).y;
            assert!(
                (dx - (cb[0] - ca[0])).abs() < 1e-6,
                "x translation drift slots {a},{b}"
            );
            assert!(
                (dy - (cb[1] - ca[1])).abs() < 1e-6,
                "y translation drift slots {a},{b}"
            );
        };
        // Adjacent hotbar slots + a far pair (closed): no progressive drift.
        check(0, 1, false);
        check(0, 8, false);
        // Open grid: a hotbar slot, an adjacent grid slot, and a far grid slot.
        check(0, 1, true);
        check(9, 10, true);
        check(0, 35, true);
    }

    /// BUG 1 (squish): the clip-space scale must be ANISOTROPIC so an icon renders as
    /// the SAME on-screen pixel shape at any framebuffer aspect ratio. NDC spans 2
    /// units across BOTH framebuffer axes, so to map a slot of P pixels to equal
    /// on-screen extents the half-extents MUST be `[P/w, P/h]` — a single uniform NDC
    /// factor (the wrong round-1 fix) would render wider than tall on a 16:9 screen.
    ///
    /// For the FLAT sprite the model is a planar ±0.5 quad, so "square on screen"
    /// is exact: its on-screen extents equal the slot's pixel size on both axes at
    /// any aspect. For the ISO cube the projected silhouette is naturally taller than
    /// wide (the +30° tilt foreshortens the horizontal diagonal), so squareness is
    /// asserted as ASPECT-INDEPENDENCE: the on-screen pixel footprint ratio is
    /// IDENTICAL across screen aspects (the round-1 uniform version stretched it by
    /// ~the framebuffer aspect ratio, ~2× between square and 2:1).
    #[test]
    fn icon_mvp_is_square_on_screen_at_any_aspect() {
        // The on-screen pixel x/y span of an MVP applied to a unit model (±0.5 cube
        // for iso, ±0.5 quad for flat), via the framebuffer dims (NDC 2 == full dim).
        let pixel_extents = |mvp: Mat4, screen: (u32, u32), three_d: bool| -> (f32, f32) {
            let (w, h) = (screen.0 as f32, screen.1 as f32);
            let zs: &[f32] = if three_d { &[-0.5, 0.5] } else { &[0.0] };
            let mut min = glam::Vec2::splat(f32::INFINITY);
            let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
            for &x in &[-0.5f32, 0.5] {
                for &y in &[-0.5f32, 0.5] {
                    for &z in zs {
                        let c = mvp * glam::Vec4::new(x, y, z, 1.0);
                        min = min.min(glam::Vec2::new(c.x, c.y));
                        max = max.max(glam::Vec2::new(c.x, c.y));
                    }
                }
            }
            ((max.x - min.x) * w * 0.5, (max.y - min.y) * h * 0.5)
        };
        // A fixed pixel-size slot so only the framebuffer aspect changes between runs.
        let r = SlotRect {
            x: 0.0,
            y: 0.0,
            w: 48.0,
            h: 48.0,
        };
        // Reference iso footprint aspect on a square screen.
        let (rx, ry) = pixel_extents(iso_icon_mvp((600, 600), r), (600, 600), true);
        let iso_ref_aspect = rx / ry;
        for &screen in &[(600u32, 600u32), (1280, 720), (1920, 1080)] {
            // Iso cube: on-screen footprint aspect is the SAME at every screen aspect.
            let (px, py) = pixel_extents(iso_icon_mvp(screen, r), screen, true);
            let aspect = px / py;
            assert!(
                (aspect - iso_ref_aspect).abs() < 1e-3,
                "iso icon footprint aspect changed with screen {screen:?}: {aspect} vs ref {iso_ref_aspect}"
            );
            // Flat sprite: the ±0.5 quad maps to the slot's exact 48px square — equal
            // on-screen pixel width/height (a true square) at any aspect.
            let (fx, fy) = pixel_extents(flat_icon_mvp(screen, r), screen, false);
            assert!(
                (fx - fy).abs() < 1e-3,
                "flat icon not square on screen {screen:?}: {fx}px wide vs {fy}px tall"
            );
            assert!(
                (fx - r.w).abs() < 1e-3,
                "flat icon px width {fx} != slot {}",
                r.w
            );
            assert!(
                (fy - r.h).abs() < 1e-3,
                "flat icon px height {fy} != slot {}",
                r.h
            );
        }
    }
}
