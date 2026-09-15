//! Clips: keyframes, tracks and markers, sampled by Blockbench's own
//! interpolation rules, plus the bone transform a sampled pose resolves through.

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use super::Bone;

/// How a keyframe travels toward the NEXT key: Blockbench's four
/// interpolation modes, sampled by Blockbench's own rules (`anim.rs`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Interpolation {
    #[default]
    Linear,
    CatmullRom,
    Bezier,
    Step,
}

/// A keyframe's per-axis Bezier handles: time offsets in seconds, value
/// offsets in the channel's units.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct BezierHandles {
    pub left_time: Vec3,
    pub left_value: Vec3,
    pub right_time: Vec3,
    pub right_value: Vec3,
}

impl BezierHandles {
    /// Blockbench's defaults, which also shape a Bezier segment through a key
    /// that is not itself `bezier` (a saved file drops its handles).
    pub const DEFAULT: Self = Self {
        left_time: Vec3::splat(-0.1),
        left_value: Vec3::ZERO,
        right_time: Vec3::splat(0.1),
        right_value: Vec3::ZERO,
    };
}

/// One keyframe: the channel's value at `time` seconds — euler degrees on a
/// rotation track, a model-unit offset on a position track. `pre` is the
/// value arriving from the previous key and `post` the value leaving toward
/// the next; they differ only on a key authored with two data points (a cut).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub pre: Vec3,
    pub post: Vec3,
    /// Authored with two data points. Blockbench's Catmull-Rom borrows the far
    /// neighbour only through a single-point key.
    pub split: bool,
    pub interpolation: Interpolation,
    pub bezier: Option<BezierHandles>,
}

impl Keyframe {
    /// A single-point key.
    pub fn new(time: f32, value: Vec3, interpolation: Interpolation) -> Self {
        Self {
            time,
            pre: value,
            post: value,
            split: false,
            interpolation,
            bezier: None,
        }
    }
}

/// A named instant in a clip, authored on Blockbench's Effects track.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Marker {
    pub time: f32,
    pub kind: MarkerKind,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkerKind {
    /// One line of a timeline script — the engine reads each line as a name.
    Timeline,
    /// A sound keyframe; the name is its effect.
    Sound,
    /// A particle keyframe; the name is its effect.
    Particle,
}

/// The bone channel a track drives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rotation,
    Position,
}

/// One bone's keys in a clip, sorted by time per channel; a channel with no
/// keys is unkeyed there.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Track {
    pub bone: usize,
    pub rotation: Vec<Keyframe>,
    pub position: Vec<Keyframe>,
}

impl Track {
    pub fn keys(&self, channel: Channel) -> &[Keyframe] {
        match channel {
            Channel::Rotation => &self.rotation,
            Channel::Position => &self.position,
        }
    }

    fn keys_mut(&mut self, channel: Channel) -> &mut Vec<Keyframe> {
        match channel {
            Channel::Rotation => &mut self.rotation,
            Channel::Position => &mut self.position,
        }
    }
}

/// A named animation: per-bone rotation and position tracks plus effect
/// markers. Rotation rotates about the bone's pivot; position translates the
/// bone (and its subtree) in its parent's frame. Scale channels are not read.
#[derive(Serialize, Deserialize, Clone)]
pub struct Animation {
    pub length: f32,
    /// Blockbench `loop: "loop"`.
    pub looping: bool,
    /// Blockbench `loop: "hold"` (bedrock `hold_on_last_frame`): a one-shot
    /// that keeps its final frame instead of ending. Ignored when `looping`.
    pub hold: bool,
    /// Sorted by bone; a bone keyed on neither channel has no entry.
    tracks: Vec<Track>,
    markers: Vec<Marker>,
}

impl Animation {
    pub fn new(length: f32, looping: bool, hold: bool) -> Self {
        Self {
            length,
            looping,
            hold,
            tracks: Vec::new(),
            markers: Vec::new(),
        }
    }

