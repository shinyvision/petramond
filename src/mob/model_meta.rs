//! Lightweight, sim-side scan of a `.bbmodel` for the idle-animation info the AI
//! needs without depending on the render-side parser: each `idle_*` animation's
//! length and whether it loops.
//!
//! The renderer's [`Model`](crate::render::bbmodel) owns the full geometry/animation
//! parse; this is a tiny name-sorted pass (matching the renderer's idle index order)
//! so the `mob` layer stays independent of `render`. Both agree on what counts as an
//! idle animation (a name starting `idle_`) and on the loop modes.

use std::collections::HashMap;

use serde_json::Value;

use crate::mathh::Vec3;

/// What the AI needs to know about one `idle_*` animation.
#[derive(Copy, Clone, Debug)]
pub struct IdleAnimMeta {
    /// Length in seconds (so a one-shot idle is played for exactly its length).
    pub length: f32,
    /// Whether it loops (Blockbench `loop: "loop"`); `once`/`hold` do not.
    pub looping: bool,
}

/// The `idle_*` animations of a `.bbmodel`, name-sorted (the order the renderer
/// indexes them), each with its length + loop mode. Empty on any parse error.
pub fn idle_anims(model_src: &str) -> Vec<IdleAnimMeta> {
    let Ok(root) = serde_json::from_str::<Value>(model_src) else {
        return Vec::new();
    };
    let Some(anims) = root.get("animations").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut idle: Vec<(&str, IdleAnimMeta)> = anims
        .iter()
        .filter_map(|a| {
            let name = a.get("name").and_then(Value::as_str)?;
            if !name.starts_with("idle_") {
                return None;
            }
            let length = a.get("length").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let looping = match a.get("loop") {
                Some(Value::String(s)) => s == "loop",
                Some(Value::Bool(b)) => *b,
                _ => false,
            };
            Some((name, IdleAnimMeta { length, looping }))
        })
        .collect();
    idle.sort_by(|a, b| a.0.cmp(b.0));
    idle.into_iter().map(|(_, m)| m).collect()
}

/// One bone of the sim-side skeleton. The death ragdoll treats each bone as a rigid
/// *body* — the box `[bbox_min, bbox_max]` covering its geometry — whose corners are
/// simulated as particles. A rigid rotation is recovered from the corners each tick
/// (shape matching), so a bone tumbles and falls over physically; `pivot` is the joint
/// where it attaches to its parent (see [`crate::mob::ragdoll`]).
#[derive(Copy, Clone, Debug)]
pub struct SkBone {
    pub pivot: Vec3,
    /// Axis-aligned box covering the bone's cubes (model space), regularised to a
    /// minimum thickness so even a flat cube is a genuine 3D body. A geometry-less group
    /// gets a small box around its `pivot`.
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub parent: Option<usize>,
}

/// The bone hierarchy of a `.bbmodel`, in the SAME index order the renderer's
/// [`Model`](crate::render::bbmodel) builds (the `groups` array order, parents from the
/// `outliner`), so a sim-computed per-bone pose drops straight into the render bake.
#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub bones: Vec<SkBone>,
}

/// Smallest half-extent (model units) a bone's box is inflated to on each axis, so a
/// flat cube still has 3D thickness for the ragdoll to tumble realistically.
const MIN_HALF: f32 = 0.5;

/// Scan a `.bbmodel` for its bone hierarchy (pivots, parents, and a bounding box per
/// bone from its cube geometry), mirroring the renderer's bone construction so
/// indices/pivots line up. Empty on any parse error (the ragdoll then degrades to no
/// flop rather than crashing).
pub fn skeleton(model_src: &str) -> Skeleton {
    let Ok(root) = serde_json::from_str::<Value>(model_src) else {
        return Skeleton::default();
    };
    // Bones from `groups`.
    let mut bones = Vec::new();
    let mut by_uuid: HashMap<String, usize> = HashMap::new();
    if let Some(groups) = root.get("groups").and_then(Value::as_array) {
        for g in groups {
            let uuid = g.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
            let pivot = arr3(g.get("origin")).unwrap_or(Vec3::ZERO);
            by_uuid.insert(uuid, bones.len());
            bones.push(SkBone { pivot, bbox_min: pivot, bbox_max: pivot, parent: None });
        }
    }
    // Cube boxes from `elements`, keyed by uuid (for each bone's bounding box).
    let mut cube_box: HashMap<String, (Vec3, Vec3)> = HashMap::new();
    if let Some(elements) = root.get("elements").and_then(Value::as_array) {
        for e in elements {
            let Some(uuid) = e.get("uuid").and_then(Value::as_str) else {
                continue;
            };
            let from = arr3(e.get("from")).unwrap_or(Vec3::ZERO);
            let to = arr3(e.get("to")).unwrap_or(from);
            cube_box.insert(uuid.to_string(), (from.min(to), from.max(to)));
        }
    }
    // Walk the outliner: set parents, and grow each bone's box by its cubes.
    let mut bbox = vec![(Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)); bones.len()];
    if let Some(outliner) = root.get("outliner").and_then(Value::as_array) {
        for node in outliner {
            walk_outliner(node, None, &by_uuid, &cube_box, &mut bones, &mut bbox);
        }
    }
    // Finalise each bone's box: the union of its cubes (regularised to a minimum
    // thickness), or a small box around the pivot for a geometry-less group.
    for (b, (mn, mx)) in bbox.into_iter().enumerate() {
        let (centre, half) = if mx.cmpge(mn).all() {
            ((mn + mx) * 0.5, ((mx - mn) * 0.5).max(Vec3::splat(MIN_HALF)))
        } else {
            (bones[b].pivot, Vec3::splat(MIN_HALF))
        };
        bones[b].bbox_min = centre - half;
        bones[b].bbox_max = centre + half;
    }
    Skeleton { bones }
}

