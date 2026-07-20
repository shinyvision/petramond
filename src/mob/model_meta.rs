//! Sim-side metadata derived from a mob's compiled [`Model`](crate::bbmodel::Model): the
//! idle-animation info the AI needs and the bone hierarchy the death ragdoll tumbles.
//!
//! These are pure functions of the precached [`Model`] (see [`crate::mob::model`]) — the
//! same in-memory asset the renderer bakes from — so the simulation reads `.llmob`-derived
//! data and never re-parses a `.bbmodel`. Bone indices line up with the renderer's by
//! construction, since both come from the one `Model`.

use crate::bbmodel::{euler_quat, Model};
use crate::mathh::Vec3;

/// Length + loop mode of one NAMED animation, for the sim's one-shot layer
/// retirement: a mod-activated `once` clip (`MobAnimSet`) retires itself when
/// its phase passes the clip's length (see `mob::anim`). Name-sorted for a
/// binary-search lookup.
pub struct NamedAnimMeta {
    pub name: String,
    pub length: f32,
    pub looping: bool,
}

/// Every animation the model carries, name-sorted, with length + loop mode.
pub fn named_anims(model: &Model) -> Vec<NamedAnimMeta> {
    let mut anims: Vec<NamedAnimMeta> = model
        .animations
        .iter()
        .map(|(name, a)| NamedAnimMeta {
            name: name.to_owned(),
            length: a.length,
            looping: a.looping,
        })
        .collect();
    anims.sort_by(|a, b| a.name.cmp(&b.name));
    anims
}

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
    /// The bone gets no rigid body of its own in the ragdoll and moves rigidly with its
    /// parent: authored with a `_weld` name suffix (teeth, decorative shells that only
    /// exist as separate bones for animation), or a cube-less rig group (physics anchors
    /// on geometry; see [`skeleton`]). Never true for the root — there is nothing to
    /// weld to.
    pub welded: bool,
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
    let parents: Vec<Option<usize>> = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            b.parent
                .or_else(|| inferred_parent(model, &boxes, &pivots, root, i))
        })
        .collect();
    let has_geom: Vec<bool> = (0..n).map(|i| hi[i].cmpge(lo[i]).all()).collect();
    // Nearest geometry-bearing ancestor through the (connected) provisional tree.
    let geom_ancestor = |bone: usize| -> Option<usize> {
        let mut next = parents[bone];
        for _ in 0..n {
            let p = next?;
            if has_geom[p] {
                return Some(p);
            }
            next = parents[p];
        }
        None
    };
    // Physics anchors on GEOMETRY. A cube-less group is animation rig, not a body — as a
    // rigid body it is a tiny noise-driven placeholder box that the joint pass slaves
    // every real bone to (the hushjaw's empty `root` froze its corpse into a statue or
    // flipped it). So the physical root is the topmost geometry-bearing bone (preferring
    // an authored `body`, else the largest box, `_weld` names excluded), physical parents
    // skip across rig bones, and each rig bone is welded to the bone that adopted its
    // children so its render pose stays defined.
    let anchor_root = {
        let topmost: Vec<usize> = (0..n)
            .filter(|&i| {
                has_geom[i] && geom_ancestor(i).is_none() && !is_weld_name(&model.bones[i].name)
            })
            .collect();
        topmost
            .iter()
            .copied()
            .find(|&i| model.bones[i].name.eq_ignore_ascii_case("body"))
            .or_else(|| {
                topmost.into_iter().max_by(|&a, &b| {
                    box_volume(boxes[a])
                        .partial_cmp(&box_volume(boxes[b]))
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
            })
    };
    let bones = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (parent, welded) = match anchor_root {
                Some(ar) if i == ar => (None, false),
                Some(ar) => {
                    let parent = geom_ancestor(i).unwrap_or(ar);
                    (Some(parent), !has_geom[i] || is_weld_name(&b.name))
                }
                // No usable geometry anywhere (empty or degenerate model): keep the
                // provisional tree and the name convention only.
                None => (parents[i], parents[i].is_some() && is_weld_name(&b.name)),
            };
            let (bbox_min, bbox_max) = boxes[i];
            SkBone {
                pivot: pivots[i],
                bbox_min,
                bbox_max,
                parent,
                welded,
            }
        })
        .collect();
    Skeleton { bones }
}

/// The `_weld` bone-name suffix marks a bone as welded (see [`SkBone::welded`]). Names
/// carry meaning like the `head` bone (AI head-look) and the `body` root preference.
fn is_weld_name(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with("_weld")
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

    fn hushjaw() -> Model {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/mods-src/monsters/pack/models/hushjaw.bbmodel"
        ));
        Model::load(src).expect("hushjaw.bbmodel parses")
    }

    #[test]
    fn weld_suffixed_and_cube_less_bones_are_welded_to_a_parent() {
        // Two ways a bone opts out of ragdoll physics, both flagged `welded` (with a
        // parent to weld to): the authored `_weld` name suffix (the hushjaw's teeth) and
        // having no cubes at all (the hushjaw's rig-only `root` group — as a rigid body
        // it is a noise-driven placeholder box every real bone gets slaved to).
        let m = hushjaw();
        let skel = skeleton(&m);
        assert!(
            m.bones.iter().any(|b| is_weld_name(&b.name)),
            "fixture must exercise _weld bones"
        );
        let mut saw_cube_less = false;
        for (i, b) in skel.bones.iter().enumerate() {
            let has_cubes = m.cubes.iter().any(|c| c.bone == i);
            saw_cube_less |= !has_cubes;
            assert_eq!(
                b.welded,
                (is_weld_name(&m.bones[i].name) || !has_cubes) && b.parent.is_some(),
                "welding covers `_weld` names and cube-less rig bones: {}",
                m.bones[i].name
            );
        }
        assert!(saw_cube_less, "fixture must exercise a cube-less rig bone");
    }

    #[test]
    fn physics_roots_on_a_geometry_bearing_bone() {
        // The hushjaw's authored root has no cubes. The PHYSICAL root must be a real
        // body (a geometry-bearing, non-welded bone) and every non-welded bone must
        // reach it through non-welded geometry — never through a rig placeholder.
        let m = hushjaw();
        let skel = skeleton(&m);
        let roots: Vec<usize> = (0..skel.bones.len())
            .filter(|&i| skel.bones[i].parent.is_none())
            .collect();
        assert_eq!(roots.len(), 1, "one physical root");
        let root = roots[0];
        assert!(
            m.cubes.iter().any(|c| c.bone == root),
            "the physical root has geometry"
        );
        assert!(!skel.bones[root].welded, "the physical root is simulated");
        for (i, b) in skel.bones.iter().enumerate() {
            if b.welded || i == root {
                continue;
            }
            let p = b.parent.expect("non-root bones have parents");
            assert!(
                !skel.bones[p].welded,
                "simulated bone {i} joints to a simulated parent, not a rig placeholder"
            );
        }
    }

    #[test]
    fn empty_model_yields_empty_metadata() {
        let m = Model::empty();
        assert!(idle_anims(&m).is_empty());
        assert!(skeleton(&m).bones.is_empty());
    }
}
