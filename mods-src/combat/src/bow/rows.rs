//! The bow's rows: every item the registry carries as a BOW or an ARROW,
//! each with its authored numbers, read ONCE at init. A second bow tier or
//! an iron arrow is one more row carrying the key — never a code change.

use mod_sdk::*;
use serde::Deserialize;

use crate::keys::{ARROW_KEY, BOW_KEY};

/// The [`BOW_KEY`] entry, as a pack writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BowSpec {
    draw_ticks: u32,
    strain_ticks: u32,
    draw_speed_scale: f32,
    launch_speed: [f32; 2],
    /// The pull frames' item names, weakest first; none = no frames.
    #[serde(default)]
    pull: Vec<String>,
}

/// The [`ARROW_KEY`] entry, as a pack writes it.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArrowSpec {
    damage_weak: [f32; 2],
    damage_full: [f32; 2],
    speed_weak: f32,
    speed_full: f32,
}

/// One bow's authored draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Draw {
    /// Ticks from the press to a full draw; the draw STATE is this many
    /// steps.
    pub full_ticks: u32,
    /// Ticks a FULL draw can be held before the strain looses it by
    /// itself; the shake grows over this window.
    pub strain_ticks: u32,
    /// Land-speed multiplier while drawing.
    pub speed_scale: f32,
    /// The arrow's launch speed (m/s) at the weakest and the fullest draw.
    pub launch_speed: [f32; 2],
}

impl Draw {
    /// A bow row's draw. A spec missing a field never parses (a bow with
    /// half its numbers is refused whole); a zero-tick draw is no draw.
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

    /// The arrow's launch speed for a draw of `ticks` (1 ..= full).
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

/// One bow row.
#[derive(Clone, Debug, PartialEq)]
pub struct BowRow {
    pub id: ItemId,
    pub draw: Draw,
    /// The pull frames' NAMES (what the display claim speaks), weakest
    /// first; the LAST is the fully drawn bow. Positional: a frame the
    /// registry lacks is `None`, and the previous one holds in its place.
    pub pull: Vec<Option<String>>,
}

/// One arrow row.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrowRow {
    pub id: ItemId,
    /// The registry name — what the bow spends and launches, and what a
    /// hit is matched against.
    pub name: String,
    pub damage_weak: [f32; 2],
    pub damage_full: [f32; 2],
    pub speed_weak: f32,
    pub speed_full: f32,
}

impl ArrowRow {
    /// An arrow row from its spec; the full speed must exceed the weak one.
    pub fn from_spec(id: ItemId, name: String, spec: &ArrowSpec) -> Result<ArrowRow, String> {
        if spec.speed_full <= spec.speed_weak {
            return Err("speed_full must exceed speed_weak".into());
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

    /// The damage `[min, max]` this arrow deals arriving at `speed` (m/s):
    /// the weak range at the weak speed, the full range at the full one,
    /// straight between — so damage falls away with the flight exactly as
    /// the speed does, and a half draw lands halfway.
    pub fn damage_at(&self, speed: f32) -> [f32; 2] {
        let t = ((speed - self.speed_weak) / (self.speed_full - self.speed_weak)).clamp(0.0, 1.0);
        [
            self.damage_weak[0] + (self.damage_full[0] - self.damage_weak[0]) * t,
            self.damage_weak[1] + (self.damage_full[1] - self.damage_weak[1]) * t,
        ]
    }
}

/// Every bow and arrow this registry carries.
#[derive(Clone, Debug, PartialEq)]
pub struct Rows {
    pub bows: Vec<BowRow>,
    /// In registry order — the order the pack spends them in.
    pub arrows: Vec<ArrowRow>,
}

impl Rows {
    /// Sweep the registry for both keys. `None` when either table is
    /// empty: a build with no bow, or no arrow to loose, leaves the whole
    /// law inert.
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

    /// The bow row `held` names, if it is one.
    pub fn bow(&self, held: Option<ItemId>) -> Option<&BowRow> {
        let held = held?;
        self.bows.iter().find(|row| row.id == held)
    }

    /// The arrow row of registry `name`, if it is one.
    pub fn arrow_named(&self, name: &str) -> Option<&ArrowRow> {
        self.arrows.iter().find(|row| row.name == name)
    }
}

/// The row's `pull` list, each frame resolved against the registry: a
/// frame the registry lacks is logged and left `None` (the previous one
/// holds), never a dead bow.
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
