use super::*;
use petramond_world::item::ItemType;

#[test]
fn bare_hand_builds_solid_cuboid() {
    let view = HeldItemView {
        item: None,
        variant: petramond_world::item::VariantId::NONE,
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
        assert_eq!(vert.tint, petramond_mesh::pack_tint(SKIN));
    }
}

#[test]
fn held_block_builds_textured_cube() {
    let view = HeldItemView {
        item: Some(ItemType::OakLog),
        variant: petramond_world::item::VariantId::NONE,
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
        variant: petramond_world::item::VariantId::NONE,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());

    build_hand_lit(
        &view,
        16.0 / 9.0,
        DynLight {
            sky: 9,
            block: petramond_world::light::BlockLight6::new(5, 2, 63),
        },
        &mut v,
        &mut i,
    );

    assert!(!v.is_empty());
    for vert in &v {
        assert_eq!(
            (vert.packed >> petramond_mesh::vertex::SKY_SHIFT) & 0x3F,
            9,
            "sky channel in word 1"
        );
        // The block channel's COLOUR must survive the three-way vertex split
        // on the dynamic path too, not just in the chunk mesher.
        assert_eq!(
            petramond_mesh::vertex::decode_vertex_light(vert),
            petramond_world::light::BlockLight6::new(5, 2, 63),
            "block light colour in the split lanes"
        );
    }
}

#[test]
fn held_sprite_emits_no_model3d_geometry() {
    // Sprite items are drawn by the renderer via the item3d (extruded)
    // pipeline, NOT the model3d hand pass, so build_hand emits nothing.
    let view = HeldItemView {
        item: Some(ItemType::Poppy),
        variant: petramond_world::item::VariantId::NONE,
        ..Default::default()
    };
    let (mut v, mut i) = (Vec::new(), Vec::new());
    build_hand(&view, 16.0 / 9.0, &mut v, &mut i);
    assert!(v.is_empty(), "sprite hand emits no model3d verts");
    assert!(i.is_empty(), "sprite hand emits no model3d indices");
}

/// CPU-rasterize one held-model view (perspective divide, z-buffer, model-atlas
/// sampling — exactly what the item3d hand pass draws) into cell `(col, row)`
/// of a `cols`-wide grid of `w`×`h` RGB cells. Shared by the preview
/// harnesses below. Deliberately does NOT back-face cull — the item3d held
/// pipeline is double-sided (cull `None`), so a reflected (negative
/// determinant) off-hand MVP draws in game exactly as it does here.
fn raster_held_cell(
    kind: petramond_world::block_model::BlockModelKind,
    mvp: Mat4,
    (w, h): (usize, usize),
    (col, row): (usize, usize),
    cols: usize,
    color: &mut [u8],
) {
    use crate::lighting::{DynLight, LightEnv};
    let (atlas_rgba, aw, ah) = petramond_world::block_model::atlas().texture();
    let (mut verts, mut indices) = (Vec::new(), Vec::new());
    crate::item_model::build_block_model_item(
        kind,
        Mat4::IDENTITY,
        DynLight::FULL,
        LightEnv::IDENTITY,
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
                let o = ((row * h + y) * (w * cols) + col * w + x) * 3;
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
            variant: petramond_world::item::VariantId::NONE,
            ..Default::default()
        };
        let (kind, mvp) = held_model(&view, aspect).expect("model item");
        raster_held_cell(kind, mvp, (w, h), (0, row), 1, &mut color);
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

/// CPU-rasterize one held SPRITE view (the extruded slab, texture-file
/// sampled through its atlas rect) into cell `(col, row)` of a `cols`-wide
/// grid — the sprite twin of [`raster_held_cell`], shared by the off-hand
/// preview. No back-face cull, like the double-sided item3d pipeline.
fn raster_sprite_cell(
    tile: petramond_world::tile::Tile,
    texture: &str,
    mvp: Mat4,
    (w, h): (usize, usize),
    (col, row): (usize, usize),
    cols: usize,
    color: &mut [u8],
) {
    use crate::atlas::tile_uv;
    use glam::Vec4;

    let mut verts = Vec::new();
    crate::item_model::build_extruded_item_lit(
        tile,
        DynLight::FULL,
        crate::lighting::LightEnv::IDENTITY,
        &mut verts,
    );
    // Repo-root texture dir (this crate moved under crates/ in the split).
    let src = format!(
        "{}/../../assets/textures/{texture}",
        env!("CARGO_MANIFEST_DIR")
    );
    let img = image::open(&src).expect("texture").to_rgba8();
    let (tw, th) = img.dimensions();
    let [au0, av0, au1, av1] = tile_uv(tile);
    let mut zbuf = vec![f32::INFINITY; w * h];
    let project = |p: [f32; 3]| -> [f32; 4] {
        let clip = mvp * Vec4::new(p[0], p[1], p[2], 1.0);
        let invw = 1.0 / clip.w;
        [
            (clip.x * invw * 0.5 + 0.5) * w as f32,
            (1.0 - (clip.y * invw * 0.5 + 0.5)) * h as f32,
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
                // Inside-triangle test that works for BOTH windings (a
                // reflected MVP flips the sign of every barycentric).
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
                if !inside {
                    continue;
                }
                let z = w0 * s[0][2] + w1 * s[1][2] + w2 * s[2][2];
                let idx = y * w + x;
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
                let o = ((row * h + y) * (w * cols) + col * w + x) * 3;
                color[o] = (texel[0] as f32 * shade) as u8;
                color[o + 1] = (texel[1] as f32 * shade) as u8;
                color[o + 2] = (texel[2] as f32 * shade) as u8;
            }
        }
    }
}

