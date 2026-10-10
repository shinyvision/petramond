use super::parse::base64_decode;
use super::*;

fn owl() -> Model {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/owl.bbmodel"
    ));
    Model::load(src).expect("owl.bbmodel parses")
}

fn sheep() -> Model {
    let src = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/models/sheep.bbmodel"
    ));
    Model::load(src).expect("sheep.bbmodel parses")
}

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

    let (walk1, walk2) = (m.animation("walk").unwrap(), m2.animation("walk").unwrap());
    for &t in &[0.0f32, 0.17, 0.33, 0.5] {
        for (a, b) in m.pose(walk1, t).iter().zip(m2.pose(walk2, t).iter()) {
            assert!(a.abs_diff_eq(*b, 1e-6), "posed transforms match at t={t}");
        }
    }
}

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

pub(crate) fn one_pixel_texture(rgba: [u8; 4]) -> String {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut png, image::ImageFormat::Png)
        .expect("png encodes");
    format!("data:image/png;base64,{}", base64_encode(&png.into_inner()))
}

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

    assert_eq!((m.tex_w, m.tex_h), (1, 2));
    assert_eq!(&m.texture_rgba[0..4], &[255, 0, 0, 255], "row 0 is red");
    assert_eq!(&m.texture_rgba[4..8], &[0, 0, 255, 255], "row 1 is blue");

    let uv_a = m.cubes[0].faces[2].expect("cube a up face");
    let uv_b = m.cubes[1].faces[2].expect("cube b up face");
    assert_eq!(uv_a.uv, [0.0, 0.0, 1.0, 0.5]);
    assert_eq!(uv_b.uv, [0.0, 0.5, 1.0, 1.0]);
}

