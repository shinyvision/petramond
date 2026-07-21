use super::parse::base64_decode;
use super::*;

fn owl() -> Model {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/models/owl.bbmodel"
    ));
    Model::load(src).expect("owl.bbmodel parses")
}

fn sheep() -> Model {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/models/sheep.bbmodel"
    ));
    Model::load(src).expect("sheep.bbmodel parses")
}

/// The compiled `.llmob` payload round-trips the model with full fidelity: same
/// geometry, same texture and — crucially — the same posed animation. Pins the
/// serialization *contract* (every field survives), compared against the original, so
/// it pins no editable table value.
#[test]
fn compiled_model_roundtrips_with_full_fidelity() {
    let m = owl();
    let bytes = bincode::serialize(&m).expect("model serializes");
    let m2: Model = bincode::deserialize(&bytes).expect("model deserializes");

    assert_eq!(m.cubes.len(), m2.cubes.len());
    assert_eq!(m.bones.len(), m2.bones.len());
    assert_eq!((m.tex_w, m.tex_h), (m2.tex_w, m2.tex_h));
    assert_eq!(m.texture_rgba, m2.texture_rgba, "texture bytes survive");
    let mut names1: Vec<&String> = m.animations.keys().collect();
    let mut names2: Vec<&String> = m2.animations.keys().collect();
    names1.sort();
    names2.sort();
    assert_eq!(names1, names2, "animation set survives");

    // Behaviour preserved: a pose from the round-tripped model matches the original
    // (proves bones, pivots, parents and keyframes all survived intact).
    let (walk1, walk2) = (m.animation("walk").unwrap(), m2.animation("walk").unwrap());
    for &t in &[0.0f32, 0.17, 0.33, 0.5] {
        for (a, b) in m.pose(walk1, t).iter().zip(m2.pose(walk2, t).iter()) {
            assert!(a.abs_diff_eq(*b, 1e-6), "posed transforms match at t={t}");
        }
    }
}

/// Base64-encode (standard alphabet, padded) — test-only counterpart of
/// [`base64_decode`], for building synthetic embedded textures.
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A 1×1 PNG of one solid colour as a Blockbench `source` data URI.
fn one_pixel_texture(rgba: [u8; 4]) -> String {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png encodes");
    format!("data:image/png;base64,{}", base64_encode(&png.into_inner()))
}

/// A model whose elements paint from DIFFERENT textures must keep every element
/// visible: all textures land in one stacked sheet and each face's UVs remap into
/// its own texture's band. (Regression: only the first texture was decoded, so
/// every other texture's elements sampled transparent texels and vanished.)
#[test]
fn multi_texture_faces_remap_into_stacked_sheet() {
    let red = one_pixel_texture([255, 0, 0, 255]);
    let blue = one_pixel_texture([0, 0, 255, 255]);
    let src = format!(
        r#"{{
            "resolution": {{ "width": 16, "height": 16 }},
            "textures": [
                {{ "uv_width": 16, "uv_height": 16, "source": "{red}" }},
                {{ "uv_width": 16, "uv_height": 16, "source": "{blue}" }}
            ],
            "elements": [
                {{ "uuid": "a", "type": "cube", "from": [0,0,0], "to": [1,1,1],
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }},
                {{ "uuid": "b", "type": "cube", "from": [2,0,0], "to": [3,1,1],
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 1 }} }} }}
            ],
            "outliner": ["a", "b"]
        }}"#
    );
    let m = Model::load(&src).expect("two-texture model parses");

    // Both 1×1 textures stack into a 1×2 sheet, red band above blue.
    assert_eq!((m.tex_w, m.tex_h), (1, 2));
    assert_eq!(&m.texture_rgba[0..4], &[255, 0, 0, 255], "row 0 is red");
    assert_eq!(&m.texture_rgba[4..8], &[0, 0, 255, 255], "row 1 is blue");

    // Cube a's face spans the top (red) half, cube b's the bottom (blue) half.
    let uv_a = m.cubes[0].faces[2].expect("cube a up face");
    let uv_b = m.cubes[1].faces[2].expect("cube b up face");
    assert_eq!(uv_a, [0.0, 0.0, 1.0, 0.5]);
    assert_eq!(uv_b, [0.0, 0.5, 1.0, 1.0]);
}

