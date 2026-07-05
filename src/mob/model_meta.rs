//! Sim-side metadata derived from a mob's compiled [`Model`](crate::bbmodel::Model): the
//! idle-animation info the AI needs and the bone hierarchy the death ragdoll tumbles.
//!
//! These are pure functions of the precached [`Model`] (see [`crate::mob::model`]) — the
//! same in-memory asset the renderer bakes from — so the simulation reads `.llmob`-derived
//! data and never re-parses a `.bbmodel`. Bone indices line up with the renderer's by
//! construction, since both come from the one `Model`.

use crate::bbmodel::{euler_quat, Model};
use crate::mathh::Vec3;

/// What the AI needs to know about one `idle_*` animation.
#[derive(Copy, Clone, Debug)]
pub struct IdleAnimMeta {
    /// Length in seconds (so a one-shot idle is played for exactly its length).
    pub length: f32,
    /// Whether it loops (Blockbench `loop: "loop"`); `once`/`hold` do not.
    pub looping: bool,
}

/// This model's `idle_*` animations in its own stable (name-sorted) index order, each with
/// its length + loop mode. The index lines up 1:1 with [`Model::idle_animation`], so an
/// index the AI picks resolves to exactly the animation the renderer plays for it.
pub fn idle_anims(model: &Model) -> Vec<IdleAnimMeta> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(a) = model.idle_animation(i) {
        out.push(IdleAnimMeta {
            length: a.length,
            looping: a.looping,
        });
        i += 1;
    }
    out
}

/// One bone of the sim-side skeleton. The death ragdoll treats each bone as a rigid
/// *body* — the box `[bbox_min, bbox_max]` covering its geometry — whose corners are
/// simulated as particles. A rigid rotation is recovered from the corners each tick
/// (shape matching), so a bone tumbles and falls over physically; `pivot` is the joint
/// where it attaches to its physical ragdoll parent (see [`crate::mob::ragdoll`]).
#[derive(Copy, Clone, Debug)]
pub struct SkBone {
    /// The joint pivot in authored rest-pose model space.
    pub pivot: Vec3,
    /// Axis-aligned box covering the bone's cubes in authored rest-pose model space,
    /// regularised to a minimum thickness so even a flat cube is a genuine 3D body. A
    /// geometry-less bone gets a small box around its rest-pose `pivot`.
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    /// Physical ragdoll parent. This usually matches the authored model parent, but
    /// disconnected top-level authored bones are attached to the creature's main root
    /// so one mob dies as one connected body, not as independent loose parts.
    pub parent: Option<usize>,
}

/// The bone hierarchy of a mob, in the SAME index order as the renderer's
/// [`Model`](crate::bbmodel::Model) (it is derived from that very model), so a sim-computed
/// per-bone pose drops straight into the render bake.
#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub bones: Vec<SkBone>,
}

/// Smallest half-extent (model units) a bone's box is inflated to on each axis, so a
/// flat cube still has 3D thickness for the ragdoll to tumble realistically.
const MIN_HALF: f32 = 0.5;

/// Build the sim skeleton from a compiled [`Model`]: each bone's pivot and box are
/// derived in the same authored rest-pose model space the renderer uses. Authored
/// parent links are preserved, while multiple top-level roots are given inferred
/// physical parents for ragdoll constraints. Indices match the renderer exactly
/// because both come from this one `Model`.
pub fn skeleton(model: &Model) -> Skeleton {
    let n = model.bones.len();
    let mut lo = vec![Vec3::splat(f32::INFINITY); n];
    let mut hi = vec![Vec3::splat(f32::NEG_INFINITY); n];
    let rest = model.rest_pose();
    for c in &model.cubes {
        let Some(bone_rest) = rest.get(c.bone).copied() else {
            continue;
        };
        let cube_rest = bone_rest
            * glam::Mat4::from_translation(c.origin)
            * glam::Mat4::from_quat(euler_quat(c.rotation))
            * glam::Mat4::from_translation(-c.origin);
        for corner in box_corners(c.from, c.to) {
            let p = cube_rest.transform_point3(corner);
            lo[c.bone] = lo[c.bone].min(p);
            hi[c.bone] = hi[c.bone].max(p);
        }
    }
    let pivots: Vec<Vec3> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            rest.get(i)
                .copied()
                .unwrap_or(glam::Mat4::IDENTITY)
                .transform_point3(b.pivot)
        })
        .collect();
    let boxes: Vec<(Vec3, Vec3)> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, _b)| {
            let (centre, half) = if hi[i].cmpge(lo[i]).all() {
                (
                    (lo[i] + hi[i]) * 0.5,
                    ((hi[i] - lo[i]) * 0.5).max(Vec3::splat(MIN_HALF)),
                )
            } else {
                (pivots[i], Vec3::splat(MIN_HALF))
            };
            (centre - half, centre + half)
        })
        .collect();
    let root = primary_root(model, &boxes);
    let bones = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let parent = b
                .parent
                .or_else(|| inferred_parent(model, &boxes, &pivots, root, i));
            let (bbox_min, bbox_max) = boxes[i];
            SkBone {
                pivot: pivots[i],
                bbox_min,
                bbox_max,
                parent,
            }
        })
        .collect();
    Skeleton { bones }
}