#[test]
fn a_faces_rotation_is_parsed_and_permutes_its_corner_uvs() {
    let src = r#"{
        "meta": { "format_version": "4.5", "model_format": "free", "box_uv": false },
        "resolution": { "width": 16, "height": 16 },
        "textures": [{ "source": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==" }],
        "elements": [
            { "uuid": "a", "type": "cube", "from": [0,0,0], "to": [1,1,1],
              "faces": { "up":   { "uv": [0,0,16,16], "texture": 0, "rotation": 180 },
                         "down": { "uv": [0,0,16,16], "texture": 0, "rotation": 90 },
                         "north":{ "uv": [0,0,16,16], "texture": 0 } } }
        ],
        "outliner": ["a"]
    }"#;
    let m = Model::load(src).expect("model parses");
    let up = m.cubes[0].faces[2].expect("up face");
    let down = m.cubes[0].faces[3].expect("down face");
    let north = m.cubes[0].faces[5].expect("north face");
    assert_eq!((up.rot, down.rot, north.rot), (2, 1, 0), "degrees / 90");

    let plain = north.corner_uv();
    assert_eq!(plain, [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    assert_eq!(up.corner_uv(), [plain[2], plain[3], plain[0], plain[1]]);
    assert_eq!(down.corner_uv(), [plain[1], plain[2], plain[3], plain[0]]);

    let mut spun = north;
    spun.rot = 4;
    assert_eq!(spun.corner_uv(), plain);

    assert_eq!(up.with_uv([0.25, 0.25, 0.5, 0.5]).rot, up.rot);
}

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
    assert_eq!(m.cubes[0].faces[2], m.cubes[1].faces[2]);
}

#[test]
fn cube_names_are_parsed_and_survive_the_compiled_roundtrip() {
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
fn empty_model_is_safe() {
    let m = Model::empty();
    assert!(m.cubes.is_empty());
    assert_eq!((m.tex_w, m.tex_h), (1, 1));
    assert_eq!(m.texture_rgba.len(), 4);
}

#[test]
fn base64_roundtrips_known_vector() {
    assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
    assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
    assert_eq!(base64_decode("TW Fu\n").unwrap(), b"Man");
}

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
    assert!(
        (p1.y - (p0.y - 3.0)).abs() < 1e-5,
        "the bone dips 3 units: {p0} -> {p1}"
    );
    assert!((p1.x - p0.x).abs() < 1e-5 && (p1.z - p0.z).abs() < 1e-5);
    let held = m.pose(dip, 2.0);
    assert!((held[0].transform_point3(Vec3::splat(2.0)).y - p1.y).abs() < 1e-5);
}

#[test]
fn compiled_model_layout_change_requires_a_format_version_bump() {
    use crate::asset_cache::CompiledAsset;

    const PINNED_VERSION: u32 = 11;
    const COMPILED_HEX: &str = "01000000000000000400000000000000726f6f740000004000000040000000400000000000000000000000000001000000000000000400000000000000626f6479000000000000000000000000000080400000804000008040000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000803f0000803f000000000100000000000000090000000000000069646c655f776176650000803f0000000100000000000000000000000000000001000000000000000000803e00002041000000000000a04000002041000000000000a04000000000000001000000000000000000003f00000000000040c00000000000000000000040c00000000000000000000000000000000000000100000000000000090000000000000069646c655f776176650400000000000000ff0000ff0100000001000000";

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
        Model::FORMAT_VERSION == PINNED_VERSION && hex == COMPILED_HEX,
        "compiled .llmob layout or version changed.\n\
         FORMAT_VERSION: {} (pinned {PINNED_VERSION})\n\
         serialized canonical model:\n{hex}\n\
         If any serialized struct or Model::load output changed, bump FORMAT_VERSION \
         in `impl CompiledAsset for Model` and update PINNED_VERSION + COMPILED_HEX \
         together. A layout change WITHOUT the bump lets stale caches mis-decode \
         into garbage models (invisible mobs).",
        Model::FORMAT_VERSION,
    );
}

#[test]
fn an_override_clip_replaces_earlier_layers_on_the_bones_it_keys() {
    let tex = one_pixel_texture([255, 0, 0, 255]);
    let clip = |name: &str, over: bool, x: f32| {
        format!(
            r#"{{ "name": "{name}", "loop": "loop", "override": {over}, "length": 1.0,
                "animators": {{ "g": {{ "name": "root", "type": "bone", "keyframes": [
                    {{ "channel": "rotation", "time": 0, "data_points": [{{ "x": "{x}", "y": "0", "z": "0" }}] }}
                ] }} }} }}"#
        )
    };
    let src = format!(
        r#"{{
            "resolution": {{ "width": 16, "height": 16 }},
            "textures": [{{ "uv_width": 16, "uv_height": 16, "source": "{tex}" }}],
            "elements": [{{ "uuid": "c", "type": "cube", "name": "body", "from": [0,0,0], "to": [4,4,4],
                "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }}],
            "groups": [{{ "uuid": "g", "name": "root", "origin": [0,0,0] }}],
            "outliner": [{{ "uuid": "g", "name": "root", "origin": [0,0,0], "children": ["c"] }}],
            "animations": [{}, {}, {}]
        }}"#,
        clip("walk", false, 40.0),
        clip("add", false, 10.0),
        clip("stance", true, 10.0),
    );
    let m = Model::load(&src).expect("model loads");
    let angle = |pose: &[Mat4]| {
        pose[0]
            .to_scale_rotation_translation()
            .1
            .to_euler(glam::EulerRot::XYZ)
            .0
    };
    let (walk, add, stance) = (
        m.animation("walk").unwrap(),
        m.animation("add").unwrap(),
        m.animation("stance").unwrap(),
    );
    let summed = angle(&m.pose_layers(&[(walk, 0.0, 1.0), (add, 0.0, 1.0)]));
    let replaced = angle(&m.pose_layers(&[(walk, 0.0, 1.0), (stance, 0.0, 1.0)]));
    let half = angle(&m.pose_layers(&[(walk, 0.0, 1.0), (stance, 0.0, 0.5)]));
    assert!(
        (summed - 50f32.to_radians()).abs() < 1e-4,
        "additive layers sum"
    );
    assert!(
        (replaced - 10f32.to_radians()).abs() < 1e-4,
        "an override replaces"
    );
    assert!(
        (half - 25f32.to_radians()).abs() < 1e-4,
        "a half-weight override crossfades from what was posed"
    );
}

#[test]
fn rest_bounds_cover_the_posed_geometry() {
    let m = owl();
    let (min, max) = m.rest_bounds();
    assert!(
        max.x > min.x && max.y > min.y && max.z > min.z,
        "{min} {max}"
    );

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
