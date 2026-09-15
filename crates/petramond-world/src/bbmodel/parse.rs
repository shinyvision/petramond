use std::collections::HashMap;

use glam::Vec3;
use serde_json::Value;

use super::texture::TextureSheet;
use super::{
    Animation, BezierHandles, Bone, Channel, Cube, Interpolation, Keyframe, Marker, MarkerKind,
};

/// Recursively assign bone parents + cube bones from one `outliner` node. A node
/// is either a cube-uuid string (a leaf) or a group object (`uuid` + `children`).
pub(super) fn walk_outliner(
    node: &Value,
    parent_bone: Option<usize>,
    bone_by_uuid: &HashMap<String, usize>,
    cube_by_uuid: &HashMap<String, usize>,
    bones: &mut [Bone],
    cubes: &mut [Cube],
) {
    match node {
        // A bare string is a cube uuid parented to the current bone.
        Value::String(uuid) => {
            if let (Some(&ci), Some(pb)) = (cube_by_uuid.get(uuid), parent_bone) {
                cubes[ci].bone = pb;
            }
        }
        // An object is a group (bone); recurse into its children.
        Value::Object(_) => {
            let uuid = node.get("uuid").and_then(Value::as_str).unwrap_or("");
            let this_bone = bone_by_uuid.get(uuid).copied();
            if let Some(b) = this_bone {
                bones[b].parent = parent_bone;
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    walk_outliner(child, this_bone, bone_by_uuid, cube_by_uuid, bones, cubes);
                }
            }
        }
        _ => {}
    }
}

/// Parse one element's `faces` map into the `Face::ALL`-ordered UV array, each
/// face's UVs normalized into its referenced texture's band of the sheet.
pub(super) fn parse_faces(
    faces: Option<&Value>,
    sheet: &TextureSheet,
) -> [Option<super::FaceUv>; 6] {
    // Blockbench face name -> our `Face::ALL` slot (PosX, NegX, PosY, NegY, PosZ, NegZ).
    const NAMES: [(&str, usize); 6] = [
        ("east", 0),  // +X
        ("west", 1),  // -X
        ("up", 2),    // +Y
        ("down", 3),  // -Y
        ("south", 4), // +Z
        ("north", 5), // -Z
    ];
    let mut out = [None; 6];
    let Some(faces) = faces else { return out };
    for (name, slot) in NAMES {
        let Some(face) = faces.get(name) else {
            continue;
        };
        if let Some(uv) = face.get("uv").and_then(Value::as_array) {
            if uv.len() == 4 {
                let v: Vec<f32> = uv.iter().filter_map(num).collect();
                if v.len() == 4 {
                    let tex = face
                        .get("texture")
                        .and_then(Value::as_u64)
                        .map(|i| i as usize);
                    if let Some(r) = sheet.rect(tex) {
                        // Normalize into the texture's sheet band; keep raw corner
                        // order so per-face flips (a reversed rect) reproduce on
                        // render.
                        let (u0, v0) = r.remap(v[0], v[1]);
                        let (u1, v1) = r.remap(v[2], v[3]);
                        // Blockbench's per-face `rotation` is degrees clockwise
                        // on the face; a quarter turn swaps the u/v axes, so it
                        // cannot be folded into the rect and rides along.
                        let rot = face
                            .get("rotation")
                            .and_then(num)
                            .map(|d| (d / 90.0).rem_euclid(4.0).round() as u8)
                            .unwrap_or(0);
                        out[slot] = Some(super::FaceUv {
                            uv: [u0, v0, u1, v1],
                            rot,
                        });
                    }
                }
            }
        }
    }
    out
}

/// Parse the `animations` array into named [`Animation`]s: per-bone rotation
/// and position tracks (animators keyed by group uuid → bone index) with every
/// key's interpolation, plus the Effects animator's markers.
pub(super) fn parse_animations(
    root: &Value,
    bone_by_uuid: &HashMap<String, usize>,
) -> HashMap<String, Animation> {
    let mut out = HashMap::new();
    let Some(anims) = root.get("animations").and_then(Value::as_array) else {
        return out;
    };
    for a in anims {
        let name = a
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let length = a.get("length").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        // Blockbench loop modes: "loop" loops, "hold" plays once and keeps its
        // last frame, "once" (or absent) plays once. Some formats store a bool.
        let (looping, hold) = match a.get("loop") {
            Some(Value::String(s)) => (s == "loop", s == "hold"),
            Some(Value::Bool(b)) => (*b, false),
            _ => (false, false),
        };
        let mut anim = Animation::new(length, looping, hold);
        if let Some(animators) = a.get("animators").and_then(Value::as_object) {
            for (uuid, animator) in animators {
                let Some(kfs) = animator.get("keyframes").and_then(Value::as_array) else {
                    continue;
                };
                if animator.get("type").and_then(Value::as_str) == Some("effect") {
                    for k in kfs {
                        push_effect_markers(&mut anim, k);
                    }
                    continue;
                }
                let Some(&bone) = bone_by_uuid.get(uuid) else {
                    continue;
                };
                for (channel, label) in [
                    (Channel::Rotation, "rotation"),
                    (Channel::Position, "position"),
                ] {
                    let keys = kfs
                        .iter()
                        .filter(|k| k.get("channel").and_then(Value::as_str) == Some(label))
                        .filter_map(parse_keyframe)
                        .collect();
                    anim.set_track(bone, channel, keys);
                }
            }
        }
        out.insert(name, anim);
    }
    out
}