/// Overlay layers (a skin's hat/jacket/sleeves) author the SAME box as their
/// base cube plus an `inflate`; dropping it makes the two coincident and
/// z-fight. The loader must bake inflate into the box (UVs untouched).
#[test]
fn element_inflate_grows_the_cube_box() {
    let tex = one_pixel_texture([255, 255, 255, 255]);
    let src = format!(
        r#"{{
            "resolution": {{ "width": 16, "height": 16 }},
            "textures": [{{ "uv_width": 16, "uv_height": 16, "source": "{tex}" }}],
            "elements": [
                {{ "uuid": "base", "type": "cube", "from": [0,0,0], "to": [4,4,4],
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }},
                {{ "uuid": "layer", "type": "cube", "from": [0,0,0], "to": [4,4,4],
                   "inflate": 0.25,
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }}
            ],
            "outliner": ["base", "layer"]
        }}"#
    );
    let m = Model::load(&src).expect("inflated model parses");
    assert_eq!(m.cubes[0].from, Vec3::ZERO, "base box untouched");
    assert_eq!(m.cubes[0].to, Vec3::splat(4.0));
    assert_eq!(
        m.cubes[1].from,
        Vec3::splat(-0.25),
        "inflate grows every face outward"
    );
    assert_eq!(m.cubes[1].to, Vec3::splat(4.25));
    // UVs are NOT rescaled by inflate.
    assert_eq!(m.cubes[0].faces[2], m.cubes[1].faces[2]);
}

#[test]
fn parses_cubes_bones_and_texture() {
    let m = owl();
    assert_eq!(
        m.cubes.len(),
        11,
        "head, beak, body, 2 wings, 2 legs, 2 feet, 2 tail"
    );
    assert!(m.bones.len() >= 6, "owl/head/lwing/rwing/lleg/rleg bones");
    // Embedded 32x32 texture decodes to RGBA.
    assert_eq!((m.tex_w, m.tex_h), (32, 32));
    assert_eq!(m.texture_rgba.len(), 32 * 32 * 4);
}

#[test]
fn cube_names_are_parsed_and_survive_the_compiled_roundtrip() {
    // Cube names carry gameplay meaning (a sheep's fleece cubes are all named
    // `wool` so the renderer can hide them while shorn), so the loader must keep
    // them and the compiled `.llmob` must round-trip them.
    let m = sheep();
    let wool = m.cubes.iter().filter(|c| c.name == "wool").count();
    assert!(wool > 0, "the sheep fixture authors `wool` cubes");

    let bytes = bincode::serialize(&m).expect("model serializes");
    let m2: Model = bincode::deserialize(&bytes).expect("model deserializes");
    let names = |m: &Model| -> Vec<String> { m.cubes.iter().map(|c| c.name.clone()).collect() };
    assert_eq!(names(&m), names(&m2), "cube names survive the round-trip");
}

#[test]
fn every_cube_has_a_resolved_bone() {
    let m = owl();
    for (i, c) in m.cubes.iter().enumerate() {
        assert!(c.bone < m.bones.len(), "cube {i} bone unresolved");
    }
}

#[test]
fn walk_animation_is_present_and_loops_half_a_second() {
    let m = owl();
    let walk = m.animation("walk").expect("walk animation");
    assert!((walk.length - 0.5).abs() < 1e-6);
    // At least the two legs are animated.
    assert!(
        walk.tracks.len() >= 2,
        "legs (and head) have rotation tracks"
    );
}

#[test]
fn pose_swings_the_legs_in_antiphase_over_the_cycle() {
    let m = owl();
    let walk = m.animation("walk").unwrap();
    // Identify the two leg bones by name.
    let leg_bones: Vec<usize> = m
        .bones
        .iter()
        .enumerate()
        .filter(|(_, b)| b.name == "lleg" || b.name == "rleg")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(leg_bones.len(), 2, "two leg bones");

    // A point at the foot, transformed by each leg's pose, should move forward
    // (±Z) and the two legs should be on opposite sides at t=0 (antiphase).
    let foot = glam::Vec4::new(0.3, 0.0, 0.75, 1.0);
    let pose0 = m.pose(walk, 0.0);
    let z: Vec<f32> = leg_bones.iter().map(|&b| (pose0[b] * foot).z).collect();
    assert!(
        (z[0] - z[1]).abs() > 0.05,
        "legs should be split fore/aft at t=0: {z:?}"
    );

    // Quarter cycle later the swing should have reversed (antiphase over time).
    let pose_q = m.pose(walk, 0.25);
    let zq: Vec<f32> = leg_bones.iter().map(|&b| (pose_q[b] * foot).z).collect();
    assert!(
        (z[0] - zq[0]).abs() > 0.05,
        "a leg should swing between t=0 and t=0.25: {z:?} vs {zq:?}"
    );
}

