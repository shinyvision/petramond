use mod_sdk::*;
use serde::Deserialize;

use crate::keys::{ARROW_KEY, BOW_KEY};

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BowSpec {
    draw_ticks: u32,
    strain_ticks: u32,
    draw_speed_scale: f32,
    launch_speed: [f32; 2],
    #[serde(default)]
    pull: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrowSpec {
    damage_weak: [f32; 2],
    damage_full: [f32; 2],
    speed_weak: f32,
    speed_full: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    pub full_ticks: u32,
    pub strain_ticks: u32,
    pub speed_scale: f32,
    pub launch_speed: [f32; 2],
}

impl Draw {
    pub fn from_spec(spec: &BowSpec) -> Result<Draw, String> {
        if spec.draw_ticks < 1 {
            return Err("a zero-tick draw is no draw".into());
        }
        Ok(Draw {
            full_ticks: spec.draw_ticks,
            strain_ticks: spec.strain_ticks,
            speed_scale: spec.draw_speed_scale,
            launch_speed: spec.launch_speed,
        })
    }

    pub fn launch_speed(&self, ticks: u32) -> f32 {
        let full = self.full_ticks.max(1);
        let t = if full == 1 {
            1.0
        } else {
            (ticks.clamp(1, full) - 1) as f32 / (full - 1) as f32
        };
        let [weak, strong] = self.launch_speed;
        weak + (strong - weak) * t
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BowRow {
    pub id: ItemId,
    pub draw: Draw,
    pub pull: Vec<Option<String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ArrowRow {
    pub id: ItemId,
    pub name: String,
    pub damage_weak: [f32; 2],
    pub damage_full: [f32; 2],
    pub speed_weak: f32,
    pub speed_full: f32,
}

impl ArrowRow {
    pub fn from_spec(id: ItemId, name: String, spec: &ArrowSpec) -> Result<ArrowRow, String> {
        if !spec.speed_weak.is_finite()
            || !spec.speed_full.is_finite()
            || spec.speed_weak <= 0.0
            || spec.speed_full <= spec.speed_weak
        {
            return Err("speeds must be finite with 0 < speed_weak < speed_full".into());
        }
        Ok(ArrowRow {
            id,
            name,
            damage_weak: spec.damage_weak,
            damage_full: spec.damage_full,
            speed_weak: spec.speed_weak,
            speed_full: spec.speed_full,
        })
    }

    pub fn damage_at(&self, speed: f32) -> [f32; 2] {
        let (start, end, low, high) = if speed < self.speed_weak {
            (0.0, self.speed_weak, [0.0; 2], self.damage_weak)
        } else {
            (
                self.speed_weak,
                self.speed_full,
                self.damage_weak,
                self.damage_full,
            )
        };
        let t = ((speed - start) / (end - start)).clamp(0.0, 1.0);
        [
            low[0] + (high[0] - low[0]) * t,
            low[1] + (high[1] - low[1]) * t,
        ]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Rows {
    pub bows: Vec<BowRow>,
    pub arrows: Vec<ArrowRow>,
}

impl Rows {
    pub fn load() -> Option<Rows> {
        let bows: Vec<BowRow> = items_with_data_as::<BowSpec>(BOW_KEY)
            .into_iter()
            .filter_map(|(id, spec)| match Draw::from_spec(&spec) {
                Ok(draw) => Some(BowRow {
                    id,
                    draw,
                    pull: pull_frames(&spec.pull),
                }),
                Err(reason) => {
                    let row = item_names(vec![id]).pop().flatten();
                    let row = row.unwrap_or_else(|| format!("{id:?}"));
                    log(&row_error(BOW_KEY, &row, &reason));
                    None
                }
            })
            .collect();
        let arrow_rows = items_with_data_as::<ArrowSpec>(ARROW_KEY);
        let names = item_names(arrow_rows.iter().map(|(id, _)| *id).collect());
        let arrows: Vec<ArrowRow> = arrow_rows
            .into_iter()
            .zip(names)
            .filter_map(|((id, spec), name)| {
                let name = name?;
                ArrowRow::from_spec(id, name.clone(), &spec)
                    .map_err(|reason| log(&row_error(ARROW_KEY, &name, &reason)))
                    .ok()
            })
            .collect();
        if bows.is_empty() {
            log(&format!(
                "[combat] no '{BOW_KEY}' rows resolved — the draw stays inert"
            ));
            return None;
        }
        if arrows.is_empty() {
            log(&format!(
                "[combat] no '{ARROW_KEY}' rows resolved — the bow has nothing to loose, the draw stays inert"
            ));
            return None;
        }
        Some(Rows { bows, arrows })
    }

    pub fn bow(&self, held: Option<ItemId>) -> Option<&BowRow> {
        let held = held?;
        self.bows.iter().find(|row| row.id == held)
    }

    pub fn arrow_named(&self, name: &str) -> Option<&ArrowRow> {
        self.arrows.iter().find(|row| row.name == name)
    }
}

fn pull_frames(names: &[String]) -> Vec<Option<String>> {
    names
        .iter()
        .map(|name| {
            let present = resolve_item(name).is_some();
            if !present {
                log(&format!(
                    "[combat] '{name}' did not resolve — that pull frame is skipped"
                ));
            }
            present.then(|| name.clone())
        })
        .collect()
}
