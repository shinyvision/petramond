use crate::charge::Draw;
use crate::keys::BOOMERANG_KEY;
use mod_sdk::*;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Spec {
    draw_ticks: u32,
    strain_ticks: u32,
    draw_speed_scale: f32,
    launch_speed: [f32; 2],
    outbound_ticks: [u32; 2],
    damage_weak: [f32; 2],
    damage_full: [f32; 2],
    return_speed: f32,
    curve: f32,
    turn_acceleration: f32,
    animations: Clips,
}

#[derive(Clone, Debug)]
pub(super) struct Row {
    pub id: ItemId,
    pub name: String,
    pub draw: Draw,
    pub outbound_ticks: [u32; 2],
    pub damage_weak: [f32; 2],
    pub damage_full: [f32; 2],
    pub return_speed: f32,
    pub curve: f32,
    pub turn_acceleration: f32,
    pub motion: Motion,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Clips {
    pub first_person: [String; 2],
    pub body: [String; 2],
}

#[derive(Clone, Debug)]
pub(super) struct Motion {
    pub first_person: [String; 2],
    pub body: [String; 2],
    pub length: f32,
    pub fp_length: f32,
    pub fp_release: f32,
}

impl Motion {
    fn resolve(clips: Clips) -> Result<Self, &'static str> {
        let release = |rig: &str, names: &[String; 2]| {
            animation_clip(rig, &names[0]).ok_or("missing draw clip")?;
            let clip = animation_clip(rig, &names[1]).ok_or("missing throw clip")?;
            let at = clip
                .markers
                .iter()
                .find(|(name, _)| name == "release")
                .map(|(_, at)| *at)
                .ok_or("throw clip needs a release marker")?;
            if !clip.length.is_finite() || !at.is_finite() || at <= 0.0 || at >= clip.length {
                return Err("release marker must be inside the throw");
            }
            Ok((clip.length, at))
        };
        let (length, _) = release(rig::PLAYER_BODY, &clips.body)?;
        let (fp_length, fp_release) = release(rig::PLAYER_FIRST_PERSON, &clips.first_person)?;
        Ok(Self {
            first_person: clips.first_person,
            body: clips.body,
            length,
            fp_length,
            fp_release,
        })
    }

    pub fn release_time(&self) -> f32 {
        self.fp_release
    }

    pub fn throw_length(&self) -> f32 {
        self.length.max(self.fp_length)
    }

    pub fn throw_times(&self, elapsed: f32) -> [Option<f32>; 2] {
        [self.fp_length, self.length]
            .map(|length| (elapsed < length).then_some((elapsed / length).clamp(0.0, 1.0)))
    }
}

impl Row {
    pub fn from_spec(id: ItemId, name: String, spec: Spec) -> Result<Self, &'static str> {
        let positive = |v: f32| v.is_finite() && v > 0.0;
        let band = |[lo, hi]: [f32; 2]| lo.is_finite() && hi.is_finite() && lo >= 0.0 && hi >= lo;
        if spec.draw_ticks == 0
            || spec.draw_ticks.checked_add(spec.strain_ticks).is_none()
            || !positive(spec.draw_speed_scale)
            || !spec.launch_speed.into_iter().all(positive)
            || spec.launch_speed[1] < spec.launch_speed[0]
            || !positive(spec.return_speed)
            || spec.outbound_ticks[0] == 0
            || spec.outbound_ticks[1] < spec.outbound_ticks[0]
            || !band(spec.damage_weak)
            || !band(spec.damage_full)
            || !spec.curve.is_finite()
            || !positive(spec.curve)
            || spec.curve > 1.0
            || !positive(spec.turn_acceleration)
            || spec.launch_speed[1] > 100.0
            || spec.return_speed > 100.0
        {
            return Err("invalid charge, flight or damage band");
        }
        Ok(Self {
            id,
            name,
            draw: Draw {
                full_ticks: spec.draw_ticks,
                strain_ticks: spec.strain_ticks,
                speed_scale: spec.draw_speed_scale,
                launch_speed: spec.launch_speed,
            },
            outbound_ticks: spec.outbound_ticks,
            damage_weak: spec.damage_weak,
            damage_full: spec.damage_full,
            return_speed: spec.return_speed,
            curve: spec.curve,
            turn_acceleration: spec.turn_acceleration,
            motion: Motion::resolve(spec.animations)?,
        })
    }
}

pub(super) fn load() -> Vec<Row> {
    let specs = items_with_data_as::<Spec>(BOOMERANG_KEY);
    let names = item_names(specs.iter().map(|(id, _)| *id).collect());
    specs
        .into_iter()
        .zip(names)
        .filter_map(|((id, spec), name)| {
            let name = name?;
            Row::from_spec(id, name.clone(), spec)
                .map_err(|e| log(&row_error(BOOMERANG_KEY, &name, e)))
                .ok()
        })
        .collect()
}
