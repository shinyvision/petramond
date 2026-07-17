use std::collections::HashMap;

use glam::Vec3;
use serde_json::Value;

use super::texture::TextureSheet;
use super::{Animation, Bone, Cube, Keyframe};

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
pub(super) fn parse_faces(faces: Option<&Value>, sheet: &TextureSheet) -> [Option<[f32; 4]>; 6] {
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
                        out[slot] = Some([u0, v0, u1, v1]);
                    }
                }
            }
        }
    }
    out
}

/// Parse the `animations` array into named [`Animation`]s with per-bone rotation
/// tracks. Animators are keyed by group uuid -> bone index.
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
        // Blockbench loop modes: "loop" loops; "once"/"hold" (or absent) play once.
        // Some formats store a bool. Anything but a looping signal counts as one-shot.
        let looping = match a.get("loop") {
            Some(Value::String(s)) => s == "loop",
            Some(Value::Bool(b)) => *b,
            _ => false,
        };
        let mut tracks: HashMap<usize, Vec<Keyframe>> = HashMap::new();
        if let Some(animators) = a.get("animators").and_then(Value::as_object) {
            for (uuid, animator) in animators {
                let Some(&bone) = bone_by_uuid.get(uuid) else {
                    continue;
                };
                let Some(kfs) = animator.get("keyframes").and_then(Value::as_array) else {
                    continue;
                };
                let mut track: Vec<Keyframe> = kfs
                    .iter()
                    .filter(|k| k.get("channel").and_then(Value::as_str) == Some("rotation"))
                    .filter_map(|k| {
                        let time = k.get("time").and_then(Value::as_f64)? as f32;
                        let dp = k.get("data_points").and_then(Value::as_array)?.first()?;
                        let rot = Vec3::new(
                            dp.get("x").and_then(num).unwrap_or(0.0),
                            dp.get("y").and_then(num).unwrap_or(0.0),
                            dp.get("z").and_then(num).unwrap_or(0.0),
                        );
                        Some(Keyframe { time, rot })
                    })
                    .collect();
                if !track.is_empty() {
                    track.sort_by(|a, b| a.time.total_cmp(&b.time));
                    tracks.insert(bone, track);
                }
            }
        }
        out.insert(
            name,
            Animation {
                length,
                looping,
                tracks,
            },
        );
    }
    out
}

/// The project `resolution` `(width, height)` — the UV divisor for a texture that
/// carries no `uv_width`/`uv_height` of its own. Falls back to 16.
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
