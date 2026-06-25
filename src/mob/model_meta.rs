//! Sim-side metadata derived from a mob's compiled [`Model`](crate::bbmodel::Model): the
//! idle-animation info the AI needs and the bone hierarchy the death ragdoll tumbles.
//!
//! These are pure functions of the precached [`Model`] (see [`crate::mob::model`]) — the
//! same in-memory asset the renderer bakes from — so the simulation reads `.llmob`-derived
//! data and never re-parses a `.bbmodel`. Bone indices line up with the renderer's by
//! construction, since both come from the one `Model`.

use crate::bbmodel::Model;
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
        out.push(IdleAnimMeta { length: a.length, looping: a.looping });
        i += 1;
    }
    out
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
    /// minimum thickness so even a flat cube is a genuine 3D body. A geometry-less bone
    /// gets a small box around its `pivot`.
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
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

/// Build the sim skeleton from a compiled [`Model`]: each bone's pivot + parent come
/// straight from the model, and its bounding box is grown from the cubes the model assigned
/// to that bone (regularised to [`MIN_HALF`] thickness; a geometry-less bone gets a small
/// box at its pivot). Indices/pivots match the renderer exactly because both come from this
/// one `Model`.
pub fn skeleton(model: &Model) -> Skeleton {
    let n = model.bones.len();
    let mut lo = vec![Vec3::splat(f32::INFINITY); n];
    let mut hi = vec![Vec3::splat(f32::NEG_INFINITY); n];
    for c in &model.cubes {
        lo[c.bone] = lo[c.bone].min(c.from.min(c.to));
        hi[c.bone] = hi[c.bone].max(c.from.max(c.to));
    }
    let bones = model
        .bones
        .iter()
        .enumerate()
        .map(|(i, b)| {
            let (centre, half) = if hi[i].cmpge(lo[i]).all() {
                ((lo[i] + hi[i]) * 0.5, ((hi[i] - lo[i]) * 0.5).max(Vec3::splat(MIN_HALF)))
            } else {
                (b.pivot, Vec3::splat(MIN_HALF))
            };
            SkBone {
                pivot: b.pivot,
                bbox_min: centre - half,
                bbox_max: centre + half,
                parent: b.parent,
            }
        })
        .collect();
    Skeleton { bones }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owl() -> Model {
        let src = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/models/owl.bbmodel"));
        Model::load(src).expect("owl.bbmodel parses")
    }

    #[test]
    fn owl_idle_animations_are_detected_with_sane_lengths() {
        let v = idle_anims(&owl());
        assert!(!v.is_empty(), "owl idle animations should be detected");
        assert!(v.iter().all(|m| m.length > 0.0), "idle animations have a length");
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
        assert!(m.idle_animation(v.len()).is_none(), "the list covers exactly the model's idles");
    }

    #[test]
    fn owl_skeleton_matches_the_model_with_a_root_and_real_boxes() {
        let m = owl();
        let skel = skeleton(&m);
        assert_eq!(skel.bones.len(), m.bones.len(), "one sk-bone per model bone");
        assert!(skel.bones.iter().any(|b| b.parent.is_none()), "the skeleton has a root");
        for b in &skel.bones {
            if let Some(p) = b.parent {
                assert!(p < skel.bones.len(), "parent index in range");
            }
            // Regularised to MIN_HALF, so every bone is a genuine 3D body the ragdoll can tumble.
            assert!((b.bbox_max - b.bbox_min).min_element() > 0.0, "non-degenerate box");
        }
    }

    #[test]
    fn empty_model_yields_empty_metadata() {
        let m = Model::empty();
        assert!(idle_anims(&m).is_empty());
        assert!(skeleton(&m).bones.is_empty());
    }
}
