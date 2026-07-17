use super::*;
use crate::item::ItemType;

#[test]
fn bare_hand_builds_solid_cuboid() {
    let view = HeldItemView {
        item: None,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
    assert!(!i.is_empty());
    // Solid cuboid = one cube (24 verts / 36 indices).
    assert_eq!(v.len(), 24);
    assert_eq!(i.len(), 36);
    // Every vertex carries the solid-color flag and the skin tint.
    for vert in &v {
        assert_eq!(
            vert.packed & super::super::SOLID_COLOR_FLAG,
            super::super::SOLID_COLOR_FLAG
        );
        assert_eq!(vert.tint, crate::mesh::pack_tint(SKIN));
    }
}

#[test]
fn held_block_builds_textured_cube() {
    let view = HeldItemView {
        item: Some(ItemType::OakLog),
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
    assert_eq!(v.len(), 24);
    assert_eq!(i.len(), 36);
    // Textured path never sets the solid flag.
    for vert in &v {
        assert_eq!(vert.packed & super::super::SOLID_COLOR_FLAG, 0);
    }
}

#[test]
fn lit_hand_packs_sampled_skylight() {
    let view = HeldItemView {
        item: Some(ItemType::Stone),
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());

    build_hand_lit(
        &view,
        16.0 / 9.0,
        DynLight { sky: 9, block: 5 },
        0,
        &mut v,
        &mut i,
    );

    assert!(!v.is_empty());
    for vert in &v {
        assert_eq!((vert.packed >> 23) & 0x3F, 9, "sky channel in word 1");
        assert_eq!(vert.packed2 & 0x3F, 5, "block channel in word 2");
    }
}

#[test]
fn held_sprite_emits_no_model3d_geometry() {
    // Sprite items are drawn by the renderer via the item3d (extruded)
    // pipeline, NOT the model3d hand pass, so build_hand emits nothing.
    let view = HeldItemView {
        item: Some(ItemType::Poppy),
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
    assert!(v.is_empty(), "sprite hand emits no model3d verts");
    assert!(i.is_empty(), "sprite hand emits no model3d indices");
}

/// CPU-rasterize one held-model view (perspective divide, z-buffer, model-atlas
/// sampling — exactly what the item3d hand pass draws) into row `row` of a stacked
/// `w`×`h`-per-row RGB canvas. Shared by the preview harnesses below.
fn raster_held_cell(
    kind: crate::block_model::BlockModelKind,
    mvp: Mat4,
    (w, h): (usize, usize),
    row: usize,
    color: &mut [u8],
) {
    use crate::render::lighting::{DynLight, LightEnv};
    let (atlas_rgba, aw, ah) = crate::block_model::atlas().texture();
    let (mut verts, mut indices) = (Vec::new(), Vec::new());
    crate::render::item_model::build_block_model_item(
        kind,
        Mat4::IDENTITY,
        DynLight::FULL,
        LightEnv::IDENTITY,
        0,
        None,
        &mut verts,
        &mut indices,
    );
    let mut zbuf = vec![f32::INFINITY; w * h];
    let project = |p: [f32; 3]| -> Option<[f32; 3]> {
        let c = mvp * glam::Vec4::new(p[0], p[1], p[2], 1.0);
        if c.w <= 1e-6 {
            return None;
        }
        let n = c / c.w;
        Some([
            (n.x * 0.5 + 0.5) * w as f32,
            (1.0 - (n.y * 0.5 + 0.5)) * h as f32,
            n.z,
        ])
    };
    for tri in indices.chunks_exact(3) {
        let vtx = [
            verts[tri[0] as usize],
            verts[tri[1] as usize],
            verts[tri[2] as usize],
        ];
        let (Some(s0), Some(s1), Some(s2)) = (
            project(vtx[0].pos),
            project(vtx[1].pos),
            project(vtx[2].pos),
        ) else {
            continue;
        };
        let s = [s0, s1, s2];
        let (x0, y0, x1, y1, x2, y2) = (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
        let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
        if area.abs() < 1e-6 {
            continue;
        }
        let inv_area = 1.0 / area;
        let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
        let maxx = x0.max(x1).max(x2).ceil().min(w as f32 - 1.0) as usize;
        let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
        let maxy = y0.max(y1).max(y2).ceil().min(h as f32 - 1.0) as usize;
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
                let li = y * w + x;
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
                let o = ((row * h + y) * w + x) * 3;
                color[o] = (atlas_rgba[ti] as f32 * shade).min(255.0) as u8;
                color[o + 1] = (atlas_rgba[ti + 1] as f32 * shade).min(255.0) as u8;
                color[o + 2] = (atlas_rgba[ti + 2] as f32 * shade).min(255.0) as u8;
            }
        }
    }
}

/// Visual preview harness (NOT an assertion): rasterizes each held bbmodel item via
/// the REAL `held_model` MVP into a stacked PNG, so the in-hand pose can be checked
/// against Blockbench's first-person preview without launching the game.
/// Run: `cargo test --lib -- --ignored --nocapture render_held_model_preview`.
/// Writes /tmp/held_model.png.
#[test]
#[ignore = "visual preview harness; run explicitly to regenerate /tmp/held_model.png"]
fn render_held_model_preview() {
    let items = [
        ("WoodenBucket", ItemType::WoodenBucket),
        ("WaterBucket", ItemType::WaterBucket),
        ("FurnitureWorkbench", ItemType::FurnitureWorkbench),
        ("Bed", ItemType::Bed),
    ];
    let (w, h) = (940usize, 530usize);
    let aspect = w as f32 / h as f32;
    let bg = [30u8, 32, 38];
    let gh = h * items.len();
    let mut color = vec![0u8; w * gh * 3];
    for px in color.chunks_mut(3) {
        px.copy_from_slice(&bg);
    }
    for (row, (label, item)) in items.iter().enumerate() {
        let view = HeldItemView {
            item: Some(*item),
            ..Default::default()
        };
        let (kind, mvp) = held_model(&view, aspect).expect("model item");
        raster_held_cell(kind, mvp, (w, h), row, &mut color);
        println!("row {row}: {label}");
    }
    image::save_buffer(
        "/tmp/held_model.png",
        &color,
        w as u32,
        gh as u32,
        image::ColorType::Rgb8,
    )
    .expect("save png");
    println!("wrote /tmp/held_model.png ({w}x{gh}, one row per item)");
}

#[test]
fn held_sprite_reports_tile_and_mvp() {
    // held_sprite drives the extruded item3d draw; it must report the sprite
    // tile (and a finite MVP) for a sprite item and None otherwise.
    let poppy = HeldItemView {
        item: Some(ItemType::Poppy),
        ..Default::default()
    };
    let (tile, mvp) = held_sprite(&poppy, 16.0 / 9.0).expect("sprite reports a tile");
    assert_eq!(tile, crate::atlas::Tile::named("poppy"));
    assert!(mvp.to_cols_array().iter().all(|f| f.is_finite()));
    // Bare hand + held block return None (they go through build_hand).
    let bare = HeldItemView {
        item: None,
        block_state: Default::default(),
        ..poppy
    };
    let block = HeldItemView {
        item: Some(ItemType::Stone),
        block_state: Default::default(),
        ..poppy
    };
    assert!(held_sprite(&bare, 1.5).is_none());
    assert!(held_sprite(&block, 1.5).is_none());
}

#[test]
fn build_hand_reuses_buffers_without_growth() {
    // The hand buffers are cleared + refilled each call, never reallocated.
    let block = HeldItemView {
        item: Some(ItemType::Stone),
        ..Default::default()
    };
    let bare = HeldItemView {
        item: None,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    build_hand(&block, 1.5, &mut v, &mut i);
    let (vcap, icap) = (v.capacity(), i.capacity());
    // Same vert/index count for the bare hand, so capacity is unchanged.
    build_hand(&bare, 1.5, &mut v, &mut i);
    assert_eq!(v.capacity(), vcap, "hand vert buffer reused");
    assert_eq!(i.capacity(), icap, "hand index buffer reused");
}

#[derive(Copy, Clone)]
struct Bounds {
    min_x: f32,
    max_x: f32,
    min_y: f32,
    max_y: f32,
}

fn ndc_bounds(mvp: Mat4) -> Bounds {
    use glam::Vec4;

    let mut bounds = Bounds {
        min_x: f32::INFINITY,
        max_x: f32::NEG_INFINITY,
        min_y: f32::INFINITY,
        max_y: f32::NEG_INFINITY,
    };
    for &x in &[-0.5f32, 0.5] {
        for &y in &[-0.5f32, 0.5] {
            for &z in &[-0.5f32, 0.5] {
                let c = mvp * Vec4::new(x, y, z, 1.0);
                let ndc = c / c.w;
                bounds.min_x = bounds.min_x.min(ndc.x);
                bounds.max_x = bounds.max_x.max(ndc.x);
                bounds.min_y = bounds.min_y.min(ndc.y);
                bounds.max_y = bounds.max_y.max(ndc.y);
            }
        }
    }
    bounds
}

fn projected_face_area(mvp: Mat4, face: [Vec3; 4]) -> f32 {
    let mut p = [[0.0f32; 2]; 4];
    for (dst, src) in p.iter_mut().zip(face) {
        let c = mvp * src.extend(1.0);
        let ndc = c / c.w;
        *dst = [ndc.x, ndc.y];
    }
    let mut area = 0.0;
    for i in 0..4 {
        let a = p[i];
        let b = p[(i + 1) & 3];
        area += a[0] * b[1] - b[0] * a[1];
    }
    area.abs() * 0.5
}

#[test]
fn bare_hand_rest_is_anchored_lower_right() {
    let screens: [(u32, u32); 4] = [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160)];
    let view = HeldItemView {
        item: None,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    for screen in screens {
        let aspect = screen.0 as f32 / screen.1 as f32;
        let bounds = ndc_bounds(build_hand(&view, aspect, &mut v, &mut i));
        assert!(
            bounds.min_x > 0.42,
            "hand starts too far left on {screen:?}: {}",
            bounds.min_x
        );
        assert!(
            bounds.max_x > 0.86,
            "hand is almost hidden off the right side on {screen:?}: {}",
            bounds.max_x
        );
        assert!(
            bounds.min_y < -0.95,
            "hand bottom should sit offscreen on {screen:?}: {}",
            bounds.min_y
        );
        assert!(
            bounds.max_y < -0.20,
            "hand is too high on {screen:?}: {}",
            bounds.max_y
        );
        assert!(
            bounds.max_y > -0.70,
            "hand is almost hidden below the screen on {screen:?}: {}",
            bounds.max_y
        );
    }
}

#[test]
fn bare_hand_rest_does_not_show_large_fist_cap() {
    let view = HeldItemView {
        item: None,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    let mvp = build_hand(&view, 16.0 / 9.0, &mut v, &mut i);

    let pos_x = [
        Vec3::new(0.5, -0.5, 0.5),
        Vec3::new(0.5, -0.5, -0.5),
        Vec3::new(0.5, 0.5, -0.5),
        Vec3::new(0.5, 0.5, 0.5),
    ];
    let neg_x = [
        Vec3::new(-0.5, -0.5, -0.5),
        Vec3::new(-0.5, -0.5, 0.5),
        Vec3::new(-0.5, 0.5, 0.5),
        Vec3::new(-0.5, 0.5, -0.5),
    ];
    let pos_y = [
        Vec3::new(-0.5, 0.5, 0.5),
        Vec3::new(0.5, 0.5, 0.5),
        Vec3::new(0.5, 0.5, -0.5),
        Vec3::new(-0.5, 0.5, -0.5),
    ];
    let neg_y = [
        Vec3::new(-0.5, -0.5, -0.5),
        Vec3::new(0.5, -0.5, -0.5),
        Vec3::new(0.5, -0.5, 0.5),
        Vec3::new(-0.5, -0.5, 0.5),
    ];
    let pos_z = [
        Vec3::new(-0.5, -0.5, 0.5),
        Vec3::new(0.5, -0.5, 0.5),
        Vec3::new(0.5, 0.5, 0.5),
        Vec3::new(-0.5, 0.5, 0.5),
    ];
    let neg_z = [
        Vec3::new(0.5, -0.5, -0.5),
        Vec3::new(-0.5, -0.5, -0.5),
        Vec3::new(-0.5, 0.5, -0.5),
        Vec3::new(0.5, 0.5, -0.5),
    ];

    let top = projected_face_area(mvp, pos_z).max(projected_face_area(mvp, neg_z));
    let side = projected_face_area(mvp, pos_x).max(projected_face_area(mvp, neg_x));
    let end_cap = projected_face_area(mvp, pos_y).max(projected_face_area(mvp, neg_y));
    assert!(
        side > end_cap * 1.5,
        "vanilla arm should not expose a dominant fist/end cap: side={side}, cap={end_cap}"
    );
    assert!(
        top > end_cap * 1.8,
        "vanilla arm top/back face should dominate fist/end cap: top={top}, cap={end_cap}"
    );
}

#[test]
fn swing_punches_forward_instead_of_right_hooking() {
    let aspect = 16.0 / 9.0;
    let rest_view = HeldItemView {
        item: None,
        ..Default::default()
    };
    let early_view = HeldItemView {
        swing: 0.25,
        ..rest_view
    };
    let mid_view = HeldItemView {
        swing: 0.5,
        ..rest_view
    };
    let late_view = HeldItemView {
        swing: 0.75,
        ..rest_view
    };
    let done_view = HeldItemView {
        swing: 1.0,
        ..rest_view
    };

    let rest = bare_arm_placement(&rest_view, aspect).transform_point3(Vec3::ZERO);
    let early = bare_arm_placement(&early_view, aspect).transform_point3(Vec3::ZERO);
    let mid = bare_arm_placement(&mid_view, aspect).transform_point3(Vec3::ZERO);
    let late = bare_arm_placement(&late_view, aspect).transform_point3(Vec3::ZERO);
    let done = bare_arm_placement(&done_view, aspect).transform_point3(Vec3::ZERO);

    assert!(
        early.x < rest.x,
        "vanilla swing should move toward center, not right: {early:?} vs {rest:?}"
    );
    assert!(
        mid.x < rest.x,
        "mid-swing should not hook right: {mid:?} vs {rest:?}"
    );
    assert!(
        mid.z < rest.z,
        "mid-swing should punch forward into the target block: {mid:?} vs {rest:?}"
    );
    assert!(
        late.x < rest.x,
        "late swing should still be returning from a forward/center punch, not finishing right"
    );
    assert!(
        (done - rest).length() < 0.001,
        "swing phase 1.0 should return to rest"
    );

    let rest_up = bare_arm_placement(&rest_view, aspect)
        .transform_vector3(Vec3::Y)
        .normalize();
    let mid_up = bare_arm_placement(&mid_view, aspect)
        .transform_vector3(Vec3::Y)
        .normalize();
    assert!(
        rest_up.dot(mid_up) < 0.86,
        "swing should rotate around the pivot, not just translate"
    );
}

#[test]
fn arm_punch_hinges_the_fist_forward_from_the_shoulder() {
    let aspect = 16.0 / 9.0;
    let fist_local = Vec3::new(0.0, 6.0, 0.0); // +Y end of the arm cuboid
    let view = |swing| HeldItemView {
        item: None,
        block_state: Default::default(),
        swing,
        swing_scale: 1.0,
        ..Default::default()
    };
    let fist = |swing| bare_arm_placement(&view(swing), aspect).transform_point3(fist_local);
    let shoulder =
        |swing| bare_arm_placement(&view(swing), aspect).transform_point3(ARM_SHOULDER_LOCAL);

    let rest = fist(0.0);
    let mid = fist(0.5);
    // The fist drives toward screen centre (smaller x) and into the screen
    // (more negative z): a forward punch, not the old sideways wipe.
    assert!(
        mid.x < rest.x,
        "fist should swing toward center: {mid:?} vs {rest:?}"
    );
    assert!(
        mid.z < rest.z,
        "fist should punch into the screen: {mid:?} vs {rest:?}"
    );

    // The shoulder pivot barely moves — the arm hinges, it doesn't slide.
    assert!(
        (shoulder(0.5) - shoulder(0.0)).length() < 1e-4,
        "shoulder is the fixed pivot of the punch"
    );

    // The strike returns home: phase 1.0 matches the rest pose.
    assert!(
        (fist(1.0) - rest).length() < 1e-3,
        "punch should ease back to rest at phase 1.0"
    );
}

#[test]
fn swing_and_place_change_the_mvp() {
    let rest = HeldItemView {
        item: Some(ItemType::Stone),
        ..Default::default()
    };
    let mid_punch = HeldItemView { swing: 0.5, ..rest };
    // A reduced amplitude (< 1.0) stands in for the softer place jab so the
    // resulting MVP differs from the full mining punch.
    let mid_place = HeldItemView {
        swing: 0.5,
        swing_scale: 0.62,
        ..rest
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    let a = build_hand(&rest, 1.5, &mut v, &mut i);
    let b = build_hand(&mid_punch, 1.5, &mut v, &mut i);
    let c = build_hand(&mid_place, 1.5, &mut v, &mut i);
    assert_ne!(a, b, "mid-swing must move the hand");
    // The softer place jab also moves the hand, but less than a full punch.
    assert_ne!(a, c, "place swing must move the hand");
    assert_ne!(b, c, "the place jab is softer than the mining punch");
}

/// Visual preview harness (NOT an assertion): rasterizes held sprite items via
/// the REAL `held_sprite` MVP (so it reflects each item's per-item `held_pose`)
/// to PNGs — pose looks right in source but wrong on screen, so render to
/// verify. Run: `cargo test --lib -- --ignored --nocapture render_held_item_preview`.
/// Writes /tmp/held_<item>.png (full 16:9) + _zoom.png (auto-framed 2x).
#[test]
#[ignore = "visual preview harness; run explicitly to regenerate /tmp PNGs"]
fn render_held_item_preview() {
    use crate::atlas::tile_uv;
    use crate::item::ItemType;
    use glam::Vec4;

    // (item, texture, eat blend, bite phase, approach) — the eat rows
    // preview the mouth-carry pose (mid-carry, full, and full at the end
    // of the toward-the-camera approach).
    let targets = [
        (ItemType::StonePickaxe, "stone_pickaxe.png", 0.0, 0.0, 0.0),
        (ItemType::Poppy, "poppy.png", 0.0, 0.0, 0.0),
        (ItemType::Poppy, "poppy.png", 0.5, 0.6, 0.0),
        (ItemType::Poppy, "poppy.png", 1.0, 0.9, 0.0),
        (ItemType::Poppy, "poppy.png", 1.0, -0.9, 1.0),
    ];
    const W: usize = 1280;
    const H: usize = 720;
    let aspect = W as f32 / H as f32;
    let bg = [74u8, 100, 64];

    for (item, file, eat, eat_bob, eat_near) in targets {
        let view = HeldItemView {
            item: Some(item),
            block_state: Default::default(),
            swing: 0.0,
            swing_scale: 1.0,
            eat,
            eat_bob,
            eat_near,
        };
        let (tile, mvp) = held_sprite(&view, aspect).expect("sprite item");
        let mut verts = Vec::new();
        crate::render::item_model::build_extruded_item_lit(
            tile,
            DynLight::FULL,
            crate::render::lighting::LightEnv::IDENTITY,
            &mut verts,
        );
        let src = format!("{}/assets/textures/{}", env!("CARGO_MANIFEST_DIR"), file);
        let img = image::open(&src).expect("texture").to_rgba8();
        let (tw, th) = img.dimensions();
        let [au0, av0, au1, av1] = tile_uv(tile);

        let mut color = vec![0u8; W * H * 3];
        for px in color.chunks_mut(3) {
            px.copy_from_slice(&bg);
        }
        let mut zbuf = vec![f32::INFINITY; W * H];
        let (mut bx0, mut by0, mut bx1, mut by1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        let project = |p: [f32; 3]| -> [f32; 4] {
            let clip = mvp * Vec4::new(p[0], p[1], p[2], 1.0);
            let invw = 1.0 / clip.w;
            [
                (clip.x * invw * 0.5 + 0.5) * W as f32,
                (1.0 - (clip.y * invw * 0.5 + 0.5)) * H as f32,
                clip.z * invw,
                invw,
            ]
        };
        for tri in verts.chunks_exact(3) {
            let shade = tri[0].shade;
            let s = [
                project(tri[0].pos),
                project(tri[1].pos),
                project(tri[2].pos),
            ];
            let uvw = [
                [tri[0].uv[0] * s[0][3], tri[0].uv[1] * s[0][3]],
                [tri[1].uv[0] * s[1][3], tri[1].uv[1] * s[1][3]],
                [tri[2].uv[0] * s[2][3], tri[2].uv[1] * s[2][3]],
            ];
            for v in &s {
                bx0 = bx0.min(v[0]);
                by0 = by0.min(v[1]);
                bx1 = bx1.max(v[0]);
                by1 = by1.max(v[1]);
            }
            let (x0, y0, x1, y1, x2, y2) =
                (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
            let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
            if area.abs() < 1e-6 {
                continue;
            }
            let inv_area = 1.0 / area;
            let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
            let maxx = x0.max(x1).max(x2).ceil().min(W as f32 - 1.0) as usize;
            let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
            let maxy = y0.max(y1).max(y2).ceil().min(H as f32 - 1.0) as usize;
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
                    let idx = y * W + x;
                    if z >= zbuf[idx] {
                        continue;
                    }
                    let invw = w0 * s[0][3] + w1 * s[1][3] + w2 * s[2][3];
                    let u = (w0 * uvw[0][0] + w1 * uvw[1][0] + w2 * uvw[2][0]) / invw;
                    let v = (w0 * uvw[0][1] + w1 * uvw[1][1] + w2 * uvw[2][1]) / invw;
                    let lu = (u - au0) / (au1 - au0);
                    let lv = (v - av0) / (av1 - av0);
                    let sx = (lu * tw as f32).clamp(0.0, tw as f32 - 1.0) as u32;
                    let sy = (lv * th as f32).clamp(0.0, th as f32 - 1.0) as u32;
                    let texel = img.get_pixel(sx, sy).0;
                    if texel[3] < 128 {
                        continue;
                    }
                    zbuf[idx] = z;
                    let o = idx * 3;
                    color[o] = (texel[0] as f32 * shade) as u8;
                    color[o + 1] = (texel[1] as f32 * shade) as u8;
                    color[o + 2] = (texel[2] as f32 * shade) as u8;
                }
            }
        }
        let name = if eat > 0.0 {
            let bite = if eat_bob >= 0.0 { "in" } else { "out" };
            let near = if eat_near > 0.0 { "_near" } else { "" };
            format!("{item:?}_eat{:.0}_bite_{bite}{near}", eat * 100.0).to_lowercase()
        } else {
            format!("{item:?}").to_lowercase()
        };
        let full = format!("/tmp/held_{name}.png");
        image::save_buffer(&full, &color, W as u32, H as u32, image::ColorType::Rgb8)
            .expect("save full");
        let pad = 24.0;
        let cx0 = (bx0 - pad).max(0.0) as usize;
        let cy0 = (by0 - pad).max(0.0) as usize;
        let cx1 = ((bx1 + pad).min(W as f32 - 1.0)) as usize;
        let cy1 = ((by1 + pad).min(H as f32 - 1.0)) as usize;
        let (cw, ch) = (cx1 - cx0 + 1, cy1 - cy0 + 1);
        let mut crop = vec![0u8; cw * 2 * ch * 2 * 3];
        for y in 0..ch * 2 {
            for x in 0..cw * 2 {
                let srcp = ((cy0 + y / 2) * W + (cx0 + x / 2)) * 3;
                let dst = (y * cw * 2 + x) * 3;
                crop[dst..dst + 3].copy_from_slice(&color[srcp..srcp + 3]);
            }
        }
        let zoom = format!("/tmp/held_{name}_zoom.png");
        image::save_buffer(
            &zoom,
            &crop,
            (cw * 2) as u32,
            (ch * 2) as u32,
            image::ColorType::Rgb8,
        )
        .expect("save zoom");
        println!("wrote {full} + {zoom}  (roll={:.2})", item.held_pose().roll);
    }
}