fn primary_root(model: &Model, boxes: &[(Vec3, Vec3)]) -> Option<usize> {
    let roots: Vec<usize> = model
        .bones
        .iter()
        .enumerate()
        .filter_map(|(i, b)| b.parent.is_none().then_some(i))
        .collect();
    if roots.is_empty() {
        return None;
    }
    if let Some(body) = roots
        .iter()
        .copied()
        .find(|&i| model.bones[i].name.eq_ignore_ascii_case("body"))
    {
        return Some(body);
    }
    roots.into_iter().max_by(|&a, &b| {
        box_volume(boxes[a])
            .partial_cmp(&box_volume(boxes[b]))
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn inferred_parent(
    model: &Model,
    boxes: &[(Vec3, Vec3)],
    pivots: &[Vec3],
    root: Option<usize>,
    bone: usize,
) -> Option<usize> {
    let root = root?;
    if bone == root {
        return None;
    }
    let pivot = pivots[bone];
    let mut best: Option<(usize, f32)> = None;
    for (i, &bone_box) in boxes.iter().enumerate() {
        if i == bone || authored_descendant(model, i, bone) {
            continue;
        }
        if !box_contains(bone_box, pivot) {
            continue;
        }
        let centre = (bone_box.0 + bone_box.1) * 0.5;
        let score = (centre - pivot).length_squared();
        if best.is_none_or(|(_, best_score)| score < best_score) {
            best = Some((i, score));
        }
    }
    best.map(|(i, _)| i).or(Some(root))
}

fn authored_descendant(model: &Model, mut child: usize, ancestor: usize) -> bool {
    while let Some(parent) = model.bones.get(child).and_then(|b| b.parent) {
        if parent == ancestor {
            return true;
        }
        if parent == child {
            return false;
        }
        child = parent;
    }
    false
}

fn box_contains((min, max): (Vec3, Vec3), p: Vec3) -> bool {
    const EPS: f32 = 1e-3;
    p.x >= min.x - EPS
        && p.x <= max.x + EPS
        && p.y >= min.y - EPS
        && p.y <= max.y + EPS
        && p.z >= min.z - EPS
        && p.z <= max.z + EPS
}

fn box_volume((min, max): (Vec3, Vec3)) -> f32 {
    let span = (max - min).max(Vec3::ZERO);
    span.x * span.y * span.z
}

fn box_corners(from: Vec3, to: Vec3) -> [Vec3; 8] {
    let min = from.min(to);
    let max = from.max(to);
    [
        Vec3::new(min.x, min.y, min.z),
        Vec3::new(max.x, min.y, min.z),
        Vec3::new(min.x, max.y, min.z),
        Vec3::new(max.x, max.y, min.z),
        Vec3::new(min.x, min.y, max.z),
        Vec3::new(max.x, min.y, max.z),
        Vec3::new(min.x, max.y, max.z),
        Vec3::new(max.x, max.y, max.z),
    ]
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn owl_idle_animations_are_detected_with_sane_lengths() {
        let v = idle_anims(&owl());
        assert!(!v.is_empty(), "owl idle animations should be detected");
        assert!(
            v.iter().all(|m| m.length > 0.0),
            "idle animations have a length"
        );
    }

    #[test]
    fn idle_anims_line_up_with_the_models_idle_index() {
        // The sim's list is 1:1 with the model's idle index, so an index the AI picks
        // resolves to the same animation the renderer plays (no independent re-sorting).
        let m = owl();
        let v = idle_anims(&m);
        for (i, meta) in v.iter().enumerate() {
            let anim = m.idle_animation(i).expect("model has this idle index");
            assert_eq!(meta.length, anim.length);
            assert_eq!(meta.looping, anim.looping);
        }
        assert!(
            m.idle_animation(v.len()).is_none(),
            "the list covers exactly the model's idles"
        );
    }

    #[test]
    fn owl_skeleton_matches_the_model_with_a_root_and_real_boxes() {
        let m = owl();
        let skel = skeleton(&m);
        assert_eq!(
            skel.bones.len(),
            m.bones.len(),
            "one sk-bone per model bone"
        );
        assert!(
            skel.bones.iter().any(|b| b.parent.is_none()),
            "the skeleton has a root"
        );
        for b in &skel.bones {
            if let Some(p) = b.parent {
                assert!(p < skel.bones.len(), "parent index in range");
            }
            // Regularised to MIN_HALF, so every bone is a genuine 3D body the ragdoll can tumble.
            assert!(
                (b.bbox_max - b.bbox_min).min_element() > 0.0,
                "non-degenerate box"
            );
        }
    }

    #[test]
    fn disconnected_model_roots_get_physical_ragdoll_parents() {
        let m = sheep();
        let authored_roots = m.bones.iter().filter(|b| b.parent.is_none()).count();
        assert!(
            authored_roots > 1,
            "fixture must exercise disconnected authored roots"
        );

        let skel = skeleton(&m);
        let physical_roots = skel.bones.iter().filter(|b| b.parent.is_none()).count();
        assert_eq!(
            physical_roots, 1,
            "ragdoll skeleton should be physically connected"
        );
        assert_eq!(
            skel.bones.len(),
            m.bones.len(),
            "physical parenting keeps renderer bone indices intact"
        );
    }

    #[test]
    fn skeleton_boxes_include_rest_pose_and_cube_rotations() {
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
        let skel = skeleton(&m);
        let (min, max) = (skel.bones[ear].bbox_min, skel.bones[ear].bbox_max);
        for cube in m.cubes.iter().filter(|c| c.bone == ear) {
            let cube_rest = rest[ear]
                * glam::Mat4::from_translation(cube.origin)
                * glam::Mat4::from_quat(euler_quat(cube.rotation))
                * glam::Mat4::from_translation(-cube.origin);
            for corner in box_corners(cube.from, cube.to) {
                let p = cube_rest.transform_point3(corner);
                assert!(
                    box_contains((min, max), p),
                    "ragdoll box contains rendered rest-geometry corner {p:?}"
                );
            }
        }
    }

    #[test]
    fn empty_model_yields_empty_metadata() {
        let m = Model::empty();
        assert!(idle_anims(&m).is_empty());
        assert!(skeleton(&m).bones.is_empty());
    }
}