#[test]
fn pose_loops_over_the_length() {
    let m = owl();
    let walk = m.animation("walk").unwrap();
    let a = m.pose(walk, 0.1);
    let b = m.pose(walk, 0.1 + walk.length);
    for (x, y) in a.iter().zip(b.iter()) {
        assert!(x.abs_diff_eq(*y, 1e-4), "pose must loop");
    }
}

#[test]
fn non_looping_animation_holds_its_final_frame() {
    let m = owl();
    // The owl's idle animations are Blockbench `once` (non-looping).
    let idle = m.idle_animation(0).expect("owl has idle animations");
    assert!(!idle.looping, "owl idle animations are one-shot");
    // Past the end it holds the final frame instead of wrapping to the start.
    let at_end = m.pose(idle, idle.length);
    let past_end = m.pose(idle, idle.length * 3.0);
    for (x, y) in at_end.iter().zip(past_end.iter()) {
        assert!(
            x.abs_diff_eq(*y, 1e-5),
            "one-shot pose holds the final frame, not loops"
        );
    }
}

#[test]
fn exposes_head_bone_idle_anims_and_affects_bone() {
    let m = owl();
    assert!(m.head_bone().is_some(), "owl has a head bone");
    // `affects_bone`: the walk animation drives the leg bones (its whole purpose).
    let lleg = m
        .bones
        .iter()
        .position(|b| b.name == "lleg")
        .expect("lleg bone");
    assert!(
        m.animation("walk").unwrap().affects_bone(lleg),
        "walk animates the legs"
    );
    // The owl ships idle_* animations, exposed by a stable index.
    assert!(
        m.idle_animation(0).is_some(),
        "idle animations exposed by index"
    );
    assert!(
        m.idle_animation(999).is_none(),
        "out-of-range idle index is None"
    );
}

#[test]
fn rest_pose_includes_static_group_rotations() {
    let m = sheep();
    let ear = m
        .bones
        .iter()
        .position(|b| b.name == "ear_left")
        .expect("sheep has a rotated ear bone");
    assert!(
        m.bones[ear].rotation.length_squared() > 0.0,
        "fixture must exercise authored group rotation"
    );

    let rest = m.rest_pose();
    let pivot = m.bones[ear].pivot;
    let marker = pivot + Vec3::X;
    assert!(
        !rest[ear].transform_point3(marker).abs_diff_eq(marker, 1e-5),
        "rest pose applies the authored bone rotation"
    );
    assert!(
        rest[ear].transform_point3(pivot).abs_diff_eq(pivot, 1e-5),
        "bone rotation is about the authored pivot"
    );
}

#[test]
fn head_look_propagates_to_child_bones() {
    let m = sheep();
    let head = m.head_bone().expect("sheep has a head bone");
    let ear = m
        .bones
        .iter()
        .position(|b| b.name == "ear_left")
        .expect("sheep has a child ear bone");
    assert!(
        m.is_descendant_of(ear, head),
        "the ear is authored under the head"
    );

    let mut pose = m.rest_pose();
    let ear_before = pose[ear];
    m.apply_head_look(&mut pose, head, 0.7, 0.2);

    assert!(
        !pose[head].abs_diff_eq(Mat4::IDENTITY, 1e-5),
        "head-look changes the head pose"
    );
    assert!(
        !pose[ear].abs_diff_eq(ear_before, 1e-5),
        "head-look carries through descendant bones"
    );
}

