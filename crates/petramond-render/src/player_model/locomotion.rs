//! The locomotion LAYER TABLE: which authored clips the player body
//! crossfades, at what phase, weighted by which blend inputs — data in
//! `assets/animations/player_locomotion.json`, layered like every asset so a
//! pack overrides a row by `id` or appends its own.
//!
//! The body pose driver (`game/body_pose.rs`) publishes a handful of named
//! 0..1 INPUTS (`Inputs`). The table turns them into per-clip weights: a
//! layer's weight is the PRODUCT of its factors, where a factor is an input
//! (`"run"`), its complement (`"!run"`), or a partial complement
//! (`{"not": "landing", "by": 0.65}` = `1 - 0.65·landing`). `derived` rows
//! name reusable products; `requires` zeroes an input or derived name when
//! the model lacks the clips it feeds, so a rig without a `run` clip folds
//! that weight back into `walk` instead of dropping it. Every weight is
//! evaluated in one pass; adding a locomotion style is a row edit.

use std::sync::LazyLock;

use petramond_world::bbmodel::{Animation, Model};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use smallvec::SmallVec;

use crate::PlayerRenderInstance;

/// Layers lighter than this are dropped: they would move nothing visible and
/// only cost a pose blend.
const MIN_LAYER_WEIGHT: f32 = 0.001;

/// The blend inputs the pose driver publishes, all in `0..=1`. Names are the
/// table's vocabulary (`Input::NAMES`), in this order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub walking: f32,
    pub sneak: f32,
    pub run: f32,
    pub backward: f32,
    /// Unsigned lateral balance.
    pub strafe: f32,
    /// 1 when the lateral balance leans right, else 0 — a selector between a
    /// left and a right clip.
    pub right: f32,
    pub airborne: f32,
    pub falling: f32,
    pub landing: f32,
    /// How far the body is swimming (`0` dry … `1` fully in a fluid).
    pub swim: f32,
    /// Swimming with the feet on the floor (wading).
    pub floor: f32,
    pub swim_moving: f32,
    pub swim_backward: f32,
    pub swim_rising: f32,
}

impl Inputs {
    const NAMES: [&'static str; 14] = [
        "walking",
        "sneak",
        "run",
        "backward",
        "strafe",
        "right",
        "airborne",
        "falling",
        "landing",
        "swim",
        "floor",
        "swim_moving",
        "swim_backward",
        "swim_rising",
    ];

    fn from_instance(inst: &PlayerRenderInstance) -> Self {
        let mix = inst.locomotion;
        let swim = mix.swim;
        let unit = |v: f32| v.clamp(0.0, 1.0);
        Self {
            walking: unit(inst.walk_weight),
            sneak: unit(inst.sneak_weight),
            run: unit(mix.run),
            backward: unit(mix.backward),
            strafe: mix.strafe.abs().min(1.0),
            right: if mix.strafe >= 0.0 { 1.0 } else { 0.0 },
            airborne: unit(mix.airborne),
            falling: unit(mix.falling),
            landing: unit(mix.landing),
            swim: unit(swim.weight),
            floor: unit(swim.grounded),
            swim_moving: unit(swim.moving),
            swim_backward: unit(swim.backward),
            swim_rising: unit(swim.rising),
        }
    }

    fn get(&self, index: usize) -> f32 {
        [
            self.walking,
            self.sneak,
            self.run,
            self.backward,
            self.strafe,
            self.right,
            self.airborne,
            self.falling,
            self.landing,
            self.swim,
            self.floor,
            self.swim_moving,
            self.swim_backward,
            self.swim_rising,
        ][index]
    }

    /// The phase each layer samples its clip at, by `phase` source.
    fn phase(inst: &PlayerRenderInstance, source: Phase) -> f32 {
        match source {
            Phase::Stride => inst.anim_time,
            Phase::Swim => inst.locomotion.swim.phase,
            Phase::Rest => 0.0,
        }
    }
}

/// Which clock a layer's clip plays on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// The normalized stride phase (ground speed–driven).
    Stride,
    /// The swim stroke clock.
    Swim,
    /// Frame 0 — a held pose the weight alone shapes.
    Rest,
}

