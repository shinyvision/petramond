use petramond_world::bbmodel::{Animation, Model};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use smallvec::SmallVec;

use super::motion::BodyState;

const TABLE_ASSET: &str = "animations/player_locomotion.json";

const MIN_LAYER_WEIGHT: f32 = 0.001;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Inputs {
    pub walking: f32,
    pub sneak: f32,
    pub run: f32,
    pub backward: f32,
    pub strafe: f32,
    pub right: f32,
    pub airborne: f32,
    pub falling: f32,
    pub landing: f32,
    pub swim: f32,
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

    fn from_state(state: &BodyState) -> Self {
        let mix = state.locomotion;
        let swim = mix.swim;
        let unit = |v: f32| v.clamp(0.0, 1.0);
        Self {
            walking: unit(state.walk_weight),
            sneak: unit(state.sneak_weight),
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

    fn phase(state: &BodyState, source: Phase) -> f32 {
        match source {
            Phase::Stride => state.anim_time,
            Phase::Swim => state.locomotion.swim.phase,
            Phase::Rest => 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Stride,
    Swim,
    Rest,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
struct Factor {
    slot: usize,
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

pub struct LocomotionTable {
    derived: Box<[Derived]>,
    requires: Box<[Box<[String]>]>,
    layers: Box<[Layer]>,
}

impl LocomotionTable {
    #[cfg(test)]
    fn empty() -> Self {
        Self {
            derived: Box::new([]),
            requires: vec![Box::<[String]>::default(); Inputs::NAMES.len()].into_boxed_slice(),
            layers: Box::new([]),
        }
    }

    #[cfg(test)]
    pub fn load() -> Result<Self, String> {
        let layers = petramond_world::assets::read_layers(TABLE_ASSET);
        if layers.is_empty() {
            return Err(format!("{TABLE_ASSET} not found"));
        }
        let texts: Vec<&str> = layers.iter().map(|(text, _)| text.as_str()).collect();
        Self::parse_layers(&texts).map_err(|e| format!("{TABLE_ASSET}: {e}"))
    }

    fn slots(&self) -> usize {
        Inputs::NAMES.len() + self.derived.len()
    }

    pub fn weights<'a>(
        &'a self,
        inputs: &Inputs,
        has_clip: impl Fn(&str) -> bool + 'a,
    ) -> impl Iterator<Item = (&'a str, Phase, f32)> + 'a {
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

pub static TABLE: petramond_world::content::Slot<LocomotionTable> =
    petramond_world::content::Slot::new(TABLE_ASSET, &[], load_table);

fn load_table(reg: &petramond_world::content::ContentRegistry) -> Result<LocomotionTable, String> {
    petramond_world::registry::read_catalog(
        reg.packs(),
        TABLE_ASSET,
        "player locomotion",
        LocomotionTable::parse_layers,
    )
}

fn table() -> &'static LocomotionTable {
    TABLE.current()
}

pub(super) fn layers<'a>(
    model: &'a Model,
    state: &BodyState,
) -> SmallVec<[(&'a Animation, f32, f32); 24]> {
    let mut out = SmallVec::new();
    if state.sleeping || state.seated {
        return out;
    }
    let inputs = Inputs::from_state(state);
    for (clip, phase, weight) in table().weights(&inputs, |name| model.animation(name).is_some()) {
        if let Some(anim) = model.animation(clip) {
            out.push((anim, Inputs::phase(state, phase) * anim.length, weight));
        }
    }
    out
}

pub(super) fn stabilize_swim_gaze(
    model: &Model,
    pose: &mut [glam::Mat4],
    head: usize,
    state: &BodyState,
) {
    let water = state.locomotion.swim.weight.clamp(0.0, 1.0);
    if water == 0.0 {
        return;
    }
    let current = pose[head].to_scale_rotation_translation().1;
    let target = glam::Quat::from_rotation_y(state.head_yaw)
        * glam::Quat::from_rotation_x(state.head_pitch)
        * petramond_world::bbmodel::euler_quat(model.bones[head].rotation);
    model.apply_bone_rotation(
        pose,
        head,
        current.slerp(target, water) * current.conjugate(),
    );
}

#[cfg(test)]
mod tests;
