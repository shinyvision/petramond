use glam::{Mat4, Vec3};

use super::{pixel_to_ndc, push_solid, UiBuild, UiVertex};
use petramond::gui::SlotRect;
use petramond_text::tiny as ui_text;

pub(super) fn push_slot_icon(
    build: &mut UiBuild,
    _screen: (u32, u32),
    stack: &petramond_world::item::ItemStack,
    r: SlotRect,
) {
    push_stack_quads(&mut build.icon_quads, stack, r);
}

pub(super) fn push_stack_quads(
    quads: &mut Vec<(petramond_world::item::ItemType, SlotRect, [f32; 4], bool)>,
    stack: &petramond_world::item::ItemStack,
    r: SlotRect,
) {
    quads.push((stack.item, r, stack_tint(stack), stack_dyed(stack)));
    for overlay in petramond_world::item::variant::overlay_items(stack.variant) {
        quads.push((overlay, r, [1.0; 4], false));
    }
}

pub(super) fn stack_dyed(stack: &petramond_world::item::ItemStack) -> bool {
    petramond_world::item::variant::tint(stack.variant).is_some()
}

pub(super) fn stack_tint(stack: &petramond_world::item::ItemStack) -> [f32; 4] {
    match petramond_world::item::variant::tint(stack.variant) {
        Some([r, g, b]) => [r, g, b, 1.0],
        None => [1.0; 4],
    }
}

fn slot_ndc_center(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    pixel_to_ndc(screen, r.x + r.w * 0.5, r.y + r.h * 0.5)
}

/// Per-axis clip-space half-extents `[p/w, p/h]` for the slot. The framebuffer isn't square, so a
/// pixel size `p` maps to a different NDC extent on each axis; a uniform scale would draw icons
/// wider than tall on 16:9. With these, `hx*w == hy*h == p` and the icon stays square. Every slot
/// gets the same pair since they share an interior pixel size.
fn ndc_half_extents(screen: (u32, u32), r: SlotRect) -> [f32; 2] {
    let (w, h) = (screen.0 as f32, screen.1 as f32);
    [r.w / w, r.h / h]
}

pub fn icon_view_dir() -> Vec3 {
    let (sx, cx) = 30f32.to_radians().sin_cos();
    let (sy, cy) = 45f32.to_radians().sin_cos();
    Vec3::new(-cx * sy, sx, cx * cy)
}

/// Iso ortho MVP mapping a unit cube into the slot's NDC rect. Back-face culling stands in for a
/// depth buffer. The icon-atlas bake calls this with a 64×64 cell `r`.
pub fn iso_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    // Classic MC item iso pose: +30° about X then 45° about Y. With CCW-front/back-face
    // culling in the model3d pipeline, this puts the cube's top face plus two sides (NegX,
    // PosZ) toward the +Z viewer and culls the bottom away, so the camera looks down at the
    // cube. A -30° X tilt instead shows the bottom plus sides.
    let rot = Mat4::from_rotation_x(30f32.to_radians()) * Mat4::from_rotation_y(45f32.to_radians());
    // A unit cube rotated this way spans ~sqrt(2) ≈ 1.414 across; scale so it fills
    // ~0.9 of the slot. The clip-space scale is ANISOTROPIC (`sx != sy` on a
    // non-square framebuffer): each axis uses its own NDC half-extent so the cube
    // renders as an on-screen SQUARE of slot pixels at any aspect ratio. A single
    // uniform factor here would stretch the cube wider than tall on a 16:9 screen.
    let model_half = std::f32::consts::SQRT_2 * 0.5;
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
fn gui_rotation(kind: petramond_world::block_model::BlockModelKind) -> glam::Quat {
    let r = petramond_world::block_model::display(kind).gui.rotation;
    glam::Quat::from_rotation_y(std::f32::consts::PI)
        * petramond_world::bbmodel::display_euler_quat(Vec3::new(r[0], -r[1], -r[2]))
}