/// Visual preview harness (NOT an assertion): every off-hand render path
/// beside its right-hand twin — LEFT column = right hand, RIGHT column = the
/// off hand — so each off view can be checked against Blockbench's lefthand
/// preview of the same model without launching the game. (Sprites mirror the
/// right-hand image exactly; bbmodels show Blockbench's conjugated-pose
/// lefthand view — see `held_model_off`.)
/// Run: `cargo test --lib -- --ignored --nocapture render_off_hand_preview`.
/// Writes /tmp/off_hand_preview.png.
#[test]
#[ignore = "visual preview harness; run explicitly to regenerate /tmp/off_hand_preview.png"]
fn render_off_hand_preview() {
    // Engine models plus the pack machines whose authored display data has
    // real x-offsets — the shapes that catch a wrong mirror rule (the
    // workspace `mods/` root loads packs in test binaries automatically).
    let model_items = [
        ("WoodenBucket", ItemType::WoodenBucket),
        ("FurnitureWorkbench", ItemType::FurnitureWorkbench),
        ("Bed", ItemType::Bed),
        (
            "forge:pottery_table",
            ItemType::by_key("forge:pottery_table").expect("forge pack loads"),
        ),
        (
            "forge:forging_furnace",
            ItemType::by_key("forge:forging_furnace").expect("forge pack loads"),
        ),
        (
            "farming:farmers_workbench",
            ItemType::by_key("farming:farmers_workbench").expect("farming pack loads"),
        ),
    ];
    let sprite_items = [
        ("StonePickaxe", ItemType::StonePickaxe, "stone_pickaxe.png"),
        ("Poppy", ItemType::Poppy, "poppy.png"),
    ];
    let (w, h) = (640usize, 400usize);
    let aspect = w as f32 / h as f32;
    let rows = model_items.len() + sprite_items.len();
    let (gw, gh) = (w * 2, h * rows);
    let bg = [30u8, 32, 38];
    let mut color = vec![0u8; gw * gh * 3];
    for px in color.chunks_mut(3) {
        px.copy_from_slice(&bg);
    }
    for (row, (label, item)) in model_items.iter().enumerate() {
        let view = HeldItemView {
            item: Some(*item),
            variant: petramond_world::item::VariantId::NONE,
            ..Default::default()
        };
        let (kind, right) = held_model(&view, aspect).expect("model item");
        let (_, left) = held_model_off(&view, aspect).expect("model item");
        raster_held_cell(kind, right, (w, h), (0, row), 2, &mut color);
        raster_held_cell(kind, left, (w, h), (1, row), 2, &mut color);
        println!("row {row}: {label} (right | off)");
    }
    for (i, (label, item, texture)) in sprite_items.iter().enumerate() {
        let row = model_items.len() + i;
        let view = HeldItemView {
            item: Some(*item),
            variant: petramond_world::item::VariantId::NONE,
            ..Default::default()
        };
        let (tile, right) = held_sprite(&view, aspect).expect("sprite item");
        let (_, left) = held_sprite_off(&view, aspect).expect("sprite item");
        raster_sprite_cell(tile, texture, right, (w, h), (0, row), 2, &mut color);
        raster_sprite_cell(tile, texture, left, (w, h), (1, row), 2, &mut color);
        println!("row {row}: {label} (right | off)");
    }
    image::save_buffer(
        "/tmp/off_hand_preview.png",
        &color,
        gw as u32,
        gh as u32,
        image::ColorType::Rgb8,
    )
    .expect("save png");
    println!(
        "wrote /tmp/off_hand_preview.png ({gw}x{gh}; left col = right hand, right col = off hand)"
    );
}

