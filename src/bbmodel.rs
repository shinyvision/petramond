//! Minimal Blockbench (`.bbmodel`) loader for animated entity models.
//!
//! Parses the subset of the bedrock-format `.bbmodel` we render: cube elements
//! (box + pivot + static rotation + per-face UVs), the bone hierarchy (groups +
//! the `outliner` tree), named bone-rotation animations, and the embedded texture.
//! This is GPU-agnostic data + pose math (no `wgpu`) — the engine's golden mob model,
//! compiled from the `.bbmodel` once (see [`crate::asset_cache`]) and then shared: the
//! renderer bakes geometry in [`crate::render::mob_model`] and uploads the texture in
//! [`crate::render::resources`], while the simulation derives its skeleton + idle metadata
//! in [`crate::mob::model_meta`]. At runtime nothing reads the `.bbmodel`; this `Model` (and
//! its `.llmob`) is authoritative.
//!
//! Coordinate notes:
//! - Cube coords are in the model's own units (this owl is built feet-at-`y=0`),
//!   scaled to metres by the caller.
//! - Face UVs are divided by the texture's `uv_width`/`uv_height` so they index the
//!   embedded sheet in `[0,1]`; the RAW corner order is kept (Blockbench encodes a
//!   per-face flip by reversing the rect), so flips reproduce on render.
//! - Bone/cube pivots are absolute model-space points; a bone's transform is
//!   `T(pivot)·R·T(-pivot)`, composed parent-before-child down the hierarchy —
//!   exactly Blockbench's nesting. Bone rotation from edit mode is the rest pose;
//!   animation rotations are applied on top of that rest rotation.
//!
//! Rotation order: euler angles are applied XYZ. Every rotation in the bundled owl
//! (static cube tilts and the walk keyframes) is single-axis, so the order is exact
//! here; a future model with multi-axis keyframes would need Blockbench's order.

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_cache::CompiledAsset;
use crate::mesh::face::Face;

mod anim;
mod parse;
#[cfg(test)]
mod tests;
mod texture;

pub(crate) use anim::euler_quat;

use anim::{bone_transform, head_look_transform, sample_track};
use parse::{arr3, num, parse_animations, parse_faces, walk_outliner};
use texture::TextureSheet;

/// One cube of the model: an axis-aligned box with a pivot + static rotation and a
/// per-face UV rect (in `Face::ALL` order; `None` = the face is omitted).
#[derive(Serialize, Deserialize)]
pub struct Cube {
    /// The authored Blockbench element name. Names carry gameplay meaning for some
    /// models — a sheep's fleece cubes are all named `wool` so the renderer can hide
    /// them while the sheep is shorn.
    pub name: String,
    pub from: Vec3,
    pub to: Vec3,
    /// Pivot for this cube's STATIC `rotation` (the modelled tilt).
    pub origin: Vec3,
    /// Static euler rotation in degrees, about `origin`.
    pub rotation: Vec3,
    /// Owning bone index (animation transforms compose from here up to the root).
    pub bone: usize,
    /// Normalized `[u0, v0_top, u1, v1_bottom]` per face, `Face::ALL` order
    /// (PosX, NegX, PosY, NegY, PosZ, NegZ). Raw corner order (flips preserved).
    pub faces: [Option<[f32; 4]>; 6],
}

/// A bone: a named pivot in the hierarchy. Animation rotates geometry about
/// `pivot`; `parent` chains the transform up to a root bone (`None`).
#[derive(Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    pub pivot: Vec3,
    /// Static Blockbench group rotation in degrees. This is the bone's rest pose.
    pub rotation: Vec3,
    pub parent: Option<usize>,
}

/// One keyframe: the channel's `Vec3` at `time` seconds — euler degrees on a
/// rotation track, a model-unit offset on a position track.
#[derive(Serialize, Deserialize)]
struct Keyframe {
    time: f32,
    v: Vec3,
}

