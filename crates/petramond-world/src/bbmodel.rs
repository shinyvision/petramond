use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::asset_cache::CompiledAsset;
use petramond_math::face::Face;

mod anim;
pub mod bedrock;
pub mod clips;
mod parse;
mod pose;
mod self_ao;
#[cfg(test)]
mod tests;
mod texture;

pub use anim::{
    display_euler_quat, euler_quat, Animation, BezierHandles, Channel, Interpolation, Keyframe,
    Marker, MarkerKind, Track,
};

use anim::{bone_transform, head_look_transform};
use parse::{arr3, num, parse_animations, parse_faces, walk_outliner};
use texture::TextureSheet;

#[derive(Serialize, Deserialize)]
pub struct Cube {
    pub name: String,
    pub from: Vec3,
    pub to: Vec3,
    pub origin: Vec3,
    pub rotation: Vec3,
    pub bone: usize,
    pub faces: [Option<FaceUv>; 6],
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct FaceUv {
    pub uv: [f32; 4],
    pub rot: u8,
}

impl FaceUv {
    pub const fn new(uv: [f32; 4]) -> Self {
        Self { uv, rot: 0 }
    }

    pub const fn with_uv(self, uv: [f32; 4]) -> Self {
        Self { uv, ..self }
    }

    pub fn corner_uv(self) -> [[f32; 2]; 4] {
        let [u0, v0, u1, v1] = self.uv;
        let c = [[u0, v1], [u1, v1], [u1, v0], [u0, v0]];
        let r = (self.rot % 4) as usize;
        std::array::from_fn(|i| c[(i + r) % 4])
    }
}

#[derive(Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    pub pivot: Vec3,
    pub rotation: Vec3,
    pub parent: Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub struct Model {
    pub bones: Vec<Bone>,
    pub cubes: Vec<Cube>,
    pub animations: HashMap<String, Animation>,
    idle_anim_names: Vec<String>,
    pub texture_rgba: Vec<u8>,
    pub tex_w: u32,
    pub tex_h: u32,
}

impl Model {
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

    pub fn animation(&self, name: &str) -> Option<&Animation> {
        self.animations.get(name)
    }

    pub fn head_bone(&self) -> Option<usize> {
        self.bones.iter().position(|b| b.name == "head")
    }

    pub fn idle_animation(&self, index: usize) -> Option<&Animation> {
        let name = self.idle_anim_names.get(index)?;
        self.animations.get(name)
    }

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

    pub fn bone_named(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name == name)
    }

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

    pub fn apply_bone_offset(&self, pose: &mut [Mat4], bone: usize, rot: Quat, translation: Vec3) {
        let Some(b) = self.bones.get(bone) else {
            return;
        };
        let Some(posed) = pose.get(bone).copied() else {
            return;
        };
        let pivot = posed.transform_point3(b.pivot);
        let delta = Mat4::from_translation(pivot + translation)
            * Mat4::from_quat(rot)
            * Mat4::from_translation(-pivot);
        for i in 0..pose.len().min(self.bones.len()) {
            if i == bone || self.is_descendant_of(i, bone) {
                pose[i] = delta * pose[i];
            }
        }
    }

    pub fn hold_bone(&self, pose: &mut [Mat4], bone: usize, rot: Vec3, translation: Vec3) {
        let Some(b) = self.bones.get(bone) else {
            return;
        };
        let Some(old) = pose.get(bone).copied() else {
            return;
        };
        let parent = b
            .parent
            .and_then(|p| pose.get(p).copied())
            .unwrap_or(Mat4::IDENTITY);
        let delta = (parent * bone_transform(b, rot, translation)) * old.inverse();
        for i in 0..pose.len().min(self.bones.len()) {
            if i == bone || self.is_descendant_of(i, bone) {
                pose[i] = delta * pose[i];
            }
        }
    }

    pub fn bones(&self) -> &[Bone] {
        &self.bones
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

    pub fn load(src: &str) -> Result<Self, String> {
        let root: Value = serde_json::from_str(src).map_err(|e| format!("json: {e}"))?;

        let sheet = TextureSheet::decode(&root)?;

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
        let mut idle_anim_names: Vec<String> = animations
            .keys()
            .filter(|n| clips::is_idle(n))
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

    pub fn rest_pose(&self) -> Vec<Mat4> {
        let local: Vec<Mat4> = self
            .bones
            .iter()
            .map(|b| bone_transform(b, Vec3::ZERO, Vec3::ZERO))
            .collect();
        self.resolve_pose(&local)
    }

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

    #[cfg(any(test, feature = "test-support"))]
    pub fn pose(&self, anim: &Animation, time: f32) -> Vec<Mat4> {
        self.pose_layers(&[(anim, time, 1.0)])
    }

    pub fn pose_layers(&self, layers: &[(&Animation, f32, f32)]) -> Vec<Mat4> {
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
                    let t = anim.clip_time(*time);
                    if let Some(r) = anim.sample(i, Channel::Rotation, t) {
                        rot = if anim.overrides {
                            rot.lerp(r, w)
                        } else {
                            rot + r * w
                        };
                    }
                    if let Some(p) = anim.sample(i, Channel::Position, t) {
                        pos = if anim.overrides {
                            pos.lerp(p, w)
                        } else {
                            pos + p * w
                        };
                    }
                }
                bone_transform(b, rot, pos)
            })
            .collect();

        self.resolve_pose(&local)
    }

    pub fn resolve_local(&self, rotations: &[Vec3], positions: &[Vec3]) -> Vec<Mat4> {
        let mut out = Vec::with_capacity(self.bones.len());
        self.resolve_local_into(rotations, positions, &mut out);
        out
    }

    pub fn resolve_local_into(&self, rotations: &[Vec3], positions: &[Vec3], out: &mut Vec<Mat4>) {
        out.clear();
        out.resize(self.bones.len(), Mat4::NAN);
        for i in 0..self.bones.len() {
            self.resolve_local_bone(i, rotations, positions, out);
        }
    }

    fn resolve_local_bone(
        &self,
        i: usize,
        rotations: &[Vec3],
        positions: &[Vec3],
        out: &mut [Mat4],
    ) -> Mat4 {
        if !out[i].x_axis.x.is_nan() {
            return out[i];
        }
        let bone = &self.bones[i];
        let local = bone_transform(
            bone,
            rotations.get(i).copied().unwrap_or(Vec3::ZERO),
            positions.get(i).copied().unwrap_or(Vec3::ZERO),
        );
        let m = match bone.parent {
            Some(p) if p != i => self.resolve_local_bone(p, rotations, positions, out) * local,
            _ => local,
        };
        out[i] = m;
        m
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
    const MAGIC: [u8; 8] = *b"LLMOB\0\0\0";
    const FORMAT_VERSION: u32 = 11;
    const SUBDIR: &'static str = "models";
    const EXTENSION: &'static str = "llmob";

    fn compile(source: &[u8]) -> Result<Self, String> {
        let src = std::str::from_utf8(source).map_err(|e| format!("bbmodel utf-8: {e}"))?;
        Model::load(src)
    }
}

pub fn face_corners(f: Face, from: Vec3, to: Vec3) -> [[f32; 3]; 4] {
    f.quad_box(from.to_array(), to.to_array())
}
