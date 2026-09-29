use std::collections::HashMap;

use glam::Vec3;
use serde_json::Value;

use super::texture::TextureSheet;
use super::{
    Animation, BezierHandles, Bone, Channel, Cube, Interpolation, Keyframe, Marker, MarkerKind,
};

pub(super) fn walk_outliner(
    node: &Value,
    parent_bone: Option<usize>,
    bone_by_uuid: &HashMap<String, usize>,
    cube_by_uuid: &HashMap<String, usize>,
    bones: &mut [Bone],
    cubes: &mut [Cube],
) {
    match node {
        Value::String(uuid) => {
            if let (Some(&ci), Some(pb)) = (cube_by_uuid.get(uuid), parent_bone) {
                cubes[ci].bone = pb;
            }
        }
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

pub(super) fn parse_faces(
    faces: Option<&Value>,
    sheet: &TextureSheet,
) -> [Option<super::FaceUv>; 6] {
    const NAMES: [(&str, usize); 6] = [
        ("east", 0),
        ("west", 1),
        ("up", 2),
        ("down", 3),
        ("south", 4),
        ("north", 5),
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
                        let (u0, v0) = r.remap(v[0], v[1]);
                        let (u1, v1) = r.remap(v[2], v[3]);
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
        let (looping, hold) = match a.get("loop") {
            Some(Value::String(s)) => (s == "loop", s == "hold"),
            Some(Value::Bool(b)) => (*b, false),
            _ => (false, false),
        };
        let overrides = a.get("override").and_then(Value::as_bool).unwrap_or(false);
        let mut anim = Animation::new(length, looping, hold).with_overrides(overrides);
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

pub(super) fn interpolation_named(name: Option<&str>) -> Interpolation {
    match name {
        Some("catmullrom") => Interpolation::CatmullRom,
        Some("bezier") => Interpolation::Bezier,
        Some("step") => Interpolation::Step,
        _ => Interpolation::Linear,
    }
}

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
    for dp in k
        .get("data_points")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if kind == MarkerKind::Timeline {
            for name in dp
                .get("script")
                .and_then(Value::as_str)
                .into_iter()
                .flat_map(script_lines)
            {
                anim.push_marker(Marker { time, kind, name });
            }
        } else if let Some(effect) = dp
            .get("effect")
            .and_then(Value::as_str)
            .filter(|e| !e.is_empty())
        {
            anim.push_marker(Marker {
                time,
                kind,
                name: effect.to_string(),
            });
        }
    }
}

pub(super) fn script_lines(script: &str) -> impl Iterator<Item = String> + '_ {
    script
        .lines()
        .map(|line| line.trim().trim_end_matches(';').trim())
        .filter(|line| !line.is_empty())
        .map(str::to_string)
}

pub(super) fn per_texture_uv_size(root: &Value) -> bool {
    root.get("meta")
        .and_then(|m| m.get("model_format"))
        .and_then(Value::as_str)
        .is_some_and(|f| f.starts_with("bedrock"))
}

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

pub(super) fn arr3(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(Vec3::new(num(&a[0])?, num(&a[1])?, num(&a[2])?))
}

pub(super) fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
}