/// One bone keyframe: its time, one or two data points (pre/post), its
/// interpolation, and — on a Bezier key — its handles.
fn parse_keyframe(k: &Value) -> Option<Keyframe> {
    let time = k.get("time").and_then(num)?;
    let points = k.get("data_points").and_then(Value::as_array)?;
    let point = |dp: &Value| {
        Vec3::new(
            dp.get("x").and_then(num).unwrap_or(0.0),
            dp.get("y").and_then(num).unwrap_or(0.0),
            dp.get("z").and_then(num).unwrap_or(0.0),
        )
    };
    let pre = point(points.first()?);
    let post = points.last().map(point).unwrap_or(pre);
    let interpolation = interpolation_named(k.get("interpolation").and_then(Value::as_str));
    let bezier = (interpolation == Interpolation::Bezier).then(|| {
        let d = BezierHandles::DEFAULT;
        BezierHandles {
            left_time: arr3(k.get("bezier_left_time")).unwrap_or(d.left_time),
            left_value: arr3(k.get("bezier_left_value")).unwrap_or(d.left_value),
            right_time: arr3(k.get("bezier_right_time")).unwrap_or(d.right_time),
            right_value: arr3(k.get("bezier_right_value")).unwrap_or(d.right_value),
        }
    });
    Some(Keyframe {
        time,
        pre,
        post,
        split: points.len() > 1,
        interpolation,
        bezier,
    })
}

/// A Blockbench interpolation name; anything unrecognised is linear, the
/// Blockbench default.
pub(super) fn interpolation_named(name: Option<&str>) -> Interpolation {
    match name {
        Some("catmullrom") => Interpolation::CatmullRom,
        Some("bezier") => Interpolation::Bezier,
        Some("step") => Interpolation::Step,
        _ => Interpolation::Linear,
    }
}

/// One keyframe of the Effects animator as markers: a timeline key gives one
/// marker per script line, a sound or particle key one per named effect.
fn push_effect_markers(anim: &mut Animation, k: &Value) {
    let Some(time) = k.get("time").and_then(num) else {
        return;
    };
    let kind = match k.get("channel").and_then(Value::as_str) {
        Some("timeline") => MarkerKind::Timeline,
        Some("sound") => MarkerKind::Sound,
        Some("particle") => MarkerKind::Particle,
        _ => return,
    };
    for dp in k.get("data_points").and_then(Value::as_array).into_iter().flatten() {
        if kind == MarkerKind::Timeline {
            for name in dp.get("script").and_then(Value::as_str).into_iter().flat_map(script_lines) {
                anim.push_marker(Marker { time, kind, name });
            }
        } else if let Some(effect) = dp.get("effect").and_then(Value::as_str).filter(|e| !e.is_empty()) {
            anim.push_marker(Marker {
                time,
                kind,
                name: effect.to_string(),
            });
        }
    }
}

/// A timeline script's lines as marker names: trimmed, trailing `;` dropped,
/// empty lines skipped.
pub(super) fn script_lines(script: &str) -> impl Iterator<Item = String> + '_ {
    script
        .lines()
        .map(|line| line.trim().trim_end_matches(';').trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

/// Whether face UVs are authored in each TEXTURE's own pixel space rather than
/// the project `resolution`. Bedrock formats give every texture its own UV
/// size; a Java block model has ONE project-wide UV space, and the `uv_width`
/// Blockbench writes beside a Java texture is merely that image's natural size.
/// Reading it as an override there scales every face by the ratio between the
/// image and the project — a 32² texture in a 128² project authors UVs up to
/// 128, and dividing those by 32 sends them clean off the sheet.
pub(super) fn per_texture_uv_size(root: &Value) -> bool {
    root.get("meta")
        .and_then(|m| m.get("model_format"))
        .and_then(Value::as_str)
        .is_some_and(|f| f.starts_with("bedrock"))
}

/// The project `resolution` `(width, height)` — the UV divisor for every face
/// unless [`per_texture_uv_size`] says the textures carry their own. Falls
/// back to 16.
pub(super) fn project_resolution(root: &Value) -> (f32, f32) {
    if let Some(res) = root.get("resolution") {
        let w = res.get("width").and_then(Value::as_f64).unwrap_or(16.0);
        let h = res.get("height").and_then(Value::as_f64).unwrap_or(16.0);
        if w > 0.0 && h > 0.0 {
            return (w as f32, h as f32);
        }
    }
    (16.0, 16.0)
}

/// Minimal standard-alphabet base64 decoder (skips `=` padding + whitespace). Kept
/// in-tree so the loader needs no base64 dependency.
pub(super) fn base64_decode(s: &str) -> Option<Vec<u8>> {
    fn val(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some((c - b'A') as u32),
            b'a'..=b'z' => Some((c - b'a' + 26) as u32),
            b'0'..=b'9' => Some((c - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut buf = 0u32;
    let mut bits = 0u32;
    for &c in s.as_bytes() {
        if c == b'=' || c.is_ascii_whitespace() {
            continue;
        }
        let v = val(c)?;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Some(out)
}

/// A `[x, y, z]` JSON array -> `Vec3` (accepts numbers; tolerant of stringified).
pub(super) fn arr3(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(Vec3::new(num(&a[0])?, num(&a[1])?, num(&a[2])?))
}

/// A JSON value as `f32`, accepting both numbers and numeric strings (Blockbench
/// keyframe data points are stored as strings, e.g. `"20"`).
pub(super) fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
}