/// A named animation: per-bone rotation and position tracks (sorted
/// keyframes), keyed by bone index. Rotation rotates about the bone's pivot;
/// position translates the bone (and its subtree) in its parent's frame —
/// both sampled with the same linear interpolation. Scale channels are not
/// read.
#[derive(Serialize, Deserialize)]
pub struct Animation {
    pub length: f32,
    /// Whether the animation loops. Blockbench `loop: "loop"` loops; `"once"` /
    /// `"hold"` play through once (the renderer holds the final frame rather than
    /// wrapping).
    pub looping: bool,
    tracks: HashMap<usize, Vec<Keyframe>>,
    #[serde(default)]
    pos_tracks: HashMap<usize, Vec<Keyframe>>,
}

impl Animation {
    /// Does this animation animate `bone` (have a rotation or position track for
    /// it)? The mob baker uses this to suppress AI head-look while an animation
    /// already drives the head bone.
    pub fn affects_bone(&self, bone: usize) -> bool {
        self.tracks.contains_key(&bone) || self.pos_tracks.contains_key(&bone)
    }
}

/// A parsed model: bones, cubes, animations, and the embedded RGBA texture.
#[derive(Serialize, Deserialize)]
pub struct Model {
    pub bones: Vec<Bone>,
    pub cubes: Vec<Cube>,
    pub animations: HashMap<String, Animation>,
    /// Names of `idle_*` animations, sorted, so a numeric idle index maps stably to
    /// one (the sim picks an index; [`idle_animation`](Self::idle_animation) resolves
    /// it here).
    idle_anim_names: Vec<String>,
    pub texture_rgba: Vec<u8>,
    pub tex_w: u32,
    pub tex_h: u32,
}

impl Model {
    /// An empty placeholder (no cubes, a 1×1 transparent texture) so a parse
    /// failure degrades to "no owls drawn" instead of crashing the renderer.
    pub fn empty() -> Self {
        Model {
            bones: Vec::new(),
            cubes: Vec::new(),
            animations: HashMap::new(),
            idle_anim_names: Vec::new(),
            texture_rgba: vec![0, 0, 0, 0],
            tex_w: 1,
            tex_h: 1,
        }
    }

    /// Look up an animation by name.
    pub fn animation(&self, name: &str) -> Option<&Animation> {
        self.animations.get(name)
    }

    /// The bone named `head`, if the model has one (gates AI head-look).
    pub fn head_bone(&self) -> Option<usize> {
        self.bones.iter().position(|b| b.name == "head")
    }

    /// The `index`-th `idle_*` animation (name-sorted), or `None` if out of range.
    pub fn idle_animation(&self, index: usize) -> Option<&Animation> {
        let name = self.idle_anim_names.get(index)?;
        self.animations.get(name)
    }

    /// Overwrite the `head` bone's posed transform with an AI head-look rotation
    /// (`yaw` about the model's up axis, `pitch` about its right axis) about the
    /// head's pivot, composed under the head's parent, and carry that same override
    /// through all descendant bones. Call AFTER posing, only when the active animation
    /// isn't itself driving the head.
    pub fn apply_head_look(&self, pose: &mut [Mat4], head_bone: usize, yaw: f32, pitch: f32) {
        let Some(bone) = self.bones.get(head_bone) else {
            return;
        };
        let Some(old_head) = pose.get(head_bone).copied() else {
            return;
        };
        let parent_world = bone
            .parent
            .and_then(|p| pose.get(p).copied())
            .unwrap_or(Mat4::IDENTITY);
        let new_head = parent_world * head_look_transform(bone, yaw, pitch);
        let delta = new_head * old_head.inverse();
        for i in 0..pose.len().min(self.bones.len()) {
            if i == head_bone || self.is_descendant_of(i, head_bone) {
                pose[i] = delta * pose[i];
            }
        }
    }

