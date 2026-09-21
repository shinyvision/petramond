//! Local-space poses: per-bone rotation (euler degrees) and position deltas
//! from a rig's rest pose. That is the space Blockbench animates in, so adding
//! clips here is exactly what Blockbench's multi-animation preview shows, and
//! a pose resolves through the same bone transform a single clip does.

use glam::{Mat4, Vec3};

use crate::bbmodel::{Animation, Channel, Model};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalPose {
    rot: Vec<Vec3>,
    pos: Vec<Vec3>,
}

impl LocalPose {
    /// The rest pose of a rig with `bones` bones.
    pub fn rest(bones: usize) -> Self {
        Self {
            rot: vec![Vec3::ZERO; bones],
            pos: vec![Vec3::ZERO; bones],
        }
    }

    pub fn len(&self) -> usize {
        self.rot.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rot.is_empty()
    }

    /// Back to rest, keeping the allocation.
    pub fn clear(&mut self) {
        self.rot.fill(Vec3::ZERO);
        self.pos.fill(Vec3::ZERO);
    }

    pub fn copy_from(&mut self, other: &LocalPose) {
        self.rot.clone_from(&other.rot);
        self.pos.clone_from(&other.pos);
    }

    pub fn rotation(&self, bone: usize) -> Vec3 {
        self.rot.get(bone).copied().unwrap_or(Vec3::ZERO)
    }

    pub fn position(&self, bone: usize) -> Vec3 {
        self.pos.get(bone).copied().unwrap_or(Vec3::ZERO)
    }

    pub fn rotations(&self) -> &[Vec3] {
        &self.rot
    }

    pub fn positions(&self) -> &[Vec3] {
        &self.pos
    }

    /// Both channel arrays at once, rotations first.
    pub fn channels_mut(&mut self) -> (&mut [Vec3], &mut [Vec3]) {
        (&mut self.rot, &mut self.pos)
    }

    pub fn set_rotation(&mut self, bone: usize, value: Vec3) {
        if let Some(r) = self.rot.get_mut(bone) {
            *r = value;
        }
    }

    pub fn set_position(&mut self, bone: usize, value: Vec3) {
        if let Some(p) = self.pos.get_mut(bone) {
            *p = value;
        }
    }

    /// Add to one bone's channels (degrees, model units).
    pub fn add_bone(&mut self, bone: usize, rotation: Vec3, position: Vec3) {
        if let (Some(r), Some(p)) = (self.rot.get_mut(bone), self.pos.get_mut(bone)) {
            *r += rotation;
            *p += position;
        }
    }

    /// Add `weight` × `anim` at playback `time` (wrapped or clamped by the
    /// clip's own loop mode). Bones the clip does not key are untouched.
    pub fn add_clip(&mut self, anim: &Animation, time: f32, weight: f32) {
        self.add_clip_at(anim, anim.clip_time(time), weight);
    }

    /// Add `weight` × `anim` sampled at clip-local `t`, taken as given.
    pub fn add_clip_at(&mut self, anim: &Animation, t: f32, weight: f32) {
        if weight == 0.0 {
            return;
        }
        for (bone, channel, v) in anim.samples_at(t, anim.looping) {
            if let Some(slot) = self.channel_mut(channel, bone) {
                *slot += v * weight;
            }
        }
    }

    /// Overwrite every channel `anim` keys with its value at clip-local `t`;
    /// channels it does not key keep what they had.
    pub fn set_clip_at(&mut self, anim: &Animation, t: f32) {
        self.set_clip_at_looping(anim, t, anim.looping);
    }

    /// [`set_clip_at`](Self::set_clip_at) with the player's own loop mode, so
    /// a clip played as a loop wraps its Catmull-Rom neighbours whatever its
    /// authored flag says.
    pub fn set_clip_at_looping(&mut self, anim: &Animation, t: f32, looping: bool) {
        for (bone, channel, v) in anim.samples_at(t, looping) {
            if let Some(slot) = self.channel_mut(channel, bone) {
                *slot = v;
            }
        }
    }

    fn channel_mut(&mut self, channel: Channel, bone: usize) -> Option<&mut Vec3> {
        match channel {
            Channel::Rotation => self.rot.get_mut(bone),
            Channel::Position => self.pos.get_mut(bone),
        }
    }