#[test]
fn bone_rotation_composes_over_the_pose_and_propagates() {
    // Unlike head-look (which replaces), apply_bone_rotation must COMPOSE: the
    // rotated head keeps its animated/rest orientation plus the delta, the pivot
    // stays fixed, and descendants (the ear) carry the delta too.
    let m = sheep();
    let head = m.head_bone().expect("sheep has a head bone");
    let ear = m
        .bones
        .iter()
        .position(|b| b.name == "ear_left")
        .expect("sheep has a child ear bone");

    let mut pose = m.rest_pose();
    let head_before = pose[head];
    let ear_before = pose[ear];
    let pivot = m.bones[head].pivot;
    let pivot_world_before = head_before.transform_point3(pivot);
    m.apply_bone_rotation(&mut pose, head, Quat::from_rotation_x(0.6));

    assert!(
        !pose[head].abs_diff_eq(head_before, 1e-5),
        "the delta rotates the bone"
    );
    assert!(
        pose[head]
            .transform_point3(pivot)
            .abs_diff_eq(pivot_world_before, 1e-4),
        "the rotation is about the bone's posed pivot"
    );
    assert!(
        !pose[ear].abs_diff_eq(ear_before, 1e-5),
        "the delta carries through descendant bones"
    );

    // Composability: a zero rotation is a no-op (pure compose, no replace).
    let mut pose2 = m.rest_pose();
    m.apply_bone_rotation(&mut pose2, head, Quat::IDENTITY);
    for (a, b) in pose2.iter().zip(m.rest_pose().iter()) {
        assert!(a.abs_diff_eq(*b, 1e-6), "identity delta leaves the pose");
    }
}

#[test]
fn empty_model_is_safe() {
    let m = Model::empty();
    assert!(m.cubes.is_empty());
    assert_eq!((m.tex_w, m.tex_h), (1, 1));
    assert_eq!(m.texture_rgba.len(), 4);
}

#[test]
fn base64_roundtrips_known_vector() {
    // "Man" -> "TWFu" (classic base64 example).
    assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
    // Padding + whitespace are ignored.
    assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
    assert_eq!(base64_decode("TW Fu\n").unwrap(), b"Man");
}

/// Position keyframes translate the bone (and its cubes) in the parent's
/// frame — a clip authored with a head that bodily dips (the lamb's `eat`)
/// must move geometry, not just parse away silently.
#[test]
fn position_tracks_translate_the_bone() {
    let tex = one_pixel_texture([255, 255, 255, 255]);
    let src = format!(
        r#"{{
            "resolution": {{ "width": 16, "height": 16 }},
            "textures": [{{ "uv_width": 16, "uv_height": 16, "source": "{tex}" }}],
            "elements": [
                {{ "uuid": "c", "type": "cube", "from": [0,0,0], "to": [4,4,4],
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }}
            ],
            "groups": [{{ "uuid": "g", "name": "head", "origin": [2,2,2] }}],
            "outliner": [{{ "uuid": "g", "name": "head", "origin": [2,2,2], "children": ["c"] }}],
            "animations": [{{
                "name": "dip", "loop": "once", "length": 1.0,
                "animators": {{ "g": {{ "name": "head", "type": "bone", "keyframes": [
                    {{ "channel": "position", "time": 0,
                       "data_points": [{{ "x": "0", "y": "0", "z": "0" }}] }},
                    {{ "channel": "position", "time": 1.0,
                       "data_points": [{{ "x": "0", "y": "-3", "z": "0" }}] }}
                ] }} }}
            }}]
        }}"#
    );
    let m = Model::load(&src).expect("position-animated model parses");
    let dip = m.animation("dip").expect("dip parses");
    assert!(dip.affects_bone(0), "a position-only track claims the bone");
    let rest = m.pose(dip, 0.0);
    let low = m.pose(dip, 1.0);
    let p0 = rest[0].transform_point3(Vec3::splat(2.0));
    let p1 = low[0].transform_point3(Vec3::splat(2.0));
    assert!((p1.y - (p0.y - 3.0)).abs() < 1e-5, "the bone dips 3 units: {p0} -> {p1}");
    assert!((p1.x - p0.x).abs() < 1e-5 && (p1.z - p0.z).abs() < 1e-5);
    // Clamped past the end (a held `once` frame keeps the offset).
    let held = m.pose(dip, 2.0);
    assert!((held[0].transform_point3(Vec3::splat(2.0)).y - p1.y).abs() < 1e-5);
}