    /// The bone with the given authored name, if the model has one.
    pub fn bone_named(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

    /// COMPOSE `rot` onto `bone`'s posed transform, rotating about the bone's posed
    /// pivot, and carry the same delta through all descendant bones — the layered
    /// counterpart of [`apply_head_look`](Self::apply_head_look) (which REPLACES the
    /// pose). Use it to stack a gameplay override (an arm swing) on top of whatever
    /// animation is already posing the bone, so a punch composes with the walk cycle
    /// instead of freezing it.
    pub fn apply_bone_rotation(&self, pose: &mut [Mat4], bone: usize, rot: Quat) {
        let Some(b) = self.bones.get(bone) else {
            return;
        };
        let Some(posed) = pose.get(bone).copied() else {
            return;
        };
        let pivot = posed.transform_point3(b.pivot);
        let delta =
            Mat4::from_translation(pivot) * Mat4::from_quat(rot) * Mat4::from_translation(-pivot);
        for i in 0..pose.len().min(self.bones.len()) {
            if i == bone || self.is_descendant_of(i, bone) {
                pose[i] = delta * pose[i];
            }
        }
    }

    fn is_descendant_of(&self, mut child: usize, ancestor: usize) -> bool {
        while let Some(parent) = self.bones.get(child).and_then(|b| b.parent) {
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

    /// Parse a `.bbmodel` (JSON) string into a [`Model`].
    pub fn load(src: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(src).map_err(|e| format!("json: {e}"))?;

        // Textures: EVERY embedded texture decoded and stacked vertically into one
        // sheet, each face's UVs remapped through its own texture's band (a model may
        // paint different elements from different textures — the bed's wood vs its
        // sheets). A single-texture model reduces to the identity rect, so the sheet
        // IS that texture.
        let sheet = TextureSheet::decode(&root)?;

        // Bones from `groups`, indexed by uuid for the outliner walk + animators.
        let mut bones = Vec::new();
        let mut bone_by_uuid: HashMap<String, usize> = HashMap::new();
        if let Some(groups) = root.get("groups").and_then(Value::as_array) {
            for g in groups {
                let uuid = g
                    .get("uuid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let pivot = arr3(g.get("origin")).unwrap_or(Vec3::ZERO);
                let rotation = arr3(g.get("rotation")).unwrap_or(Vec3::ZERO);
                let name = g
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                bone_by_uuid.insert(uuid, bones.len());
                bones.push(Bone {
                    name,
                    pivot,
                    rotation,
                    parent: None,
                });
            }
        }

        // Cubes from `elements`, indexed by uuid; bone assigned by the outliner walk.
        let mut cubes = Vec::new();
        let mut cube_by_uuid: HashMap<String, usize> = HashMap::new();
        if let Some(elements) = root.get("elements").and_then(Value::as_array) {
            for e in elements {
                if e.get("type").and_then(Value::as_str).unwrap_or("cube") != "cube" {
                    continue;
                }
                let name = e
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let mut from = arr3(e.get("from")).unwrap_or(Vec3::ZERO);
                let mut to = arr3(e.get("to")).unwrap_or(Vec3::ZERO);
                let origin = arr3(e.get("origin")).unwrap_or(from);
                // Blockbench `inflate` grows every face outward by that amount
                // (UVs unchanged) — how skin overlay layers (hat/jacket/sleeves)
                // float slightly off the base cube instead of z-fighting it.
                let inflate = e.get("inflate").and_then(num).unwrap_or(0.0);
                if inflate != 0.0 {
                    from -= Vec3::splat(inflate);
                    to += Vec3::splat(inflate);
                }
                let rotation = arr3(e.get("rotation")).unwrap_or(Vec3::ZERO);
                let faces = parse_faces(e.get("faces"), &sheet);
                let uuid = e
                    .get("uuid")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                cube_by_uuid.insert(uuid, cubes.len());
                cubes.push(Cube {
                    name,
                    from,
                    to,
                    origin,
                    rotation,
                    bone: usize::MAX,
                    faces,
                });
            }
        }

        // Walk the `outliner` tree: set each bone's parent and each cube's owning
        // bone. Top-level nodes have no parent.
        if let Some(outliner) = root.get("outliner").and_then(Value::as_array) {
            for node in outliner {
                walk_outliner(
                    node,
                    None,
                    &bone_by_uuid,
                    &cube_by_uuid,
                    &mut bones,
                    &mut cubes,
                );
            }
        }

        // Any cube the outliner never placed (shouldn't happen for a well-formed
        // model) gets a synthetic identity root bone so it still renders.
        if cubes.iter().any(|c| c.bone == usize::MAX) {
            let fallback = bones.len();
            bones.push(Bone {
                name: "<root>".into(),
                pivot: Vec3::ZERO,
                rotation: Vec3::ZERO,
                parent: None,
            });
            for c in cubes.iter_mut().filter(|c| c.bone == usize::MAX) {
                c.bone = fallback;
            }
        }

        let animations = parse_animations(&root, &bone_by_uuid);
        // Stable, name-sorted index of the idle_* animations (matches the sim's count).
        let mut idle_anim_names: Vec<String> = animations
            .keys()
            .filter(|n| n.starts_with("idle_"))
            .cloned()
            .collect();
        idle_anim_names.sort();

        Ok(Model {
            bones,
            cubes,
            animations,
            idle_anim_names,
            texture_rgba: sheet.rgba,
            tex_w: sheet.w,
            tex_h: sheet.h,
        })
    }

    /// The rest pose: the authored Blockbench group rotations composed down the bone
    /// hierarchy. Cubes render at their modelled positions + static tilts, with any
    /// rotated groups (ears, tails, etc.) included.
    pub fn rest_pose(&self) -> Vec<Mat4> {
        let local: Vec<Mat4> = self
            .bones
            .iter()
            .map(|b| bone_transform(b, Vec3::ZERO, Vec3::ZERO))
            .collect();
        self.resolve_pose(&local)
    }

    /// Tight AABB over the rest-posed geometry, MODEL space (feet near y=0) —
    /// every cube's box through its bone + static-tilt transform, the same
    /// composition the render bake applies. Callers derive conservative cull
    /// volumes from this (scaled to the world + slack for animation), instead
    /// of guessing a species' extent from its collision size. An empty model
    /// answers a zero box at the origin.
    pub fn rest_bounds(&self) -> (Vec3, Vec3) {
        let pose = self.rest_pose();
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for cube in &self.cubes {
            let bone = pose.get(cube.bone).copied().unwrap_or(Mat4::IDENTITY);
            let s_cube = Mat4::from_translation(cube.origin)
                * Mat4::from_quat(euler_quat(cube.rotation))
                * Mat4::from_translation(-cube.origin);
            let m = bone * s_cube;
            for i in 0..8 {
                let corner = Vec3::new(
                    if i & 1 == 0 { cube.from.x } else { cube.to.x },
                    if i & 2 == 0 { cube.from.y } else { cube.to.y },
                    if i & 4 == 0 { cube.from.z } else { cube.to.z },
                );
                let p = m.transform_point3(corner);
                min = min.min(p);
                max = max.max(p);
            }
        }
        if !min.x.is_finite() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        (min, max)
    }

    /// Per-bone world-within-model transforms posed by `anim` at `time` seconds
    /// (looped over the animation length) — the single-animation form of
    /// [`pose_layers`](Self::pose_layers). Index by `Cube::bone`. Apply to a
    /// model-space cube vertex to get its posed model-space position (before the
    /// caller's scale/yaw/translate to the world).
    #[cfg(test)]
    pub fn pose(&self, anim: &Animation, time: f32) -> Vec<Mat4> {
        self.pose_layers(&[(anim, time, 1.0)])
    }

    /// Pose blended from several `(animation, time, weight)` layers at once: each
    /// bone rotates by the weight-scaled SUM of the layers' sampled eulers (the
    /// multi-layer generalization of a single weighted animation — summing
    /// weighted per-axis euler tracks is the same exactness argument). Weights
    /// clamp to `[0, 1]` individually; layers totalling 1 cross-fade (the player
    /// walk↔sneak blend), and an empty/zero-weight set is exactly the rest pose.
    pub fn pose_layers(&self, layers: &[(&Animation, f32, f32)]) -> Vec<Mat4> {
        // Each bone's LOCAL transform: rotate about its pivot by the authored rest
        // euler plus the summed, weighted animation eulers.
        let local: Vec<Mat4> = self
            .bones
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let mut rot = Vec3::ZERO;
                let mut pos = Vec3::ZERO;
                for (anim, time, weight) in layers {
                    let w = weight.clamp(0.0, 1.0);
                    if w <= 0.0 {
                        continue;
                    }
                    let t = if anim.length <= 0.0 {
                        0.0
                    } else if anim.looping {
                        time.rem_euclid(anim.length)
                    } else {
                        // Non-looping (Blockbench `once`/`hold`): play through once
                        // and hold the final frame instead of wrapping back.
                        time.clamp(0.0, anim.length)
                    };
                    if let Some(kfs) = anim.tracks.get(&i) {
                        rot += sample_track(kfs, t) * w;
                    }
                    if let Some(kfs) = anim.pos_tracks.get(&i) {
                        pos += sample_track(kfs, t) * w;
                    }
                }
                bone_transform(b, rot, pos)
            })
            .collect();

        self.resolve_pose(&local)
    }

    fn resolve_pose(&self, local: &[Mat4]) -> Vec<Mat4> {
        let mut world: Vec<Option<Mat4>> = vec![None; self.bones.len()];
        for i in 0..self.bones.len() {
            self.resolve_world(i, local, &mut world);
        }
        world
            .into_iter()
            .map(|m| m.unwrap_or(Mat4::IDENTITY))
            .collect()
    }

    fn resolve_world(&self, i: usize, local: &[Mat4], world: &mut [Option<Mat4>]) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let m = match self.bones[i].parent {
            Some(p) if p != i => self.resolve_world(p, local, world) * local[i],
            _ => local[i],
        };
        world[i] = Some(m);
        m
    }
}

impl CompiledAsset for Model {
    /// `LLMOB` — the compiled mob/entity model container (one file holds geometry, bones,
    /// animations and the decoded texture).
    const MAGIC: [u8; 8] = *b"LLMOB\0\0\0";
    /// v4: bones (including rest rotations) + cubes (per-face UV, element name) +
    /// named rotation animations + the RGBA texture — since v4 the texture is the
    /// combined multi-texture sheet with face UVs remapped into it. v5: element
    /// `inflate` baked into the cube box (skin overlay layers stop z-fighting).
    /// v6: animations carry `pos_tracks` (the 2026-07-20 position channels —
    /// this bump is LATE: v5-era caches mis-decoded under the grown layout, and
    /// an unlucky byte order decoded into valid-but-empty garbage instead of a
    /// clean failure — the invisible-hushjaw bug).
    /// Bump on any change to these fields or to [`Model::load`]'s output; the
    /// `compiled_model_layout_change_requires_a_format_version_bump` guard
    /// fails until you do.
    const FORMAT_VERSION: u32 = 6;
    const SUBDIR: &'static str = "models";
    const EXTENSION: &'static str = "llmob";

    /// Compile = parse the authored `.bbmodel` (UTF-8 JSON) via [`Model::load`].
    fn compile(source: &[u8]) -> Result<Self, String> {
        let src = std::str::from_utf8(source).map_err(|e| format!("bbmodel utf-8: {e}"))?;
        Model::load(src)
    }
}

/// The four corners of cube face `f` over box `[from, to]`, in `quad_box` order
/// (p0 bottom-left, p1 bottom-right, p2 top-right, p3 top-left).
pub(crate) fn face_corners(f: Face, from: Vec3, to: Vec3) -> [[f32; 3]; 4] {
    f.quad_box(from.to_array(), to.to_array())
}