#[test]
fn held_sprite_reports_tile_and_mvp() {
    // held_sprite drives the extruded item3d draw; it must report the sprite
    // tile (and a finite MVP) for a sprite item and None otherwise.
    let poppy = HeldItemView {
        item: Some(ItemType::Poppy),
        variant: petramond_world::item::VariantId::NONE,
        ..Default::default()
    };
    let (tile, mvp) = held_sprite(&poppy, 16.0 / 9.0).expect("sprite reports a tile");
    assert_eq!(tile, petramond_world::tile::Tile::named("poppy"));
    assert!(mvp.to_cols_array().iter().all(|f| f.is_finite()));
    // Bare hand + held block return None (they go through build_hand).
    let bare = HeldItemView {
        item: None,
        variant: petramond_world::item::VariantId::NONE,
        block_state: Default::default(),
        ..poppy
    };
    let block = HeldItemView {
        item: Some(ItemType::Stone),
        variant: petramond_world::item::VariantId::NONE,
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
        variant: petramond_world::item::VariantId::NONE,
        ..Default::default()
    };
    let bare = HeldItemView {
        item: None,
        variant: petramond_world::item::VariantId::NONE,
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
        variant: petramond_world::item::VariantId::NONE,
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
        variant: petramond_world::item::VariantId::NONE,
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
fn swing_and_place_change_the_mvp() {
    let rest = HeldItemView {
        item: Some(ItemType::Stone),
        variant: petramond_world::item::VariantId::NONE,
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
    use glam::Vec4;
    use petramond_world::item::ItemType;

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
            variant: petramond_world::item::VariantId::NONE,
            block_state: Default::default(),
            bob: [0.0, 0.0],
            swing: 0.0,
            swing_scale: 1.0,
            eat,
            eat_bob,
            eat_near,
            pose: Default::default(),
        };
        let (tile, mvp) = held_sprite(&view, aspect).expect("sprite item");
        let mut verts = Vec::new();
        crate::item_model::build_extruded_item_lit(
            tile,
            DynLight::FULL,
            crate::lighting::LightEnv::IDENTITY,
            &mut verts,
        );
        // Repo-root texture dir (this crate moved under crates/ in the split).
        let src = format!(
            "{}/../../assets/textures/{}",
            env!("CARGO_MANIFEST_DIR"),
            file
        );
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
            let (x0, y0, x1, y1, x2, y2) = (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
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

/// Visual preview harness (NOT an assertion): photograph a mod-set held pose
/// (`SetPlayerHeldPose`) in BOTH views, BOTH hands, and both of a two-state
/// rule's states — the instrument for any "position the held item exactly
/// like this" work.
///
/// A pose looks right in source and wrong on screen, and the two views start
/// from DIFFERENT authored holds, so a number that reads well in one is
/// routinely nonsense in the other. Shooting all four cells at once is what
/// makes that visible in one glance instead of three playtest rounds.
///
/// Reads the real `held_model` / `held_model_off` MVPs and the real
/// `held_model_transform*` body attach chain, so what it draws is what the
/// game draws — the poses below are the ones the mod publishes.
///
/// Run (the pack registry needs the built mods):
///   `bash scripts/with-test-mods.sh cargo test -p petramond-render --lib \
///        -- --ignored --nocapture render_held_pose_preview`
/// Writes /tmp/held_pose_<item>.png — a 2×4 grid, rows = states, columns =
/// [1P main, 1P off, 3P front, 3P side].
#[test]
#[ignore = "visual preview harness; run explicitly to regenerate /tmp/held_pose_*.png"]
fn render_held_pose_preview() {
    use crate::item_model::ItemVertex;
    use crate::lighting::LightEnv;
    use crate::HeldPose;
    use petramond::player::model::player_model;
    use petramond_world::block_model::DisplayTransform;
    use petramond_world::item::{ItemRenderKind, ItemType};
    use petramond_world::light::BlockLight6;

    // The item under the lens: any bbmodel item in the registry. `HELD_POSE_ITEM`
    // names a pack's (run under `scripts/with-test-mods.sh` so it resolves);
    // the default is an engine item, so the harness works in a bare checkout.
    //
    // NOTHING here mirrors a pack's tuned numbers. A pose is authored in the
    // pack that owns it and driven in through `HELD_POSE_STATES`, which is the
    // whole point: iterating is a re-run, not an edit — of this file least of
    // all.
    let item_name =
        std::env::var("HELD_POSE_ITEM").unwrap_or_else(|_| "petramond:wooden_bucket".into());
    // The default rows are single-axis probes, not somebody's finished pose:
    // render `identity` plus one nudge per channel and read what each does.
    let nudged = |rotation: [f32; 3], translation: [f32; 3]| DisplayTransform {
        rotation,
        translation,
        ..Default::default()
    };
    let mut states: Vec<(String, HeldPose, Vec<crate::BoneOffset>)> = vec![
        ("identity".into(), HeldPose::default(), Vec::new()),
        (
            "+4px up".into(),
            HeldPose {
                first_person: nudged([0.0; 3], [0.0, 4.0, 0.0]),
                third_person: nudged([0.0; 3], [0.0, 4.0, 0.0]),
            },
            Vec::new(),
        ),
        (
            "-30deg x".into(),
            HeldPose {
                first_person: nudged([-30.0, 0.0, 0.0], [0.0; 3]),
                third_person: nudged([-30.0, 0.0, 0.0], [0.0; 3]),
            },
            Vec::new(),
        ),
    ];
    // Iterating a pose is a RE-RUN, not an edit: `HELD_POSE_STATES` replaces
    // the rows above with
    // `label=rx,ry,rz,tx,ty,tz|rx,ry,rz,tx,ty,tz[|bone:rx,ry,rz,tx,ty,tz]`
    // (first person | third person | one optional BONE offset),
    // semicolon-separated between rows.
    if let Ok(spec) = std::env::var("HELD_POSE_STATES") {
        states.clear();
        for row in spec.split(';').filter(|r| !r.trim().is_empty()) {
            let (label, rest) = row.split_once('=').expect("label=…");
            let mut parts = rest.split('|');
            let parse = |t: &str| {
                let n: Vec<f32> = t
                    .split(',')
                    .map(|v| v.trim().parse().expect("number"))
                    .collect();
                DisplayTransform {
                    rotation: [n[0], n[1], n[2]],
                    translation: [n[3], n[4], n[5]],
                    ..Default::default()
                }
            };
            let first_person = parse(parts.next().expect("first-person pose"));
            let third_person = parse(parts.next().expect("third-person pose"));
            let mut bones: Vec<crate::BoneOffset> = Vec::new();
            for spec in parts.filter(|s| !s.trim().is_empty()) {
                let (bone, nums) = spec.split_once(':').expect("bone:rx,…");
                let t = parse(nums);
                let Some(bone) = player_model().bone_named(bone.trim()) else {
                    panic!("the player rig has no bone '{bone}'");
                };
                bones.push(crate::BoneOffset {
                    bone,
                    rotation: t.rotation,
                    translation: t.translation,
                    hold: true,
                });
            }
            states.push((
                label.to_string(),
                HeldPose {
                    first_person,
                    third_person,
                },
                bones,
            ));
        }
    }

    let Some(item) = ItemType::by_name(&item_name) else {
        panic!(
            "'{item_name}' is not in the registry — a pack item needs scripts/with-test-mods.sh"
        );
    };
    // The harness covers BOTH held item render kinds: a bbmodel item through
    // `held_model` and the player rig; a sprite item (tools!) through the
    // extruded item3d chains. The pack's tool swings are sprites, so the
    // sprite column pair is not a fallback — it is the subject.

    // Tiles are the game's 16:9, and the first-person columns are rendered
    // at that same aspect: shot square, a right-anchored hand falls off the
    // edge and every pose reads as "invisible" for a reason that is the
    // harness's fault, not the pose's.
    const TILE_W: usize = 640;
    const TILE_H: usize = 360;
    let aspect = TILE_W as f32 / TILE_H as f32;
    // 5 columns: the two first-person fists, the third-person body from the
    // front and the side, and the third-person item ISOLATED and framed.
    // The isolated shot is what settles ORIENTATION questions — "is it upside
    // down" is unanswerable against a torso that hides half the silhouette,
    // and a crop of the body shot is guesswork about where the item went.
    let cols = 5usize;
    let rows = states.len();
    let (w, h) = (TILE_W * cols, TILE_H * rows);
    // A flat mid-grey ground tone: a shield lost against the background is
    // the exact failure this harness exists to catch, so the backdrop must
    // not be near the art's own colours.
    let bg = [64u8, 68, 78];
    let mut color = vec![0u8; w * h * 3];
    for px in color.chunks_mut(3) {
        px.copy_from_slice(&bg);
    }
    let mut zbuf = vec![f32::INFINITY; w * h];

    let (atlas, aw, ah) = petramond_world::block_model::atlas().texture();
    let model = player_model();
    let skin = (model.texture_rgba.as_slice(), model.tex_w, model.tex_h);

    // Rasterize `verts` (already in the space `mvp` expects) into one tile.
    let raster = |verts: &[ItemVertex],
                  tex: (&[u8], u32, u32),
                  mvp: Mat4,
                  col: usize,
                  row: usize,
                  zbuf: &mut [f32],
                  color: &mut [u8]| {
        let (pix, tw, th) = tex;
        for tri in verts.chunks_exact(3) {
            let mut s = [[0f32; 3]; 3];
            let mut ok = true;
            for (dst, v) in s.iter_mut().zip(tri) {
                let c = mvp * glam::Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
                if c.w <= 1e-6 {
                    ok = false;
                    break;
                }
                let n = c / c.w;
                *dst = [
                    (n.x * 0.5 + 0.5) * TILE_W as f32,
                    (1.0 - (n.y * 0.5 + 0.5)) * TILE_H as f32,
                    n.z,
                ];
            }
            if !ok {
                continue;
            }
            let (x0, y0, x1, y1, x2, y2) = (s[0][0], s[0][1], s[1][0], s[1][1], s[2][0], s[2][1]);
            let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
            if area.abs() < 1e-6 {
                continue;
            }
            let inv_area = 1.0 / area;
            let minx = x0.min(x1).min(x2).floor().max(0.0) as usize;
            let maxx = x0.max(x1).max(x2).ceil().min(TILE_W as f32 - 1.0) as usize;
            let miny = y0.min(y1).min(y2).floor().max(0.0) as usize;
            let maxy = y0.max(y1).max(y2).ceil().min(TILE_H as f32 - 1.0) as usize;
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
                    let gx = col * TILE_W + x;
                    let gy = row * TILE_H + y;
                    let li = gy * w + gx;
                    if z >= zbuf[li] {
                        continue;
                    }
                    let u = w0 * tri[0].uv[0] + w1 * tri[1].uv[0] + w2 * tri[2].uv[0];
                    let v = w0 * tri[0].uv[1] + w1 * tri[1].uv[1] + w2 * tri[2].uv[1];
                    let tx = (u * tw as f32).clamp(0.0, tw as f32 - 1.0) as u32;
                    let ty = (v * th as f32).clamp(0.0, th as f32 - 1.0) as u32;
                    let ti = ((ty * tw + tx) * 4) as usize;
                    if pix[ti + 3] < 128 {
                        continue;
                    }
                    let shade = w0 * tri[0].shade + w1 * tri[1].shade + w2 * tri[2].shade;
                    zbuf[li] = z;
                    let o = (gy * w + gx) * 3;
                    for c in 0..3 {
                        color[o + c] = (pix[ti + c] as f32 * shade).min(255.0) as u8;
                    }
                }
            }
        }
    };

    // The held bbmodel's triangles under an arbitrary model matrix.
    let model_tris =
        |kind: petramond_world::block_model::BlockModelKind, m: Mat4| -> Vec<ItemVertex> {
            let (mut v, mut i) = (Vec::new(), Vec::new());
            crate::item_model::build_block_model_item(
                kind,
                m,
                DynLight::FULL,
                LightEnv::IDENTITY,
                None,
                &mut v,
                &mut i,
            );
            i.iter().map(|&i| v[i as usize]).collect()
        };

    for (row, (label, pose, bones)) in states.iter().enumerate() {
        let view = HeldItemView {
            item: Some(item),
            pose: *pose,
            ..Default::default()
        };

        // The third-person body build is SHARED by both render kinds: the
        // arm and its pose are item-kind-independent; only the item's own
        // transform (and its texture sheet) differ.
        let inst = crate::PlayerRenderInstance {
            pos: Vec3::ZERO,
            body_yaw: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            anim_time: 0.0,
            // `HELD_POSE_WALK=<0..1>` drives the gait, so a STANCE can be
            // checked against a moving body — the arm that holds a guard must
            // not swing with the stride, and a still shot cannot show that.
            walk_weight: std::env::var("HELD_POSE_WALK")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(0.0),
            sneak_weight: 0.0,
            sleeping: false,
            seated: false,
            hurt: 0.0,
            skylight: 63,
            blocklight: BlockLight6::DARK,
            bones: crate::BoneRange {
                start: 0,
                len: bones.len() as u32,
            },
        };
        let (mut bv, mut bi) = (Vec::new(), Vec::new());
        let (_, hand, off_hand) = crate::player_model::build_player_body(
            model,
            LightEnv::IDENTITY,
            &inst,
            bones,
            &view,
            &view,
            &mut bv,
            &mut bi,
        );
        let body: Vec<ItemVertex> = bi
            .iter()
            .map(|&i| {
                let v = bv[i as usize];
                ItemVertex {
                    pos: v.pos,
                    uv: v.uv,
                    shade: v.shade,
                    tint: [1.0; 3],
                }
            })
            .collect();
        let hand = crate::player_model::posed_hand(hand, &pose.third_person, false);
        let off_hand = crate::player_model::posed_hand(off_hand, &pose.third_person, true);
        let proj = Mat4::perspective_rh(40f32.to_radians(), aspect, 0.05, 20.0);

        // The SPRITE chain: the item's own extrusion, textured from its own
        // sheet, at the held anchor and the fist transforms the game uses.
        if let ItemRenderKind::Sprite(tile) = item.render_kind() {
            let texture = format!("{}.png", tile.name());
            for (col, mvp) in [
                held_sprite(&view, aspect).expect("sprite item").1,
                held_sprite_off(&view, aspect).expect("sprite item").1,
            ]
            .into_iter()
            .enumerate()
            {
                raster_sprite_cell(
                    tile,
                    &texture,
                    mvp,
                    (TILE_W, TILE_H),
                    (col, row),
                    5,
                    &mut color,
                );
            }
            for (col, eye) in [Vec3::new(0.0, 1.25, 2.9), Vec3::new(2.9, 1.25, 0.0)]
                .into_iter()
                .enumerate()
            {
                let mvp = proj * Mat4::look_at_rh(eye, Vec3::new(0.0, 0.95, 0.0), Vec3::Y);
                raster(&body, skin, mvp, col + 2, row, &mut zbuf, &mut color);
                for t in [
                    crate::player_model::held_sprite_transform(hand),
                    crate::player_model::held_sprite_transform_off(off_hand),
                ] {
                    raster_sprite_cell(
                        tile,
                        &texture,
                        mvp * t,
                        (TILE_W, TILE_H),
                        (col + 2, row),
                        5,
                        &mut color,
                    );
                }
            }
            // The solo shot: the item alone, framed on its bounds, from the
            // same front axis — the orientation read a torso hides.
            let mut base: Vec<ItemVertex> = Vec::new();
            crate::item_model::build_extruded_item_lit(
                tile,
                DynLight::FULL,
                crate::lighting::LightEnv::IDENTITY,
                &mut base,
            );
            let model = crate::player_model::held_sprite_transform(hand);
            let world: Vec<ItemVertex> = base
                .iter()
                .map(|v| {
                    let p = model * glam::Vec4::new(v.pos[0], v.pos[1], v.pos[2], 1.0);
                    ItemVertex {
                        pos: [p.x, p.y, p.z],
                        uv: v.uv,
                        shade: v.shade,
                        tint: v.tint,
                    }
                })
                .collect();
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &world {
                lo = lo.min(Vec3::from(v.pos));
                hi = hi.max(Vec3::from(v.pos));
            }
            let centre = (lo + hi) * 0.5;
            let span = (hi - lo).max_element().max(0.05);
            let solo_eye = centre + Vec3::new(0.0, 0.0, span * 1.9);
            let solo_mvp = Mat4::perspective_rh(40f32.to_radians(), aspect, 0.01, 50.0)
                * Mat4::look_at_rh(solo_eye, centre, Vec3::Y);
            raster_sprite_cell(
                tile,
                &texture,
                solo_mvp,
                (TILE_W, TILE_H),
                (4, row),
                5,
                &mut color,
            );
            println!("row {row}: {label} (sprite)");
            continue;
        }

        let ItemRenderKind::Model(kind) = item.render_kind() else {
            panic!(
                "'{item_name}' is a block-cube item — the preview shoots sprite and bbmodel items"
            );
        };

        // --- columns 0/1: FIRST PERSON, each fist, the real hand camera ----
        for (col, mvp) in [
            held_model(&view, aspect).expect("bbmodel item").1,
            held_model_off(&view, aspect).expect("bbmodel item").1,
        ]
        .into_iter()
        .enumerate()
        {
            // The unit-cube geometry is rebased by the MVP itself.
            raster(
                &model_tris(kind, Mat4::IDENTITY),
                (atlas, aw, ah),
                mvp,
                col,
                row,
                &mut zbuf,
                &mut color,
            );
        }

        // --- columns 2/3: THIRD PERSON, front and side, both fists ---------
        for (col, eye) in [Vec3::new(0.0, 1.25, 2.9), Vec3::new(2.9, 1.25, 0.0)]
            .into_iter()
            .enumerate()
        {
            let mvp = proj * Mat4::look_at_rh(eye, Vec3::new(0.0, 0.95, 0.0), Vec3::Y);
            let col = col + 2;
            raster(&body, skin, mvp, col, row, &mut zbuf, &mut color);
            for m in [
                crate::player_model::held_model_transform(hand, kind),
                crate::player_model::held_model_transform_off(off_hand, kind),
            ] {
                raster(
                    &model_tris(kind, m),
                    (atlas, aw, ah),
                    mvp,
                    col,
                    row,
                    &mut zbuf,
                    &mut color,
                );
            }
        }

        // The main hand's item alone, from the same front camera, but framed
        // on its own bounds so the whole silhouette fills the tile.
        let solo = model_tris(kind, crate::player_model::held_model_transform(hand, kind));
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &solo {
            lo = lo.min(Vec3::from(v.pos));
            hi = hi.max(Vec3::from(v.pos));
        }
        let centre = (lo + hi) * 0.5;
        // Back off far enough that the tallest extent fits the vertical FOV,
        // looking along the SAME axis as the front body shot so "up" means the
        // same thing in both.
        let span = (hi - lo).max_element().max(0.05);
        let eye = centre + Vec3::new(0.0, 0.0, span * 1.9);
        let mvp = Mat4::perspective_rh(40f32.to_radians(), aspect, 0.01, 20.0)
            * Mat4::look_at_rh(eye, centre, Vec3::Y);
        raster(&solo, (atlas, aw, ah), mvp, 4, row, &mut zbuf, &mut color);
        println!("row {row}: {label}");
    }

    // Tile borders + a first-person crosshair: without a frame of reference
    // "a bit left of centre" is unreadable, and the crosshair is where the
    // player is actually looking.
    for r in 0..rows {
        for c in 0..cols {
            for x in 0..TILE_W {
                for y in [0usize, TILE_H - 1] {
                    let o = ((r * TILE_H + y) * w + c * TILE_W + x) * 3;
                    color[o..o + 3].copy_from_slice(&[20, 20, 24]);
                }
            }
            for y in 0..TILE_H {
                for x in [0usize, TILE_W - 1] {
                    let o = ((r * TILE_H + y) * w + c * TILE_W + x) * 3;
                    color[o..o + 3].copy_from_slice(&[20, 20, 24]);
                }
            }
            if c < 2 {
                let (cx, cy) = (c * TILE_W + TILE_W / 2, r * TILE_H + TILE_H / 2);
                for d in 0..8usize {
                    for (x, y) in [(cx + d, cy), (cx - d, cy), (cx, cy + d), (cx, cy - d)] {
                        let o = (y * w + x) * 3;
                        color[o..o + 3].copy_from_slice(&[235, 235, 235]);
                    }
                }
            }
        }
    }

    let out = format!("/tmp/held_pose_{}.png", item_name.replace(':', "_"));
    image::save_buffer(
        &out,
        &color,
        w as u32,
        h as u32,
        image::ExtendedColorType::Rgb8,
    )
    .expect("save preview");
    println!(
        "wrote {out}  (rows = states, cols = 1P main | 1P off | 3P front | 3P side | 3P item alone)"
    );
}

/// One corner of an axis-aligned box, `i` in `0..8` (bit per axis).
#[cfg(test)]
fn corner(from: Vec3, to: Vec3, i: usize) -> Vec3 {
    Vec3::new(
        if i & 1 == 0 { from.x } else { to.x },
        if i & 2 == 0 { from.y } else { to.y },
        if i & 4 == 0 { from.z } else { to.z },
    )
}

/// A pose in the OFF hand is the exact mirror image of the same pose in the
/// main hand, arm stance included.
///
/// TWO independent mirror rules meet on one body: the caller mirrors an
/// authored ARM (negate the y/z rotations, Blockbench's own left-hand rule)
/// and the engine mirrors the HELD POSE by conjugating the attach frame.
/// Where they meet is precisely where an item ends up facing backwards on the
/// wrong side — and only for the hand nobody checks first. The numbers below
/// are arbitrary on purpose: this pins the ALGEBRA, not anybody's tuned pose.
#[test]
fn an_off_hand_pose_is_the_mirror_of_the_main_hands() {
    use crate::lighting::LightEnv;
    use petramond::player::model::player_model;
    use petramond_world::block_model::DisplayTransform;
    use petramond_world::item::{ItemRenderKind, ItemType};
    use petramond_world::light::BlockLight6;

    let item = ItemType::by_name("petramond:wooden_bucket").expect("engine bbmodel item");
    let ItemRenderKind::Model(kind) = item.render_kind() else {
        panic!("the bucket is a bbmodel item")
    };
    // Deliberately arbitrary, and deliberately NOT any pack's tuned pose: a
    // number here that happened to match one in `mods-src/` would read as a
    // value to keep in sync, which is how the engine ends up owning a mod's
    // content again. All three axes are non-zero so nothing cancels by luck.
    let stance_shoulder = [41.0, 27.0, -13.0];
    let stance_elbow = [6.0, -11.0, -32.0];
    let held = DisplayTransform {
        rotation: [-63.0, 21.0, 17.0],
        translation: [2.0, 4.0, 7.0],
        ..Default::default()
    };
    let model = player_model();
    let bone = |name: &str, r: [f32; 3], mirror: bool| crate::BoneOffset {
        bone: model.bone_named(name).expect(name),
        rotation: if mirror { [r[0], -r[1], -r[2]] } else { r },
        translation: [0.0; 3],
        hold: true,
    };

    // The item's corners for one hand, in the body's own space.
    let corners = |off_side: bool| -> Vec<Vec3> {
        let bones = if off_side {
            vec![
                bone("right_shoulder", stance_shoulder, true),
                bone("right_elbow", stance_elbow, true),
            ]
        } else {
            vec![
                bone("left_shoulder", stance_shoulder, false),
                bone("left_elbow", stance_elbow, false),
            ]
        };
        let inst = crate::PlayerRenderInstance {
            pos: Vec3::ZERO,
            body_yaw: 0.0,
            head_yaw: 0.0,
            head_pitch: 0.0,
            anim_time: 0.0,
            walk_weight: 0.0,
            sneak_weight: 0.0,
            sleeping: false,
            seated: false,
            hurt: 0.0,
            skylight: 63,
            blocklight: BlockLight6::DARK,
            bones: crate::BoneRange {
                start: 0,
                len: bones.len() as u32,
            },
        };
        let view = crate::HeldItemView {
            item: Some(item),
            ..Default::default()
        };
        let (mut bv, mut bi) = (Vec::new(), Vec::new());
        let (_, hand, off_hand) = crate::player_model::build_player_body(
            model,
            LightEnv::IDENTITY,
            &inst,
            &bones,
            &view,
            &view,
            &mut bv,
            &mut bi,
        );
        let m = if off_side {
            crate::player_model::held_model_transform_off(
                crate::player_model::posed_hand(off_hand, &held, true),
                kind,
            )
        } else {
            crate::player_model::held_model_transform(
                crate::player_model::posed_hand(hand, &held, false),
                kind,
            )
        };
        let geo = petramond_world::block_model::instance(kind);
        let fp = Vec3::new(
            geo.footprint[0] as f32,
            geo.footprint[1] as f32,
            geo.footprint[2] as f32,
        );
        let uspan = fp.max_element().max(1.0);
        geo.cubes
            .iter()
            .flat_map(|c| {
                (0..8).map(move |i| {
                    let p = Mat4::from_translation(c.origin)
                        * Mat4::from_quat(petramond_world::bbmodel::euler_quat(c.rotation))
                        * Mat4::from_translation(-c.origin);
                    (p.transform_point3(corner(c.from, c.to, i)) - fp * 0.5) / uspan
                })
            })
            .map(|unit| m.transform_point3(unit))
            .collect()
    };

    let main = corners(false);
    let off = corners(true);
    assert_eq!(main.len(), off.len());
    assert!(!main.is_empty(), "the item has geometry");

    // The body faces engine +Z at yaw 0, so mirroring the world X flips left
    // for right. A mirrored corner SET is the same set — the reflection
    // renumbers which corner is which — so each main corner is matched to its
    // nearest mirrored partner rather than by index.
    let worst = main
        .iter()
        .map(|m| {
            let want = Vec3::new(-m.x, m.y, m.z);
            off.iter()
                .map(|o| (*o - want).length())
                .fold(f32::MAX, f32::min)
        })
        .fold(0.0f32, f32::max);
    assert!(
        worst < 0.01,
        "the off-hand pose is not the main hand's mirror: {worst:.4} blocks out"
    );

    // Non-vacuous on both counts: the item is genuinely off the body's centre
    // line (so an x-flip says something), and the two hands are genuinely on
    // opposite sides of it (so this is not comparing a set with itself).
    let centre = |v: &[Vec3]| v.iter().fold(Vec3::ZERO, |a, p| a + *p) / v.len() as f32;
    let (cm, co) = (centre(&main), centre(&off));
    assert!(
        cm.x.abs() > 0.1 && (cm.x + co.x).abs() < 0.01,
        "hands at {cm:?} and {co:?} are not opposite sides of the body"
    );
}

/// The pose sandwich honors the sample's own pivot: rotation happens ABOUT
/// `origin` (the pivot maps to itself plus only the keyed translation), and
/// distances from the pivot are preserved. Pinned on synthetic data because
/// the obvious "simplification" — dropping the `T(origin)…T(-origin)`
/// sandwich for a plain `T*R` — reads identically for zero-origin files and
/// silently unhinges every curve that keys a pivot.
#[test]
fn pose_matrix_rotates_about_the_sampled_origin() {
    let origin = [1.0, -6.0, 2.0];
    let hinged = pose_matrix(PoseSample {
        rotation: [0.0, 0.0, 90.0],
        translation: [0.0; 3],
        origin,
    });
    let pivot = Vec3::from(origin);
    assert!(
        (hinged.transform_point3(pivot) - pivot).length() < 1e-5,
        "an untranslated sample holds its pivot fixed"
    );
    let tip = pivot + Vec3::new(0.0, 4.0, 0.0);
    let swung = hinged.transform_point3(tip);
    assert!(
        ((swung - pivot).length() - 4.0).abs() < 1e-4,
        "the tip stays on the hinge radius"
    );
    assert!(
        (swung - tip).length() > 1.0,
        "the tip actually travelled around the pivot"
    );

    // The keyed translation rides on top of the hinge, not inside it.
    let carried = pose_matrix(PoseSample {
        rotation: [0.0, 0.0, 90.0],
        translation: [0.5, 0.0, 0.0],
        origin,
    });
    assert!(
        (carried.transform_point3(pivot) - (pivot + Vec3::new(0.5, 0.0, 0.0))).length() < 1e-5,
        "translation displaces the pivot verbatim"
    );
}