    /// Replace one bone channel's keys (sorted here; an empty list removes the
    /// track).
    pub fn set_track(&mut self, bone: usize, channel: Channel, mut keys: Vec<Keyframe>) {
        keys.sort_by(|a, b| a.time.total_cmp(&b.time));
        match self.tracks.binary_search_by_key(&bone, |t| t.bone) {
            Ok(i) => {
                *self.tracks[i].keys_mut(channel) = keys;
                let t = &self.tracks[i];
                if t.rotation.is_empty() && t.position.is_empty() {
                    self.tracks.remove(i);
                }
            }
            Err(i) if !keys.is_empty() => {
                let mut track = Track { bone, ..Track::default() };
                *track.keys_mut(channel) = keys;
                self.tracks.insert(i, track);
            }
            Err(_) => {}
        }
    }

    pub fn push_marker(&mut self, marker: Marker) {
        let at = self.markers.partition_point(|m| m.time <= marker.time);
        self.markers.insert(at, marker);
    }

    /// Markers in time order.
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// Every keyed bone's track, in bone order.
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }

    fn track(&self, bone: usize) -> Option<&Track> {
        let i = self.tracks.binary_search_by_key(&bone, |t| t.bone).ok()?;
        Some(&self.tracks[i])
    }

    /// Does this animation animate `bone` (have a rotation or position track for
    /// it)? The mob baker uses this to suppress AI head-look while an animation
    /// already drives the head bone.
    pub fn affects_bone(&self, bone: usize) -> bool {
        self.track(bone).is_some()
    }

    /// Bones with a track on `channel`.
    pub fn keyed_bones(&self, channel: Channel) -> impl Iterator<Item = usize> + '_ {
        self.tracks
            .iter()
            .filter(move |t| !t.keys(channel).is_empty())
            .map(|t| t.bone)
    }

    /// Where playback `time` seconds lands inside the clip: wrapped for a
    /// looping clip, clamped otherwise.
    pub fn clip_time(&self, time: f32) -> f32 {
        if self.length <= 0.0 {
            0.0
        } else if self.looping {
            time.rem_euclid(self.length)
        } else {
            time.clamp(0.0, self.length)
        }
    }

    /// One bone channel at clip-local `time`, or `None` when unkeyed.
    pub fn sample(&self, bone: usize, channel: Channel, time: f32) -> Option<Vec3> {
        let keys = self.track(bone)?.keys(channel);
        (!keys.is_empty()).then(|| sample_track(keys, time, self.looping))
    }

    /// Every keyed channel's value at clip-local `t`, bone by bone in one
    /// pass. `looping` decides whether Catmull-Rom neighbours wrap — the
    /// clip's own flag, or a player's override of it.
    pub fn samples_at(
        &self,
        t: f32,
        looping: bool,
    ) -> impl Iterator<Item = (usize, Channel, Vec3)> + '_ {
        self.tracks.iter().flat_map(move |track| {
            [Channel::Rotation, Channel::Position].into_iter().filter_map(move |channel| {
                let keys = track.keys(channel);
                (!keys.is_empty()).then(|| (track.bone, channel, sample_track(keys, t, looping)))
            })
        })
    }
}


/// Two times this close are the same instant — Blockbench's `1/1200` s.
const KEY_EPSILON: f32 = 1.0 / 1200.0;