/// Icon MVP for a bbmodel block. Takes authored Blockbench `display.gui` pose,
/// maps through `gui_rotation` into icon camera, auto-frames to ~0.9 of the slot
/// (square at any aspect, same trick as [`iso_icon_mvp`]). `build_block_model_icon`
/// bakes it into geometry; the model-icon pass depth-tests for panel/drawer order.
/// The icon-atlas bake calls it once per 64x64 cell `r`.
pub fn model_icon_mvp(
    screen: (u32, u32),
    r: SlotRect,
    kind: petramond_world::block_model::BlockModelKind,
) -> Mat4 {
    icon_mvp_for_rot(screen, r, kind, gui_rotation(kind))
}

fn icon_mvp_for_rot(
    screen: (u32, u32),
    r: SlotRect,
    kind: petramond_world::block_model::BlockModelKind,
    rot_quat: glam::Quat,
) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    let [hx, hy] = ndc_half_extents(screen, r);
    let rot = Mat4::from_quat(rot_quat);
    let fp = petramond_world::block_model::footprint(kind);
    let fpv = Vec3::new(fp[0] as f32, fp[1] as f32, fp[2] as f32);
    let span = fpv.max_element().max(1.0);
    let (bmn, bmx) = petramond_world::block_model::outline_bounds(kind);
    // Frame about the GEOMETRY centre, not the footprint centre: a model whose
    // silhouette overhangs its footprint (`fit: native` — the chair backrest,
    // the miller tray) otherwise pivots around a point off its visual centre
    // and renders small + shifted in the slot.
    let bc = ((Vec3::from(bmn) + Vec3::from(bmx)) * 0.5 - fpv * 0.5) / span;
    let mut half = 1e-3f32;
    let mut half_z = 1e-3f32;
    for &cx in &[bmn[0], bmx[0]] {
        for &cy in &[bmn[1], bmx[1]] {
            for &cz in &[bmn[2], bmx[2]] {
                let centred = (Vec3::new(cx, cy, cz) - fpv * 0.5) / span - bc;
                let p = rot.transform_point3(centred);
                half = half.max(p.x.abs()).max(p.y.abs());
                half_z = half_z.max(p.z.abs());
            }
        }
    }
    let fill = 1.0;
    let sx = hx * fill / half;
    let sy = hy * fill / half;
    let sz = 0.4 / half_z;
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(-sx, sy, sz))
        * rot
        * Mat4::from_translation(-bc)
}

pub fn flat_icon_mvp(screen: (u32, u32), r: SlotRect) -> Mat4 {
    let center = slot_ndc_center(screen, r);
    let [hx, hy] = ndc_half_extents(screen, r);
    let sx = hx * 2.0;
    let sy = hy * 2.0;
    Mat4::from_translation(Vec3::new(center[0], center[1], 0.5))
        * Mat4::from_scale(Vec3::new(sx, sy, 0.05))
}