/// The serialized bytes of a canonical compiled model, pinned against
/// [`CompiledAsset::FORMAT_VERSION`]. The cache header treats an equal version
/// as decode-compatible, so ANY change to the serialized layout that ships
/// without a bump lets stale `.llmob` files mis-decode under the new structs —
/// usually a clean failure (recompile), but an unlucky byte order decodes into
/// valid-but-garbage models (the 2026-07-21 invisible-hushjaw bug: a species
/// silently baking zero triangles). Every collection in the canonical model
/// holds at most ONE entry, so HashMap iteration order cannot vary the bytes.
///
/// If this fails you changed the compiled layout (or `Model::load`'s output):
/// bump `FORMAT_VERSION` and update `GOLDEN_VERSION` + `GOLDEN_HEX` from the
/// failure message. Never update the golden without the bump.
#[test]
fn compiled_model_layout_change_requires_a_format_version_bump() {
    use crate::asset_cache::CompiledAsset;

    const GOLDEN_VERSION: u32 = 6;
    const GOLDEN_HEX: &str = concat!(
        "01000000000000000400000000000000726f6f7400000040000000400000004000000000",
        "00000000000000000001000000000000000400000000000000626f647900000000000000",
        "000000000000008040000080400000804000000000000000000000000000000000000000",
        "0000000000000000000000000000000100000000000000000000803f0000803f00000001",
        "00000000000000090000000000000069646c655f776176650000803f0001000000000000",
        "00000000000000000001000000000000000000803e00002041000000000000a040010000",
        "0000000000000000000000000001000000000000000000003f00000000000040c0000000",
        "000100000000000000090000000000000069646c655f776176650400000000000000ff00",
        "00ff0100000001000000",
    );

    let tex = one_pixel_texture([255, 0, 0, 255]);
    let src = format!(
        r#"{{
            "resolution": {{ "width": 16, "height": 16 }},
            "textures": [{{ "uv_width": 16, "uv_height": 16, "source": "{tex}" }}],
            "elements": [
                {{ "uuid": "c", "type": "cube", "name": "body", "from": [0,0,0], "to": [4,4,4],
                   "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }}
            ],
            "groups": [{{ "uuid": "g", "name": "root", "origin": [2,2,2] }}],
            "outliner": [{{ "uuid": "g", "name": "root", "origin": [2,2,2], "rotation": [0,10,0], "children": ["c"] }}],
            "animations": [{{
                "name": "idle_wave", "loop": "once", "length": 1.0,
                "animators": {{ "g": {{ "name": "root", "type": "bone", "keyframes": [
                    {{ "channel": "rotation", "time": 0.25,
                       "data_points": [{{ "x": "10", "y": "0", "z": "5" }}] }},
                    {{ "channel": "position", "time": 0.5,
                       "data_points": [{{ "x": "0", "y": "-3", "z": "0" }}] }}
                ] }} }}
            }}]
        }}"#
    );
    let m = <Model as CompiledAsset>::compile(src.as_bytes()).expect("canonical model compiles");
    let bytes = bincode::serialize(&m).expect("serializes");
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    assert!(
        Model::FORMAT_VERSION == GOLDEN_VERSION && hex == GOLDEN_HEX,
        "compiled .llmob layout or version changed.\n\
         FORMAT_VERSION: {} (golden {GOLDEN_VERSION})\n\
         serialized canonical model:\n{hex}\n\
         If any serialized struct or Model::load output changed, bump FORMAT_VERSION \
         in `impl CompiledAsset for Model` and update GOLDEN_VERSION + GOLDEN_HEX \
         together. A layout change WITHOUT the bump lets stale caches mis-decode \
         into garbage models (invisible mobs).",
        Model::FORMAT_VERSION,
    );
}

/// The rest-pose bounds must cover the posed geometry — the renderer derives
/// mob frustum-cull volumes from them, and an undersized answer clips live
/// mobs out of view (the old hardcoded 1.2 m pad bug, inverted).
#[test]
fn rest_bounds_cover_the_posed_geometry() {
    let m = owl();
    let (min, max) = m.rest_bounds();
    assert!(max.x > min.x && max.y > min.y && max.z > min.z, "{min} {max}");

    // Every posed cube corner lies inside the answered box.
    let pose = m.rest_pose();
    for cube in &m.cubes {
        let bone = pose.get(cube.bone).copied().unwrap_or(Mat4::IDENTITY);
        let s_cube = Mat4::from_translation(cube.origin)
            * Mat4::from_quat(euler_quat(cube.rotation))
            * Mat4::from_translation(-cube.origin);
        let p = (bone * s_cube).transform_point3(cube.from);
        assert!(
            p.cmpge(min - 1e-4).all() && p.cmple(max + 1e-4).all(),
            "posed corner {p} escapes bounds {min}..{max}"
        );
    }

    let empty = Model::empty();
    assert_eq!(empty.rest_bounds(), (Vec3::ZERO, Vec3::ZERO));
}