/// Sample a sorted channel track at clip-local time `t`, by Blockbench's own
/// `interpolate()` rules so a clip plays exactly as its preview shows:
///
/// - `before` is the last key strictly before `t`, `after` the first at or
///   past it; a key within [`KEY_EPSILON`] of `t` answers its arriving value.
/// - A `step` key holds; outside the keyed range the nearest key holds.
/// - Linear when `before` is linear and `after` linear or step, else
///   Catmull-Rom when either is, else Bezier when either is.
/// - `before` contributes its leaving value (`post`), `after` its arriving
///   one (`pre`).
///
/// No Minecraft format enables Blockbench's loop wrapping, so a looping clip
/// does not interpolate across its end — only its Catmull-Rom neighbours wrap.
fn sample_track(kfs: &[Keyframe], t: f32, looping: bool) -> Vec3 {
    if kfs.is_empty() {
        return Vec3::ZERO;
    }
    let split = kfs.partition_point(|k| k.time < t);
    let before = split.checked_sub(1);
    let after = (split < kfs.len()).then_some(split);
    let near = |i: usize| (kfs[i].time - t).abs() <= KEY_EPSILON;
    match (before, after) {
        (Some(b), _) if near(b) => kfs[b].pre,
        (_, Some(a)) if near(a) => kfs[a].pre,
        (Some(b), _) if kfs[b].interpolation == Interpolation::Step => kfs[b].post,
        (Some(b), None) => kfs[b].post,
        (None, Some(a)) => kfs[a].pre,
        (Some(b), Some(a)) => between(kfs, b, a, t, looping),
        (None, None) => Vec3::ZERO,
    }
}

fn between(kfs: &[Keyframe], b: usize, a: usize, t: f32, looping: bool) -> Vec3 {
    use Interpolation::*;
    let (kb, ka) = (&kfs[b], &kfs[a]);
    let alpha = (t - kb.time) / (ka.time - kb.time);
    if kb.interpolation == Linear && matches!(ka.interpolation, Linear | Step) {
        kb.post + (ka.pre - kb.post) * alpha
    } else if kb.interpolation == CatmullRom || ka.interpolation == CatmullRom {
        catmull_rom(kfs, b, a, alpha, looping)
    } else if kb.interpolation == Bezier || ka.interpolation == Bezier {
        bezier(kb, ka, t)
    } else {
        kb.post + (ka.pre - kb.post) * alpha
    }
}

/// Blockbench's `getCatmullromLerp`: three.js's UNIFORM spline through up to
/// four values at the time fraction, a missing neighbour duplicating the end.
/// The parameter offset counts `before_plus` even when a split `before` kept
/// it off the curve — Blockbench does exactly that, so this does too.
fn catmull_rom(kfs: &[Keyframe], b: usize, a: usize, alpha: f32, looping: bool) -> Vec3 {
    let mut before_plus = b.checked_sub(1);
    let mut after_plus = (a + 1 < kfs.len()).then_some(a + 1);
    if looping && kfs.len() >= 3 {
        before_plus = before_plus.or(Some(kfs.len() - 2));
        after_plus = after_plus.or(Some(1));
    }
    let mut points = [Vec3::ZERO; 4];
    let mut n = 0;
    let mut push = |v: Vec3| {
        points[n] = v;
        n += 1;
    };
    if let Some(bp) = before_plus.filter(|_| !kfs[b].split) {
        push(kfs[bp].post);
    }
    push(kfs[b].post);
    push(kfs[a].pre);
    if let Some(ap) = after_plus.filter(|_| !kfs[a].split) {
        push(kfs[ap].pre);
    }
    let offset = if before_plus.is_some() { 1.0 } else { 0.0 };
    spline_point(&points[..n], (alpha + offset) / (n - 1) as f32)
}

/// three.js `SplineCurve.getPoint`, componentwise.
fn spline_point(points: &[Vec3], t: f32) -> Vec3 {
    let n = points.len() as isize;
    let last = points.len() - 1;
    let p = (n - 1) as f32 * t;
    let i = (p.floor() as isize).clamp(0, n - 1);
    let weight = p - i as f32;
    let at = |k: isize| points[k.clamp(0, last as isize) as usize];
    let p0 = at(if i == 0 { i } else { i - 1 });
    let p1 = at(i);
    let p2 = at(if i > n - 2 { n - 1 } else { i + 1 });
    let p3 = at(if i > n - 3 { n - 1 } else { i + 2 });
    let v0 = (p2 - p0) * 0.5;
    let v1 = (p3 - p1) * 0.5;
    let t2 = weight * weight;
    let t3 = weight * t2;
    (2.0 * p1 - 2.0 * p2 + v0 + v1) * t3 + (-3.0 * p1 + 3.0 * p2 - 2.0 * v0 - v1) * t2 + v0 * weight + p1
}