pub(super) fn push_count(
    out: &mut Vec<UiVertex>,
    screen: (u32, u32),
    count: u32,
    r: SlotRect,
    scale: f32,
) {
    let fp = scale.max(1.0);
    let num_w = ui_text::number_width(count) as f32 * fp;
    let num_h = ui_text::GLYPH_H as f32 * fp;
    let x0 = r.x + r.w - num_w - fp * 0.0;
    let y0 = r.y + r.h - num_h - fp * 0.0;
    let shadow = [0.0, 0.0, 0.0, 1.0];
    let white = [1.0, 1.0, 1.0, 1.0];
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
    use petramond::gui::gui_scale;

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

    #[test]
    fn model_icon_bakes_the_real_model_into_the_slot() {
        use petramond_world::block_model::BlockModelKind;
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

    /// Not an assertion, just a visual check. Renders the bbmodel icon through the real
    /// `model_icon_mvp` + `build_block_model_icon` path, with the same depth buffer and atlas
    /// sampling as the `model_icon` pipeline, and dumps it to a PNG so you can check
    /// orientation and draw order without booting the game.
    /// Run: `cargo test --lib -- --ignored --nocapture render_model_icon_preview`.
    /// Writes /tmp/model_icon.png.
    #[test]
    #[ignore = "visual preview harness; run explicitly to regenerate /tmp/model_icon.png"]
    fn render_model_icon_preview() {
        use glam::Quat;
        use petramond_world::block_model::{self, BlockModelKind};

        let (atlas_rgba, aw, ah) = block_model::atlas().texture();

        let candidates: Vec<(String, BlockModelKind, Quat)> = [
            BlockModelKind::FurnitureWorkbench,
            BlockModelKind::Bed,
            BlockModelKind::ChiselingStation,
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

    /// The icon tilt determines which cube face points toward the camera, so
    /// check the top and bottom normals have opposite clip-z signs and that flipping the tilt
    /// swaps which one faces the camera.
    #[test]
    fn iso_mvp_flips_to_show_top_face() {
        let screen = (1280u32, 720u32);
        let r = slot_rect(0, screen, false, gui_scale(screen)).unwrap();
        let shipped = iso_icon_mvp(screen, r);
        let z_of = |m: Mat4, n: Vec3| (m * n.extend(0.0)).z;
        assert!(
            z_of(shipped, Vec3::Y) * z_of(shipped, Vec3::NEG_Y) < 0.0,
            "top and bottom normals must point opposite ways in the iso view"
        );
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

    #[test]
    fn iso_mvp_differs_only_by_per_slot_translation() {
        let screen = (1280, 720);
        let scale = gui_scale(screen);
        let check = |a: usize, b: usize, open: bool| {
            let ra = slot_rect(a, screen, open, scale).unwrap();
            let rb = slot_rect(b, screen, open, scale).unwrap();
            let ma = iso_icon_mvp(screen, ra);
            let mb = iso_icon_mvp(screen, rb);
            for col in 0..3 {
                let ca = ma.col(col);
                let cb = mb.col(col);
                assert!(
                    (ca - cb).length() < 1e-6,
                    "linear column {col} differs between slots {a},{b} (open={open}): {ca:?} vs {cb:?}"
                );
            }
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
        check(0, 1, false);
        check(0, 8, false);
        // Open grid: a hotbar slot, an adjacent grid slot, and a far grid slot.
        check(0, 1, true);
        check(9, 10, true);
        check(0, 35, true);
    }

    /// Scale must be anisotropic so icons keep the same shape across
    /// aspect ratios. NDC spans 2 units on both axes, so mapping a P-pixel slot to equal
    /// on-screen extents needs half-extents `[P/w, P/h]`; a uniform NDC factor
    /// stretches wide on 16:9.
    ///
    /// Flat sprite is a planar quad, so square-on-screen is exact: extents match slot pixel
    /// size on both axes at any aspect. Iso cube's silhouette is naturally taller than wide
    /// (the +30° tilt foreshortens the horizontal diagonal), so squareness here means
    /// aspect-independence: the on-screen footprint ratio stays identical across aspects (the
    /// old uniform version stretched it by roughly the framebuffer aspect ratio, about 2x
    /// between square and 2:1).
    #[test]
    fn icon_mvp_is_square_on_screen_at_any_aspect() {
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
        let r = SlotRect {
            x: 0.0,
            y: 0.0,
            w: 48.0,
            h: 48.0,
        };
        let (rx, ry) = pixel_extents(iso_icon_mvp((600, 600), r), (600, 600), true);
        let iso_ref_aspect = rx / ry;
        for &screen in &[(600u32, 600u32), (1280, 720), (1920, 1080)] {
            let (px, py) = pixel_extents(iso_icon_mvp(screen, r), screen, true);
            let aspect = px / py;
            assert!(
                (aspect - iso_ref_aspect).abs() < 1e-3,
                "iso icon footprint aspect changed with screen {screen:?}: {aspect} vs ref {iso_ref_aspect}"
            );
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