/// One factor as written: `"name"`, `"!name"`, or `{"not": name, "by": k}`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(untagged)]
enum RawFactor {
    Name(String),
    Partial { not: String, by: f32 },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDerived {
    name: String,
    factors: Vec<RawFactor>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLayer {
    id: String,
    clip: String,
    phase: Phase,
    factors: Vec<RawFactor>,
    /// A pack disables an engine row by overriding it with `"enabled": false`.
    #[serde(default = "enabled")]
    enabled: bool,
}

fn enabled() -> bool {
    true
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    #[serde(default)]
    requires: FxHashMap<String, Vec<String>>,
    #[serde(default)]
    derived: Vec<RawDerived>,
    #[serde(default)]
    layers: Vec<RawLayer>,
}

/// A resolved factor over the value vector (inputs first, then derived).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Factor {
    slot: usize,
    /// `value` when 0, else `1 - by · value`.
    complement_by: f32,
}

impl Factor {
    #[inline]
    fn apply(self, values: &[f32]) -> f32 {
        let v = values[self.slot];
        if self.complement_by == 0.0 {
            v
        } else {
            1.0 - self.complement_by * v
        }
    }
}

struct Derived {
    factors: Box<[Factor]>,
}

struct Layer {
    clip: String,
    phase: Phase,
    factors: Box<[Factor]>,
}

/// The compiled table: value slots are the 14 inputs followed by the derived
/// names in definition order; `requires` gates by slot.
pub struct LocomotionTable {
    derived: Box<[Derived]>,
    /// Per value slot, the clips that must exist for the slot to read non-zero.
    requires: Box<[Box<[String]>]>,
    layers: Box<[Layer]>,
}

impl LocomotionTable {
    /// Number of value slots (inputs + derived).
    fn slots(&self) -> usize {
        Inputs::NAMES.len() + self.derived.len()
    }

    /// Evaluate every layer for `inputs` against a model that `has_clip`.
    /// Yields `(clip, phase source, weight)` for layers above the weight floor
    /// whose clip exists, in table order.
    pub fn weights<'a>(
        &'a self,
        inputs: &Inputs,
        has_clip: impl Fn(&str) -> bool + 'a,
    ) -> impl Iterator<Item = (&'a str, Phase, f32)> + 'a {
        // Inputs first, gated by their `requires`; then each derived product
        // in definition order, so a gate on an input flows through every
        // product that reads it, and a gated derived slot reads 0 itself.
        let mut values: SmallVec<[f32; 32]> = SmallVec::with_capacity(self.slots());
        let open = |slot: usize| self.requires[slot].iter().all(|c| has_clip(c));
        for i in 0..Inputs::NAMES.len() {
            values.push(if open(i) { inputs.get(i) } else { 0.0 });
        }
        for (i, derived) in self.derived.iter().enumerate() {
            let slot = Inputs::NAMES.len() + i;
            let v = if open(slot) {
                derived.factors.iter().map(|f| f.apply(&values)).product()
            } else {
                0.0
            };
            values.push(v);
        }
        self.layers.iter().filter_map(move |layer| {
            let weight: f32 = layer.factors.iter().map(|f| f.apply(&values)).product();
            (weight > MIN_LAYER_WEIGHT && has_clip(&layer.clip)).then_some((
                layer.clip.as_str(),
                layer.phase,
                weight,
            ))
        })
    }

    /// Parse and merge the asset layers (base first). Later layers replace a
    /// `derived` row or a layer row by name/id and append new ones; `requires`
    /// merges by key. Every factor name must be an input or a derived name
    /// defined EARLIER, so evaluation is one forward pass.
    pub fn parse_layers(texts: &[&str]) -> Result<Self, String> {
        let mut requires: FxHashMap<String, Vec<String>> = FxHashMap::default();
        let mut derived: Vec<RawDerived> = Vec::new();
        let mut layers: Vec<RawLayer> = Vec::new();
        for (i, text) in texts.iter().enumerate() {
            let file: RawFile =
                serde_json::from_str(text).map_err(|e| format!("layer #{i}: invalid JSON: {e}"))?;
            requires.extend(file.requires);
            for row in file.derived {
                match derived.iter_mut().find(|d| d.name == row.name) {
                    Some(existing) => *existing = row,
                    None => derived.push(row),
                }
            }
            for row in file.layers {
                match layers.iter_mut().find(|l| l.id == row.id) {
                    Some(existing) => *existing = row,
                    None => layers.push(row),
                }
            }
        }
        let mut names: Vec<&str> = Inputs::NAMES.to_vec();
        for row in &derived {
            if names.contains(&row.name.as_str()) {
                return Err(format!("derived '{}' shadows an existing name", row.name));
            }
            names.push(&row.name);
        }
        let resolve = |factors: &[RawFactor], visible: usize, what: &str| {
            factors
                .iter()
                .map(|f| {
                    let (name, by) = match f {
                        RawFactor::Name(n) => match n.strip_prefix('!') {
                            Some(rest) => (rest, 1.0),
                            None => (n.as_str(), 0.0),
                        },
                        RawFactor::Partial { not, by } => {
                            if !by.is_finite() || !(0.0..=1.0).contains(by) {
                                return Err(format!("{what}: 'by' must be in 0..=1"));
                            }
                            (not.as_str(), *by)
                        }
                    };
                    match names[..visible].iter().position(|n| *n == name) {
                        Some(slot) => Ok(Factor {
                            slot,
                            complement_by: by,
                        }),
                        None => Err(format!(
                            "{what}: '{name}' is not an input or an earlier derived name"
                        )),
                    }
                })
                .collect::<Result<Box<[Factor]>, String>>()
        };
        let derived = derived
            .iter()
            .enumerate()
            .map(|(i, row)| {
                Ok(Derived {
                    factors: resolve(
                        &row.factors,
                        Inputs::NAMES.len() + i,
                        &format!("derived '{}'", row.name),
                    )?,
                })
            })
            .collect::<Result<Box<[Derived]>, String>>()?;
        let layers = layers
            .iter()
            .filter(|row| row.enabled)
            .map(|row| {
                Ok(Layer {
                    clip: row.clip.clone(),
                    phase: row.phase,
                    factors: resolve(&row.factors, names.len(), &format!("layer '{}'", row.id))?,
                })
            })
            .collect::<Result<Box<[Layer]>, String>>()?;
        let mut requires_by_slot: Vec<Box<[String]>> = vec![Box::new([]); names.len()];
        for (name, clips) in requires {
            let Some(slot) = names.iter().position(|n| *n == name) else {
                return Err(format!(
                    "requires: '{name}' is not an input or a derived name"
                ));
            };
            requires_by_slot[slot] = clips.into_boxed_slice();
        }
        Ok(Self {
            derived,
            requires: requires_by_slot.into_boxed_slice(),
            layers,
        })
    }
}

/// The shipped table (all asset layers). A missing or malformed table is a
/// content error at first use, like every catalog.
fn table() -> &'static LocomotionTable {
    static TABLE: LazyLock<LocomotionTable> = LazyLock::new(|| {
        let layers = petramond_world::assets::read_layers("animations/player_locomotion.json");
        if layers.is_empty() {
            panic!("animations/player_locomotion.json not found; the player body cannot animate");
        }
        let texts: Vec<&str> = layers.iter().map(|(text, _)| text.as_str()).collect();
        LocomotionTable::parse_layers(&texts)
            .unwrap_or_else(|e| panic!("animations/player_locomotion.json: {e}"))
    });
    &TABLE
}

