//! Bedrock `.animation.json` clip libraries: the NAME-keyed export Blockbench
//! writes (File → Export → Bedrock Animations), so a pack can ship clips for a
//! rig it does not own — a `.bbmodel`'s own animators are keyed by group uuid,
//! which only that file knows.
//!
//! Parsing mirrors Blockbench's importer: arrays flip into Blockbench's
//! internal axes (rotation X and Y and position X negate; a uniform number
//! does not), `pre`/`post` objects become two-point keys, a key whose leaving
//! value equals the next key's arriving value becomes a `step`, `lerp_mode`
//! sets interpolation, and the effect tracks become markers.

use glam::Vec3;
use serde_json::Value;

use super::parse::{interpolation_named, num, script_lines};
use super::{Animation, Channel, Interpolation, Keyframe, Marker, MarkerKind};

/// Parse every animation in a Bedrock animation file, resolving bone names
/// through `bone_named` (tried verbatim, then lowercased; a name the rig
/// lacks drops that bone's tracks). A document without an `animations`
/// object is an error; a malformed value inside one reads as 0, like the
/// `.bbmodel` parser.
pub fn parse_library(
    text: &str,
    bone_named: impl Fn(&str) -> Option<usize>,
) -> Result<Vec<(String, Animation)>, String> {
    let root: Value = serde_json::from_str(text).map_err(|e| format!("json: {e}"))?;
    let anims = root
        .get("animations")
        .and_then(Value::as_object)
        .ok_or("no `animations` object")?;
    let mut out = Vec::with_capacity(anims.len());
    for (name, a) in anims {
        let (looping, hold) = match a.get("loop") {
            Some(Value::Bool(b)) => (*b, false),
            Some(Value::String(s)) => (false, s == "hold_on_last_frame"),
            _ => (false, false),
        };
        let mut anim = Animation::new(0.0, looping, hold);
        let mut last_key = 0.0f32;
        if let Some(bones) = a.get("bones").and_then(Value::as_object) {
            for (bone_name, channels) in bones {
                let Some(bone) =
                    bone_named(bone_name).or_else(|| bone_named(&bone_name.to_lowercase()))
                else {
                    continue;
                };
                let Some(channels) = channels.as_object() else {
                    continue;
                };
                for (label, value) in channels {
                    let (channel, flip) = match label.as_str() {
                        "rotation" => (Channel::Rotation, Vec3::new(-1.0, -1.0, 1.0)),
                        "position" => (Channel::Position, Vec3::new(-1.0, 1.0, 1.0)),
                        _ => continue,
                    };
                    let keys = channel_keys(value, flip);
                    last_key = keys.iter().map(|k| k.time).fold(last_key, f32::max);
                    anim.set_track(bone, channel, keys);
                }
            }
        }
        for (field, kind) in [
            ("sound_effects", MarkerKind::Sound),
            ("particle_effects", MarkerKind::Particle),
            ("timeline", MarkerKind::Timeline),
        ] {
            let Some(track) = a.get(field).and_then(Value::as_object) else {
                continue;
            };
            for (stamp, entry) in track {
                let Ok(time) = stamp.trim().parse::<f32>() else {
                    continue;
                };
                last_key = last_key.max(time);
                let items: &[Value] = match entry {
                    Value::Array(items) => items,
                    one => std::slice::from_ref(one),
                };
                for item in items {
                    if kind == MarkerKind::Timeline {
                        for name in item.as_str().into_iter().flat_map(script_lines) {
                            anim.push_marker(Marker { time, kind, name });
                        }
                    } else if let Some(effect) = item
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
        }
        anim.length = a.get("animation_length").and_then(num).unwrap_or(last_key);
        out.push((name.clone(), anim));
    }
    Ok(out)
}

/// One channel's keys in any of Bedrock's spellings: a constant (array, number
/// or string) or a `pre`/`post` object at time 0, or a map of timestamps.
fn channel_keys(value: &Value, flip: Vec3) -> Vec<Keyframe> {
    let mut keys: Vec<Keyframe> = match value {
        Value::Object(map) if !map.contains_key("post") && !map.contains_key("pre") => map
            .iter()
            .filter_map(|(stamp, v)| key(stamp.trim().parse().ok()?, v, flip))
            .collect(),
        other => key(0.0, other, flip).into_iter().collect(),
    };
    keys.sort_by(|a, b| a.time.total_cmp(&b.time));
    // A key stepping into the next one; the last key steps only when the
    // pair right before it did.
    let mut last_was_step = false;
    for i in 0..keys.len() {
        if i + 1 < keys.len() {
            last_was_step = keys[i + 1].split && keys[i].post == keys[i + 1].pre;
            if last_was_step {
                keys[i + 1].pre = keys[i + 1].post;
                keys[i + 1].split = false;
                keys[i].interpolation = Interpolation::Step;
            }
        } else if last_was_step {
            keys[i].interpolation = Interpolation::Step;
        }
    }
    keys
}

fn key(time: f32, value: &Value, flip: Vec3) -> Option<Keyframe> {
    let Value::Object(o) = value else {
        return point(value, flip).map(|v| Keyframe::new(time, v, Interpolation::Linear));
    };
    let identical = matches!(
        (o.get("pre"), o.get("post")),
        (Some(Value::Array(a)), Some(Value::Array(b))) if a == b
    );
    let pre = o.get("pre").and_then(|v| point(v, flip));
    let post = o.get("post").and_then(|v| point(v, flip));
    let (pre, post, split) = match (pre, post) {
        (Some(p), Some(q)) if !identical => (p, q, true),
        (Some(p), _) => (p, p, false),
        (None, Some(q)) => (q, q, false),
        (None, None) => return None,
    };
    Some(Keyframe {
        time,
        pre,
        post,
        split,
        interpolation: interpolation_named(o.get("lerp_mode").and_then(Value::as_str)),
        bezier: None,
    })
}

fn point(v: &Value, flip: Vec3) -> Option<Vec3> {
    match v {
        Value::Array(a) if a.len() == 3 => Some(
            Vec3::new(
                num(&a[0]).unwrap_or(0.0),
                num(&a[1]).unwrap_or(0.0),
                num(&a[2]).unwrap_or(0.0),
            ) * flip,
        ),
        Value::Number(_) | Value::String(_) => Some(Vec3::splat(num(v).unwrap_or(0.0))),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
