use petramond_math::math::Vec3;
use petramond_world::bbmodel::{euler_quat, Model};

pub struct NamedAnimMeta {
    pub name: String,
    pub length: f32,
    pub looping: bool,
}

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

#[derive(Copy, Clone, Debug)]
pub struct IdleAnimMeta {
    pub length: f32,
    pub looping: bool,
}

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

#[derive(Copy, Clone, Debug)]
pub struct SkBone {
    pub pivot: Vec3,
    pub bbox_min: Vec3,
    pub bbox_max: Vec3,
    pub parent: Option<usize>,
    pub welded: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    pub bones: Vec<SkBone>,
}

const MIN_HALF: f32 = 0.5;

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
    // Nearest ancestor with geometry, walking the provisional tree.
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

    fn sheep() -> Model {
        let src = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/sheep.bbmodel"
        ));
        Model::load(src).expect("sheep.bbmodel parses")
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