/// The clip layers for this body this frame: `(clip, time into the clip,
/// weight)`, ready for [`Model::pose_layers`]. Stack-allocated for the shipped
/// table's size.
pub(super) fn layers<'a>(
    model: &'a Model,
    inst: &PlayerRenderInstance,
) -> SmallVec<[(&'a Animation, f32, f32); 24]> {
    let mut out = SmallVec::new();
    if inst.sleeping || inst.seated {
        return out;
    }
    let inputs = Inputs::from_instance(inst);
    for (clip, phase, weight) in table().weights(&inputs, |name| model.animation(name).is_some()) {
        if let Some(anim) = model.animation(clip) {
            out.push((anim, Inputs::phase(inst, phase) * anim.length, weight));
        }
    }
    out
}

// The torso pitches into a stroke; the gaze must still follow the look.
pub(super) fn stabilize_swim_gaze(
    model: &Model,
    pose: &mut [glam::Mat4],
    head: usize,
    inst: &PlayerRenderInstance,
) {
    let water = inst.locomotion.swim.weight.clamp(0.0, 1.0);
    if water == 0.0 {
        return;
    }
    let current = pose[head].to_scale_rotation_translation().1;
    let target = glam::Quat::from_rotation_y(inst.head_yaw)
        * glam::Quat::from_rotation_x(inst.head_pitch)
        * petramond_world::bbmodel::euler_quat(model.bones[head].rotation);
    model.apply_bone_rotation(
        pose,
        head,
        current.slerp(target, water) * current.conjugate(),
    );
}

#[cfg(test)]
mod tests;
