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

/// One rotation keyframe: euler degrees at `time` seconds.
#[derive(Serialize, Deserialize)]
struct Keyframe {
    time: f32,
    rot: Vec3,
}

/// A named animation: per-bone rotation tracks (sorted keyframes), keyed by bone
/// index. Only the rotation channel is read (the bundled owl animates rotation only).
#[derive(Serialize, Deserialize)]
pub struct Animation {
    pub length: f32,
    /// Whether the animation loops. Blockbench `loop: "loop"` loops; `"once"` /
    /// `"hold"` play through once (the renderer holds the final frame rather than
    /// wrapping).
    pub looping: bool,
    tracks: HashMap<usize, Vec<Keyframe>>,
}

impl Animation {
    /// Does this animation animate `bone` (have a rotation track for it)? The mob
    /// baker uses this to suppress AI head-look while an animation already drives the
    /// head bone.
    pub fn affects_bone(&self, bone: usize) -> bool {
        self.tracks.contains_key(&bone)
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
            .map(|b| bone_transform(b, Vec3::ZERO))
            .collect();
        self.resolve_pose(&local)
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
                }
                bone_transform(b, rot)
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
    /// Bump on any change to these fields or to [`Model::load`]'s output.
    const FORMAT_VERSION: u32 = 5;
    const SUBDIR: &'static str = "models";
    const EXTENSION: &'static str = "llmob";

    /// Compile = parse the authored `.bbmodel` (UTF-8 JSON) via [`Model::load`].
    fn compile(source: &[u8]) -> Result<Self, String> {
        let src = std::str::from_utf8(source).map_err(|e| format!("bbmodel utf-8: {e}"))?;
        Model::load(src)
    }
}

