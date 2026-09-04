//! The bow's rows: every item the registry carries as a BOW or an ARROW,
//! each with its authored numbers, read ONCE at init. A second bow tier or
//! an iron arrow is one more row carrying the key — never a code change.

use mod_sdk::*;

/// The item-row data key naming a bow and its draw:
/// `{"draw_ticks", "strain_ticks", "draw_speed_scale", "launch_speed":
/// [weakest, fullest], "pull"?: [frame item names, weakest first]}`.
pub const BOW_KEY: &str = "combat:bow";

/// The item-row data key naming an arrow and its damage by arrival speed:
/// `{"damage_weak": [min, max], "damage_full": [min, max], "speed_weak",
/// "speed_full"}` — the ranges dealt arriving at the two speeds (m/s),
/// linear between, clamped outside.
pub const ARROW_KEY: &str = "combat:arrow";

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
    /// Read a bow row's data; `None` for any missing or malformed field —
    /// a bow with half its numbers is refused whole.
    pub fn parse(v: &json::Value) -> Option<Draw> {
        let full_ticks = ticks(v, "draw_ticks")?;
        (full_ticks >= 1).then_some(())?;
        Some(Draw {
            full_ticks,
            strain_ticks: ticks(v, "strain_ticks")?,
            speed_scale: num(v, "draw_speed_scale")?,
            launch_speed: pair(v, "launch_speed")?,
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
    /// Read an arrow row's data; `None` refuses the row whole.
    pub fn parse(id: ItemId, name: String, v: &json::Value) -> Option<ArrowRow> {
        let speed_weak = num(v, "speed_weak")?;
        let speed_full = num(v, "speed_full")?;
        (speed_full > speed_weak).then_some(())?;
        Some(ArrowRow {
            id,
            name,
            damage_weak: pair(v, "damage_weak")?,
            damage_full: pair(v, "damage_full")?,
            speed_weak,
            speed_full,
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
        let bows: Vec<BowRow> = items_with_data(BOW_KEY)
            .into_iter()
            .filter_map(|(id, text)| {
                let row = json::Value::parse(&text).and_then(|v| {
                    let draw = Draw::parse(&v)?;
                    let pull = pull_frames(&v)?;
                    Some(BowRow { id, draw, pull })
                });
                if row.is_none() {
                    log(&format!(
                        "[combat] a '{BOW_KEY}' row's data is incomplete — that bow is skipped: {text}"
                    ));
                }
                row
            })
            .collect();
        let arrow_rows = items_with_data(ARROW_KEY);
        let names = item_names(arrow_rows.iter().map(|(id, _)| *id).collect());
        let arrows: Vec<ArrowRow> = arrow_rows
            .into_iter()
            .zip(names)
            .filter_map(|((id, text), name)| {
                let row = name.and_then(|name| {
                    json::Value::parse(&text).and_then(|v| ArrowRow::parse(id, name, &v))
                });
                if row.is_none() {
                    log(&format!(
                        "[combat] an '{ARROW_KEY}' row's data is incomplete — that arrow is skipped: {text}"
                    ));
                }
                row
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
/// holds), never a dead bow. No list = no frames.
fn pull_frames(v: &json::Value) -> Option<Vec<Option<String>>> {
    let Some(list) = v.get("pull") else {
        return Some(Vec::new());
    };
    list.as_array()?
        .iter()
        .map(|entry| {
            let name = entry.as_str()?;
            let present = resolve_item(name).is_some();
            if !present {
                log(&format!(
                    "[combat] '{name}' did not resolve — that pull frame is skipped"
                ));
            }
            Some(present.then(|| name.to_owned()))
        })
        .collect()
}

fn num(v: &json::Value, key: &str) -> Option<f32> {
    v.get(key)?.as_f64().map(|n| n as f32)
}

fn ticks(v: &json::Value, key: &str) -> Option<u32> {
    let n = v.get(key)?.as_f64()?;
    (n.fract() == 0.0 && (0.0..=u32::MAX as f64).contains(&n)).then_some(n as u32)
}

fn pair(v: &json::Value, key: &str) -> Option<[f32; 2]> {
    let list = v.get(key)?.as_array()?;
    match list {
        [a, b] => Some([a.as_f64()? as f32, b.as_f64()? as f32]),
        _ => None,
    }
}