/// Set each bone's parent from one `outliner` node and grow its bounding box by its
/// cubes. Group objects are bones; bare string children are cube uuids (this bone's
/// geometry).
fn walk_outliner(
    node: &Value,
    parent: Option<usize>,
    by_uuid: &HashMap<String, usize>,
    cube_box: &HashMap<String, (Vec3, Vec3)>,
    bones: &mut [SkBone],
    bbox: &mut [(Vec3, Vec3)],
) {
    match node {
        // A cube uuid: grows the current (parent) bone's bounding box.
        Value::String(uuid) => {
            if let (Some(b), Some((f, t))) = (parent, cube_box.get(uuid)) {
                bbox[b].0 = bbox[b].0.min(*f);
                bbox[b].1 = bbox[b].1.max(*t);
            }
        }
        Value::Object(_) => {
            let this = node
                .get("uuid")
                .and_then(Value::as_str)
                .and_then(|u| by_uuid.get(u).copied());
            if let Some(b) = this {
                bones[b].parent = parent;
            }
            if let Some(children) = node.get("children").and_then(Value::as_array) {
                for child in children {
                    walk_outliner(child, this, by_uuid, cube_box, bones, bbox);
                }
            }
        }
        _ => {}
    }
}

/// A `[x, y, z]` JSON array → `Vec3` (numbers or numeric strings).
fn arr3(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    let n = |x: &Value| match x {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    };
    Some(Vec3::new(n(&a[0])?, n(&a[1])?, n(&a[2])?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owl_idle_animations_are_detected() {
        let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/models/owl.bbmodel"));
        let idle = idle_anims(src);
        // The owl ships idle_* animations; the exact set is free to change, so just
        // confirm they're found with sane lengths (the parsing is pinned below).
        assert!(!idle.is_empty(), "owl idle animations should be detected");
        assert!(idle.iter().all(|m| m.length > 0.0), "idle animations have a length");
    }

    #[test]
    fn scans_idle_length_and_loop_mode_in_name_order() {
        let json = r#"{"animations":[
            {"name":"walk","loop":"loop","length":0.5},
            {"name":"idle_1","loop":"loop","length":2.0},
            {"name":"idle_0","loop":"once","length":1.0},
            {"name":"attack","length":1.0}
        ]}"#;
        let v = idle_anims(json);
        assert_eq!(v.len(), 2, "only idle_* animations, walk/attack ignored");
        // Name-sorted: idle_0 (once, 1.0) then idle_1 (loop, 2.0).
        assert!(!v[0].looping && (v[0].length - 1.0).abs() < 1e-6, "idle_0 once @1.0");
        assert!(v[1].looping && (v[1].length - 2.0).abs() < 1e-6, "idle_1 loop @2.0");
    }

    #[test]
    fn malformed_input_is_empty() {
        assert!(idle_anims("not json").is_empty());
        assert!(idle_anims("{}").is_empty());
    }

    #[test]
    fn owl_skeleton_has_a_root_and_valid_parents() {
        let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/models/owl.bbmodel"));
        let skel = skeleton(src);
        // Matches the renderer's bone count expectation (owl/head/wings/legs/...).
        assert!(skel.bones.len() >= 6, "owl bones detected");
        assert!(
            skel.bones.iter().any(|b| b.parent.is_none()),
            "the skeleton has a root bone"
        );
        for b in &skel.bones {
            if let Some(p) = b.parent {
                assert!(p < skel.bones.len(), "parent index in range");
            }
        }
    }

    #[test]
    fn skeleton_of_malformed_input_is_empty() {
        assert!(skeleton("not json").bones.is_empty());
        assert!(skeleton("{}").bones.is_empty());
    }
}