/// Recursively assign bone parents + cube bones from one `outliner` node. A node
/// is either a cube-uuid string (a leaf) or a group object (`uuid` + `children`).
fn walk_outliner(
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
fn parse_faces(faces: Option<&Value>, sheet: &TextureSheet) -> [Option<[f32; 4]>; 6] {
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
fn parse_animations(
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

/// Linearly interpolate a sorted rotation track (euler degrees) at time `t`.
/// Clamps to the endpoints outside the keyed range.
fn sample_track(kfs: &[Keyframe], t: f32) -> Vec3 {
    if kfs.is_empty() {
        return Vec3::ZERO;
    }
    if t <= kfs[0].time {
        return kfs[0].rot;
    }
    let last = kfs.len() - 1;
    if t >= kfs[last].time {
        return kfs[last].rot;
    }
    for w in kfs.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if t >= a.time && t <= b.time {
            let span = b.time - a.time;
            let f = if span > 1e-6 {
                (t - a.time) / span
            } else {
                0.0
            };
            return a.rot + (b.rot - a.rot) * f;
        }
    }
    kfs[last].rot
}

/// Quaternion from euler degrees (XYZ order — exact for single-axis rotations).
/// Shared with [`crate::render::mob_model`] for the static per-cube tilt.
pub(crate) fn euler_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        glam::EulerRot::XYZ,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

fn bone_transform(bone: &Bone, anim_rot: Vec3) -> Mat4 {
    Mat4::from_translation(bone.pivot)
        * Mat4::from_quat(euler_quat(bone.rotation + anim_rot))
        * Mat4::from_translation(-bone.pivot)
}

fn head_look_transform(bone: &Bone, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(bone.pivot)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_quat(euler_quat(bone.rotation))
        * Mat4::from_translation(-bone.pivot)
}

/// The project `resolution` `(width, height)` — the UV divisor for a texture that
/// carries no `uv_width`/`uv_height` of its own. Falls back to 16.
fn project_resolution(root: &Value) -> (f32, f32) {
    if let Some(res) = root.get("resolution") {
        let w = res.get("width").and_then(Value::as_f64).unwrap_or(16.0);
        let h = res.get("height").and_then(Value::as_f64).unwrap_or(16.0);
        if w > 0.0 && h > 0.0 {
            return (w as f32, h as f32);
        }
    }
    (16.0, 16.0)
}

/// One texture's slot in the combined sheet: its own UV divisor plus the
/// normalized offset/scale of its band, so a face UV remaps in one step.
struct TexRect {
    uv_w: f32,
    uv_h: f32,
    u_off: f32,
    v_off: f32,
    u_scale: f32,
    v_scale: f32,
}

impl TexRect {
    /// A raw face UV coordinate (in this texture's own UV units) → the sheet.
    fn remap(&self, u: f32, v: f32) -> (f32, f32) {
        (
            self.u_off + u / self.uv_w * self.u_scale,
            self.v_off + v / self.uv_h * self.v_scale,
        )
    }
}

/// Every embedded texture decoded and stacked vertically into one RGBA sheet.
/// Faces reference textures by array index; `rects` is index-aligned with the
/// authored `textures` array (`None` = that entry had no decodable source).
struct TextureSheet {
    rgba: Vec<u8>,
    w: u32,
    h: u32,
    rects: Vec<Option<TexRect>>,
}

impl TextureSheet {
    /// The rect for a face's texture reference: its own entry when it decoded,
    /// else the first decoded texture (the old first-texture-only behavior).
    fn rect(&self, index: Option<usize>) -> Option<&TexRect> {
        index
            .and_then(|i| self.rects.get(i))
            .and_then(Option::as_ref)
            .or_else(|| self.rects.iter().flatten().next())
    }

    fn decode(root: &Value) -> Result<TextureSheet, String> {
        let (res_w, res_h) = project_resolution(root);
        let empty = Vec::new();
        let texs = root
            .get("textures")
            .and_then(Value::as_array)
            .unwrap_or(&empty);

        // Decode each entry's `data:image/png;base64,<payload>` source; an entry
        // without one (or that fails to decode) stays `None` so indices keep lining
        // up with face references.
        struct DecodedTex {
            rgba: Vec<u8>,
            w: u32,
            h: u32,
            /// UV-space size the face coordinates are authored in (entry override
            /// or the project resolution).
            uv_w: f32,
            uv_h: f32,
        }
        let images: Vec<Option<DecodedTex>> = texs
            .iter()
            .map(|t| {
                let src = t.get("source").and_then(Value::as_str)?;
                let payload = src.split_once(',').map(|(_, b)| b).unwrap_or(src);
                let bytes = base64_decode(payload)?;
                let img = image::load_from_memory(&bytes).ok()?.to_rgba8();
                let (w, h) = (img.width(), img.height());
                let uv_w = t.get("uv_width").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let uv_h = t.get("uv_height").and_then(Value::as_f64).unwrap_or(0.0) as f32;
                let (uv_w, uv_h) = if uv_w > 0.0 && uv_h > 0.0 {
                    (uv_w, uv_h)
                } else {
                    (res_w, res_h)
                };
                Some(DecodedTex {
                    rgba: img.into_raw(),
                    w,
                    h,
                    uv_w,
                    uv_h,
                })
            })
            .collect();

        let sheet_w = images.iter().flatten().map(|i| i.w).max().unwrap_or(0);
        let sheet_h: u32 = images.iter().flatten().map(|i| i.h).sum();
        if sheet_w == 0 || sheet_h == 0 {
            return Err("no embedded texture source".into());
        }

        let mut rgba = vec![0u8; (sheet_w * sheet_h * 4) as usize];
        let mut rects = Vec::with_capacity(images.len());
        let mut y_off = 0u32;
        for img in images {
            let Some(tex) = img else {
                rects.push(None);
                continue;
            };
            for row in 0..tex.h {
                let src = (row * tex.w * 4) as usize;
                let dst = (((y_off + row) * sheet_w) * 4) as usize;
                rgba[dst..dst + (tex.w * 4) as usize]
                    .copy_from_slice(&tex.rgba[src..src + (tex.w * 4) as usize]);
            }
            rects.push(Some(TexRect {
                uv_w: tex.uv_w,
                uv_h: tex.uv_h,
                u_off: 0.0,
                v_off: y_off as f32 / sheet_h as f32,
                u_scale: tex.w as f32 / sheet_w as f32,
                v_scale: tex.h as f32 / sheet_h as f32,
            }));
            y_off += tex.h;
        }

        Ok(TextureSheet {
            rgba,
            w: sheet_w,
            h: sheet_h,
            rects,
        })
    }
}

/// Minimal standard-alphabet base64 decoder (skips `=` padding + whitespace). Kept
/// in-tree so the loader needs no base64 dependency.
fn base64_decode(s: &str) -> Option<Vec<u8>> {
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
fn arr3(v: Option<&Value>) -> Option<Vec3> {
    let a = v?.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some(Vec3::new(num(&a[0])?, num(&a[1])?, num(&a[2])?))
}

/// A JSON value as `f32`, accepting both numbers and numeric strings (Blockbench
/// keyframe data points are stored as strings, e.g. `"20"`).
fn num(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => s.trim().parse::<f32>().ok(),
        _ => None,
    }
}

/// The four corners of cube face `f` over box `[from, to]`, in `quad_box` order
/// (p0 bottom-left, p1 bottom-right, p2 top-right, p3 top-left).
pub(crate) fn face_corners(f: Face, from: Vec3, to: Vec3) -> [[f32; 3]; 4] {
    f.quad_box(from.to_array(), to.to_array())
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

    /// The compiled `.llmob` payload round-trips the model with full fidelity: same
    /// geometry, same texture and — crucially — the same posed animation. Pins the
    /// serialization *contract* (every field survives), compared against the original, so
    /// it pins no editable table value.
    #[test]
    fn compiled_model_roundtrips_with_full_fidelity() {
        let m = owl();
        let bytes = bincode::serialize(&m).expect("model serializes");
        let m2: Model = bincode::deserialize(&bytes).expect("model deserializes");

        assert_eq!(m.cubes.len(), m2.cubes.len());
        assert_eq!(m.bones.len(), m2.bones.len());
        assert_eq!((m.tex_w, m.tex_h), (m2.tex_w, m2.tex_h));
        assert_eq!(m.texture_rgba, m2.texture_rgba, "texture bytes survive");
        let mut names1: Vec<&String> = m.animations.keys().collect();
        let mut names2: Vec<&String> = m2.animations.keys().collect();
        names1.sort();
        names2.sort();
        assert_eq!(names1, names2, "animation set survives");

        // Behaviour preserved: a pose from the round-tripped model matches the original
        // (proves bones, pivots, parents and keyframes all survived intact).
        let (walk1, walk2) = (m.animation("walk").unwrap(), m2.animation("walk").unwrap());
        for &t in &[0.0f32, 0.17, 0.33, 0.5] {
            for (a, b) in m.pose(walk1, t).iter().zip(m2.pose(walk2, t).iter()) {
                assert!(a.abs_diff_eq(*b, 1e-6), "posed transforms match at t={t}");
            }
        }
    }

    /// Base64-encode (standard alphabet, padded) — test-only counterpart of
    /// [`base64_decode`], for building synthetic embedded textures.
    fn base64_encode(bytes: &[u8]) -> String {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = u32::from_be_bytes([0, b[0], b[1], b[2]]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    /// A 1×1 PNG of one solid colour as a Blockbench `source` data URI.
    fn one_pixel_texture(rgba: [u8; 4]) -> String {
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("png encodes");
        format!("data:image/png;base64,{}", base64_encode(&png.into_inner()))
    }

    /// A model whose elements paint from DIFFERENT textures must keep every element
    /// visible: all textures land in one stacked sheet and each face's UVs remap into
    /// its own texture's band. (Regression: only the first texture was decoded, so
    /// every other texture's elements sampled transparent texels and vanished.)
    #[test]
    fn multi_texture_faces_remap_into_stacked_sheet() {
        let red = one_pixel_texture([255, 0, 0, 255]);
        let blue = one_pixel_texture([0, 0, 255, 255]);
        let src = format!(
            r#"{{
                "resolution": {{ "width": 16, "height": 16 }},
                "textures": [
                    {{ "uv_width": 16, "uv_height": 16, "source": "{red}" }},
                    {{ "uv_width": 16, "uv_height": 16, "source": "{blue}" }}
                ],
                "elements": [
                    {{ "uuid": "a", "type": "cube", "from": [0,0,0], "to": [1,1,1],
                       "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }},
                    {{ "uuid": "b", "type": "cube", "from": [2,0,0], "to": [3,1,1],
                       "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 1 }} }} }}
                ],
                "outliner": ["a", "b"]
            }}"#
        );
        let m = Model::load(&src).expect("two-texture model parses");

        // Both 1×1 textures stack into a 1×2 sheet, red band above blue.
        assert_eq!((m.tex_w, m.tex_h), (1, 2));
        assert_eq!(&m.texture_rgba[0..4], &[255, 0, 0, 255], "row 0 is red");
        assert_eq!(&m.texture_rgba[4..8], &[0, 0, 255, 255], "row 1 is blue");

        // Cube a's face spans the top (red) half, cube b's the bottom (blue) half.
        let uv_a = m.cubes[0].faces[2].expect("cube a up face");
        let uv_b = m.cubes[1].faces[2].expect("cube b up face");
        assert_eq!(uv_a, [0.0, 0.0, 1.0, 0.5]);
        assert_eq!(uv_b, [0.0, 0.5, 1.0, 1.0]);
    }

    /// Overlay layers (a skin's hat/jacket/sleeves) author the SAME box as their
    /// base cube plus an `inflate`; dropping it makes the two coincident and
    /// z-fight. The loader must bake inflate into the box (UVs untouched).
    #[test]
    fn element_inflate_grows_the_cube_box() {
        let tex = one_pixel_texture([255, 255, 255, 255]);
        let src = format!(
            r#"{{
                "resolution": {{ "width": 16, "height": 16 }},
                "textures": [{{ "uv_width": 16, "uv_height": 16, "source": "{tex}" }}],
                "elements": [
                    {{ "uuid": "base", "type": "cube", "from": [0,0,0], "to": [4,4,4],
                       "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }},
                    {{ "uuid": "layer", "type": "cube", "from": [0,0,0], "to": [4,4,4],
                       "inflate": 0.25,
                       "faces": {{ "up": {{ "uv": [0,0,16,16], "texture": 0 }} }} }}
                ],
                "outliner": ["base", "layer"]
            }}"#
        );
        let m = Model::load(&src).expect("inflated model parses");
        assert_eq!(m.cubes[0].from, Vec3::ZERO, "base box untouched");
        assert_eq!(m.cubes[0].to, Vec3::splat(4.0));
        assert_eq!(
            m.cubes[1].from,
            Vec3::splat(-0.25),
            "inflate grows every face outward"
        );
        assert_eq!(m.cubes[1].to, Vec3::splat(4.25));
        // UVs are NOT rescaled by inflate.
        assert_eq!(m.cubes[0].faces[2], m.cubes[1].faces[2]);
    }

    #[test]
    fn parses_cubes_bones_and_texture() {
        let m = owl();
        assert_eq!(
            m.cubes.len(),
            11,
            "head, beak, body, 2 wings, 2 legs, 2 feet, 2 tail"
        );
        assert!(m.bones.len() >= 6, "owl/head/lwing/rwing/lleg/rleg bones");
        // Embedded 32x32 texture decodes to RGBA.
        assert_eq!((m.tex_w, m.tex_h), (32, 32));
        assert_eq!(m.texture_rgba.len(), 32 * 32 * 4);
    }

    #[test]
    fn cube_names_are_parsed_and_survive_the_compiled_roundtrip() {
        // Cube names carry gameplay meaning (a sheep's fleece cubes are all named
        // `wool` so the renderer can hide them while shorn), so the loader must keep
        // them and the compiled `.llmob` must round-trip them.
        let m = sheep();
        let wool = m.cubes.iter().filter(|c| c.name == "wool").count();
        assert!(wool > 0, "the sheep fixture authors `wool` cubes");

        let bytes = bincode::serialize(&m).expect("model serializes");
        let m2: Model = bincode::deserialize(&bytes).expect("model deserializes");
        let names = |m: &Model| -> Vec<String> { m.cubes.iter().map(|c| c.name.clone()).collect() };
        assert_eq!(names(&m), names(&m2), "cube names survive the round-trip");
    }

    #[test]
    fn every_cube_has_a_resolved_bone() {
        let m = owl();
        for (i, c) in m.cubes.iter().enumerate() {
            assert!(c.bone < m.bones.len(), "cube {i} bone unresolved");
        }
    }

    #[test]
    fn walk_animation_is_present_and_loops_half_a_second() {
        let m = owl();
        let walk = m.animation("walk").expect("walk animation");
        assert!((walk.length - 0.5).abs() < 1e-6);
        // At least the two legs are animated.
        assert!(
            walk.tracks.len() >= 2,
            "legs (and head) have rotation tracks"
        );
    }

    #[test]
    fn pose_swings_the_legs_in_antiphase_over_the_cycle() {
        let m = owl();
        let walk = m.animation("walk").unwrap();
        // Identify the two leg bones by name.
        let leg_bones: Vec<usize> = m
            .bones
            .iter()
            .enumerate()
            .filter(|(_, b)| b.name == "lleg" || b.name == "rleg")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(leg_bones.len(), 2, "two leg bones");

        // A point at the foot, transformed by each leg's pose, should move forward
        // (±Z) and the two legs should be on opposite sides at t=0 (antiphase).
        let foot = glam::Vec4::new(0.3, 0.0, 0.75, 1.0);
        let pose0 = m.pose(walk, 0.0);
        let z: Vec<f32> = leg_bones.iter().map(|&b| (pose0[b] * foot).z).collect();
        assert!(
            (z[0] - z[1]).abs() > 0.05,
            "legs should be split fore/aft at t=0: {z:?}"
        );

        // Quarter cycle later the swing should have reversed (antiphase over time).
        let pose_q = m.pose(walk, 0.25);
        let zq: Vec<f32> = leg_bones.iter().map(|&b| (pose_q[b] * foot).z).collect();
        assert!(
            (z[0] - zq[0]).abs() > 0.05,
            "a leg should swing between t=0 and t=0.25: {z:?} vs {zq:?}"
        );
    }

    #[test]
    fn pose_loops_over_the_length() {
        let m = owl();
        let walk = m.animation("walk").unwrap();
        let a = m.pose(walk, 0.1);
        let b = m.pose(walk, 0.1 + walk.length);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!(x.abs_diff_eq(*y, 1e-4), "pose must loop");
        }
    }

    #[test]
    fn non_looping_animation_holds_its_final_frame() {
        let m = owl();
        // The owl's idle animations are Blockbench `once` (non-looping).
        let idle = m.idle_animation(0).expect("owl has idle animations");
        assert!(!idle.looping, "owl idle animations are one-shot");
        // Past the end it holds the final frame instead of wrapping to the start.
        let at_end = m.pose(idle, idle.length);
        let past_end = m.pose(idle, idle.length * 3.0);
        for (x, y) in at_end.iter().zip(past_end.iter()) {
            assert!(
                x.abs_diff_eq(*y, 1e-5),
                "one-shot pose holds the final frame, not loops"
            );
        }
    }

    #[test]
    fn exposes_head_bone_idle_anims_and_affects_bone() {
        let m = owl();
        assert!(m.head_bone().is_some(), "owl has a head bone");
        // `affects_bone`: the walk animation drives the leg bones (its whole purpose).
        let lleg = m
            .bones
            .iter()
            .position(|b| b.name == "lleg")
            .expect("lleg bone");
        assert!(
            m.animation("walk").unwrap().affects_bone(lleg),
            "walk animates the legs"
        );
        // The owl ships idle_* animations, exposed by a stable index.
        assert!(
            m.idle_animation(0).is_some(),
            "idle animations exposed by index"
        );
        assert!(
            m.idle_animation(999).is_none(),
            "out-of-range idle index is None"
        );
    }

    #[test]
    fn rest_pose_includes_static_group_rotations() {
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
        let pivot = m.bones[ear].pivot;
        let marker = pivot + Vec3::X;
        assert!(
            !rest[ear].transform_point3(marker).abs_diff_eq(marker, 1e-5),
            "rest pose applies the authored bone rotation"
        );
        assert!(
            rest[ear].transform_point3(pivot).abs_diff_eq(pivot, 1e-5),
            "bone rotation is about the authored pivot"
        );
    }

    #[test]
    fn head_look_propagates_to_child_bones() {
        let m = sheep();
        let head = m.head_bone().expect("sheep has a head bone");
        let ear = m
            .bones
            .iter()
            .position(|b| b.name == "ear_left")
            .expect("sheep has a child ear bone");
        assert!(
            m.is_descendant_of(ear, head),
            "the ear is authored under the head"
        );

        let mut pose = m.rest_pose();
        let ear_before = pose[ear];
        m.apply_head_look(&mut pose, head, 0.7, 0.2);

        assert!(
            !pose[head].abs_diff_eq(Mat4::IDENTITY, 1e-5),
            "head-look changes the head pose"
        );
        assert!(
            !pose[ear].abs_diff_eq(ear_before, 1e-5),
            "head-look carries through descendant bones"
        );
    }

    #[test]
    fn bone_rotation_composes_over_the_pose_and_propagates() {
        // Unlike head-look (which replaces), apply_bone_rotation must COMPOSE: the
        // rotated head keeps its animated/rest orientation plus the delta, the pivot
        // stays fixed, and descendants (the ear) carry the delta too.
        let m = sheep();
        let head = m.head_bone().expect("sheep has a head bone");
        let ear = m
            .bones
            .iter()
            .position(|b| b.name == "ear_left")
            .expect("sheep has a child ear bone");

        let mut pose = m.rest_pose();
        let head_before = pose[head];
        let ear_before = pose[ear];
        let pivot = m.bones[head].pivot;
        let pivot_world_before = head_before.transform_point3(pivot);
        m.apply_bone_rotation(&mut pose, head, Quat::from_rotation_x(0.6));

        assert!(
            !pose[head].abs_diff_eq(head_before, 1e-5),
            "the delta rotates the bone"
        );
        assert!(
            pose[head]
                .transform_point3(pivot)
                .abs_diff_eq(pivot_world_before, 1e-4),
            "the rotation is about the bone's posed pivot"
        );
        assert!(
            !pose[ear].abs_diff_eq(ear_before, 1e-5),
            "the delta carries through descendant bones"
        );

        // Composability: a zero rotation is a no-op (pure compose, no replace).
        let mut pose2 = m.rest_pose();
        m.apply_bone_rotation(&mut pose2, head, Quat::IDENTITY);
        for (a, b) in pose2.iter().zip(m.rest_pose().iter()) {
            assert!(a.abs_diff_eq(*b, 1e-6), "identity delta leaves the pose");
        }
    }

    #[test]
    fn empty_model_is_safe() {
        let m = Model::empty();
        assert!(m.cubes.is_empty());
        assert_eq!((m.tex_w, m.tex_h), (1, 1));
        assert_eq!(m.texture_rgba.len(), 4);
    }

    #[test]
    fn base64_roundtrips_known_vector() {
        // "Man" -> "TWFu" (classic base64 example).
        assert_eq!(base64_decode("TWFu").unwrap(), b"Man");
        // Padding + whitespace are ignored.
        assert_eq!(base64_decode("TWE=").unwrap(), b"Ma");
        assert_eq!(base64_decode("TW Fu\n").unwrap(), b"Man");
    }
}