/// Blockbench's `getBezierLerp`, per axis: the cubic through each key's value
/// and handles, with the handles' time clamped inside the segment, solved for
/// x = `t`. (Blockbench approximates the solve over 200 samples; this solves
/// it.)
fn bezier(kb: &Keyframe, ka: &Keyframe, t: f32) -> Vec3 {
    let hb = kb.bezier.unwrap_or(BezierHandles::DEFAULT);
    let ha = ka.bezier.unwrap_or(BezierHandles::DEFAULT);
    let gap = ka.time - kb.time;
    Vec3::from_array(std::array::from_fn(|axis| {
        let x = [
            kb.time,
            kb.time + hb.right_time[axis].clamp(0.0, gap),
            ka.time + ha.left_time[axis].clamp(-gap, 0.0),
            ka.time,
        ];
        let y = [
            kb.post[axis],
            kb.post[axis] + hb.right_value[axis],
            ka.pre[axis] + ha.left_value[axis],
            ka.pre[axis],
        ];
        let cubic = |c: [f32; 4], s: f32| {
            let k = 1.0 - s;
            k * k * k * c[0] + 3.0 * k * k * s * c[1] + 3.0 * k * s * s * c[2] + s * s * s * c[3]
        };
        // The clamped handles keep x monotone in s, so bisection converges.
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..32 {
            let mid = 0.5 * (lo + hi);
            if cubic(x, mid) < t {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        cubic(y, 0.5 * (lo + hi))
    }))
}

/// Quaternion from an OUTLINER NODE's euler degrees — a `.bbmodel` cube tilt, a
/// bone's rest rotation, an animation rotation channel. X turns first, then Y,
/// then Z, which is Blockbench's `Format.euler_order` (`'ZYX'`, the default for
/// every format it ships). Single-axis rotations are order-free, so this only
/// shows on a cube or bone turned about two axes at once — and then it shows
/// hugely: the weapons workbench's shield lands 10.7 px out under the other
/// order, buried inside the board it hangs on.
///
/// NOT the order for a display transform or a pose offset — those are
/// [`display_euler_quat`].
pub fn euler_quat(deg: Vec3) -> Quat {
    Quat::from_rotation_z(deg.z.to_radians())
        * Quat::from_rotation_y(deg.y.to_radians())
        * Quat::from_rotation_x(deg.x.to_radians())
}

/// Quaternion from a DISPLAY TRANSFORM's euler degrees — a `.bbmodel` `display`
/// slot (the held/GUI pose) and the engine's pose-offset seams, which are
/// authored in the same vocabulary. Z turns first here, matching Minecraft's
/// own item transform; a display slot is not an outliner node and Blockbench
/// does not preview it under `Format.euler_order`.
pub fn display_euler_quat(deg: Vec3) -> Quat {
    Quat::from_euler(
        glam::EulerRot::XYZ,
        deg.x.to_radians(),
        deg.y.to_radians(),
        deg.z.to_radians(),
    )
}

/// A bone's local pose: the animated position offset translates the bone (and
/// its subtree) in the parent's frame, then the rest + animated rotation turns
/// it about its pivot — matching Blockbench's preview of both channels.
pub(super) fn bone_transform(bone: &Bone, anim_rot: Vec3, anim_pos: Vec3) -> Mat4 {
    Mat4::from_translation(bone.pivot + anim_pos)
        * Mat4::from_quat(euler_quat(bone.rotation + anim_rot))
        * Mat4::from_translation(-bone.pivot)
}

pub(super) fn head_look_transform(bone: &Bone, yaw: f32, pitch: f32) -> Mat4 {
    Mat4::from_translation(bone.pivot)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_x(pitch)
        * Mat4::from_quat(euler_quat(bone.rotation))
        * Mat4::from_translation(-bone.pivot)
}

#[cfg(test)]
mod tests;