    /// Add `weight` × `other`, bone for bone, scaled by the bone's `mask`
    /// entry when one is given (a missing entry is 0 — outside the mask).
    pub fn add_scaled(&mut self, other: &LocalPose, weight: f32, mask: Option<&[f32]>) {
        if weight == 0.0 {
            return;
        }
        let bones = self.rot.len().min(other.rot.len());
        for bone in 0..bones {
            let w = weight * mask_weight(mask, bone);
            if w != 0.0 {
                self.rot[bone] += other.rot[bone] * w;
                self.pos[bone] += other.pos[bone] * w;
            }
        }
    }

    /// Move each bone a fraction toward `other`: `weight` scaled by the bone's
    /// `mask` entry when one is given (a missing entry is 0 — outside the mask).
    pub fn blend_toward(&mut self, other: &LocalPose, weight: f32, mask: Option<&[f32]>) {
        if weight == 0.0 {
            return;
        }
        let bones = self.rot.len().min(other.rot.len());
        for bone in 0..bones {
            let w = weight * mask_weight(mask, bone);
            if w != 0.0 {
                let (r, p) = (self.rot[bone], self.pos[bone]);
                self.rot[bone] = r + (other.rot[bone] - r) * w;
                self.pos[bone] = p + (other.pos[bone] - p) * w;
            }
        }
    }

    /// This pose reflected across the rig's YZ plane into `out`: each bone
    /// takes its partner's channels with position X and rotation Y/Z negated
    /// (a reflection conjugates a ZYX euler to `(x, -y, -z)`). Exact for a rig
    /// whose partners are authored mirror images of each other.
    pub fn mirror_into(&self, map: &MirrorMap, out: &mut LocalPose) {
        out.rot.resize(self.rot.len(), Vec3::ZERO);
        out.pos.resize(self.pos.len(), Vec3::ZERO);
        for bone in 0..self.rot.len() {
            let from = map.partner(bone);
            let (r, p) = (self.rotation(from), self.position(from));
            out.rot[bone] = Vec3::new(r.x, -r.y, -r.z);
            out.pos[bone] = Vec3::new(-p.x, p.y, p.z);
        }
    }

    /// The bone world transforms this pose resolves to on `model`.
    pub fn resolve(&self, model: &Model) -> Vec<Mat4> {
        model.resolve_local(&self.rot, &self.pos)
    }

    /// [`resolve`](Self::resolve) into `out`, reusing its storage.
    pub fn resolve_into(&self, model: &Model, out: &mut Vec<Mat4>) {
        model.resolve_local_into(&self.rot, &self.pos, out);
    }
}

fn mask_weight(mask: Option<&[f32]>, bone: usize) -> f32 {
    mask.map_or(1.0, |m| m.get(bone).copied().unwrap_or(0.0))
}

/// Which bone mirrors which, by name: `left`/`right` swapped wherever the
/// word appears, in any capitalization (`left_shoulder` ↔ `right_shoulder`,
/// `leftArm` ↔ `rightArm`). A bone with no partner mirrors onto itself.
#[derive(Clone, Debug, PartialEq)]
pub struct MirrorMap {
    partner: Vec<usize>,
}

impl MirrorMap {
    pub fn for_model(model: &Model) -> Self {
        let names: Vec<&str> = model.bones().iter().map(|b| b.name.as_str()).collect();
        let partner = names
            .iter()
            .enumerate()
            .map(|(i, name)| {
                swap_side(name)
                    .and_then(|other| names.iter().position(|n| *n == other))
                    .unwrap_or(i)
            })
            .collect();
        Self { partner }
    }

    pub fn partner(&self, bone: usize) -> usize {
        self.partner.get(bone).copied().unwrap_or(bone)
    }
}

/// `name` with its first `left`/`right` swapped (matching the capitalization
/// of the word's first letter), or `None` when it has neither.
pub fn swap_side(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    let (at, from, to) = match (lower.find("left"), lower.find("right")) {
        (Some(l), Some(r)) if r < l => (r, "right", "left"),
        (Some(l), _) => (l, "left", "right"),
        (None, Some(r)) => (r, "right", "left"),
        (None, None) => return None,
    };
    let mut word = to.to_string();
    if name[at..].starts_with(|c: char| c.is_ascii_uppercase()) {
        word[..1].make_ascii_uppercase();
    }
    Some(format!(
        "{}{}{}",
        &name[..at],
        word,
        &name[at + from.len()..]
    ))
}

#[cfg(test)]
mod tests;
