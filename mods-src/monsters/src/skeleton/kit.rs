//! What a skeleton carries and what that lets it do.
//!
//! A LOADOUT (skeleton row data `monsters:loadouts`) names the items in each hand. What an item
//! does in a skeleton's hand is the item row's own `monsters:wield` data, so any pack makes an
//! item wieldable with a patch row. A loadout naming an item the registry does not know is dropped
//! whole, which is how the combat pack's weapons disappear from camps without that pack.

use mod_sdk::*;
use serde::Deserialize;

use super::aim::Flight;
use super::keys::{LOADOUTS_DATA, PROJECTILE_DATA, SKELETON, UNARMED_DATA, WIELD_DATA};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Melee {
    pub damage: f32,
    #[serde(default)]
    pub knockback: f32,
    pub reach: f32,
    pub windup: u32,
    pub cooldown: u32,
    pub clip: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Ranged {
    pub ammo: String,
    #[serde(default)]
    pub pull: Vec<String>,
    pub draw: u32,
    pub speed: f32,
    #[serde(default)]
    pub spread: f32,
    pub min_range: f32,
    pub max_range: f32,
    pub rest: u32,
    pub clip_draw: String,
    pub clip_loose: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Guard {
    pub arc_deg: f32,
    pub raise: [u32; 2],
    pub lower: [u32; 2],
    pub clip: String,
    pub impact_clip: String,
    #[serde(default)]
    pub sound: Option<String>,
}

/// An item row's `monsters:wield`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Wield {
    #[serde(default)]
    pub grip: Option<String>,
    #[serde(default)]
    pub stance: Option<String>,
    #[serde(default)]
    pub walk_stance: Option<String>,
    #[serde(default)]
    pub melee: Option<Melee>,
    #[serde(default)]
    pub ranged: Option<Ranged>,
    #[serde(default)]
    pub guard: Option<Guard>,
}

fn clip_ok(clip: &str) -> bool {
    !clip.is_empty() && clip.len() <= MAX_MOB_ANIM_NAME_BYTES
}

impl Melee {
    pub fn check(&self) -> Result<(), String> {
        if !(self.damage.is_finite() && self.damage >= 0.0) {
            return Err("melee damage must be a finite number of at least 0".into());
        }
        if !(self.knockback.is_finite() && self.knockback >= 0.0) {
            return Err("melee knockback must be a finite number of at least 0".into());
        }
        if !(self.reach.is_finite() && self.reach > 0.0) {
            return Err("melee reach must be positive".into());
        }
        if self.windup == 0 || self.windup > self.cooldown {
            return Err("melee windup must be at least 1 tick and no longer than cooldown".into());
        }
        if !clip_ok(&self.clip) {
            return Err("melee clip must be a clip name".into());
        }
        Ok(())
    }

    /// Damage per tick of attacking: how two melee items are ranked.
    pub fn rate(&self) -> f32 {
        self.damage / self.cooldown.max(1) as f32
    }
}

impl Ranged {
    pub fn display(&self, elapsed: u64) -> Option<&str> {
        let stage = (elapsed.min(u64::from(self.draw)) * self.pull.len() as u64
            / u64::from(self.draw.max(1))) as usize;
        self.pull[..stage]
            .iter()
            .rev()
            .find_map(|s| (!s.is_empty()).then_some(s.as_str()))
    }

    fn check(&self) -> Result<(), String> {
        if self.draw == 0 || !(self.speed.is_finite() && self.speed > 0.0) {
            return Err("ranged draw and speed must be positive".into());
        }
        if !(self.spread.is_finite() && self.spread >= 0.0) {
            return Err("ranged spread must be at least 0".into());
        }
        let band = self.min_range.is_finite()
            && self.max_range.is_finite()
            && self.min_range >= 0.0
            && self.min_range < self.max_range
            && self.max_range <= RAYCAST_MAX_DISTANCE;
        if !band {
            return Err(format!(
                "ranged range must satisfy 0 <= min_range < max_range <= {RAYCAST_MAX_DISTANCE}"
            ));
        }
        if !clip_ok(&self.clip_draw) || !clip_ok(&self.clip_loose) {
            return Err("ranged clips must be clip names".into());
        }
        Ok(())
    }
}

impl Guard {
    fn check(&self) -> Result<(), String> {
        if !(self.arc_deg > 0.0 && self.arc_deg <= 180.0) {
            return Err("guard arc_deg must be in (0, 180]".into());
        }
        let span = |[lo, hi]: [u32; 2]| lo >= 1 && lo <= hi;
        if !span(self.raise) || !span(self.lower) {
            return Err("guard raise and lower must be [min, max] with 1 <= min <= max".into());
        }
        if !clip_ok(&self.clip) || !clip_ok(&self.impact_clip) {
            return Err("guard clips must be clip names".into());
        }
        Ok(())
    }
}

impl Wield {
    pub fn check(&self) -> Result<(), String> {
        if let Some(melee) = &self.melee {
            melee.check()?;
        }
        if let Some(ranged) = &self.ranged {
            ranged.check()?;
        }
        if let Some(guard) = &self.guard {
            guard.check()?;
        }
        if self.stance.as_deref().is_some_and(|s| !clip_ok(s)) {
            return Err("stance must be a clip name".into());
        }
        if self.grip.as_deref().is_some_and(|s| !clip_ok(s)) {
            return Err("grip must be a clip name".into());
        }
        if self.walk_stance.as_deref().is_some_and(|s| !clip_ok(s)) {
            return Err("walk_stance must be a clip name".into());
        }
        Ok(())
    }
}

/// One entry of the skeleton row's `monsters:loadouts`.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoadoutSpec {
    pub name: String,
    #[serde(default)]
    pub main: Option<String>,
    #[serde(default)]
    pub off: Option<String>,
    pub weight: u32,
    #[serde(default)]
    pub watch: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Main,
    Off,
}

impl Hand {
    pub const BOTH: [Hand; 2] = [Hand::Main, Hand::Off];

    pub fn index(self) -> usize {
        match self {
            Hand::Main => 0,
            Hand::Off => 1,
        }
    }
}

/// A melee attack and the hand it comes from (`None` = the bare-handed fallback).
#[derive(Clone, Debug, PartialEq)]
pub struct Swing {
    pub hand: Option<Hand>,
    pub melee: Melee,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bow {
    pub hand: Hand,
    pub ranged: Ranged,
    pub flight: Flight,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shield {
    pub hand: Hand,
    pub guard: Guard,
}

/// What a loadout can do, derived from the items' wield rows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Kit {
    pub held: [Option<String>; 2],
    pub grips: [Option<String>; 2],
    pub stances: [Option<String>; 2],
    pub walk_stances: [Option<String>; 2],
    pub swing: Option<Swing>,
    pub bow: Option<Bow>,
    pub shield: Option<Shield>,
}

impl Kit {
    /// The best melee in either hand is the attack; with none, the bare-handed fallback.
    pub fn of(
        held: [Option<String>; 2],
        wield: impl Fn(&str) -> Option<Wield>,
        flight: impl Fn(&str) -> Option<Flight>,
        unarmed: Option<&Melee>,
    ) -> Kit {
        let mut kit = Kit {
            held: held.clone(),
            ..Kit::default()
        };
        for hand in Hand::BOTH {
            let Some(w) = held[hand.index()].as_deref().and_then(&wield) else {
                continue;
            };
            kit.stances[hand.index()] = w.stance.clone();
            kit.walk_stances[hand.index()] = w.walk_stance.clone();
            kit.grips[hand.index()] = w.grip.clone();
            if let Some(melee) = w.melee {
                let better = kit
                    .swing
                    .as_ref()
                    .is_none_or(|s| melee.rate() > s.melee.rate());
                if better {
                    kit.swing = Some(Swing {
                        hand: Some(hand),
                        melee,
                    });
                }
            }
            if let (None, Some(ranged)) = (&kit.bow, w.ranged) {
                if let Some(flight) = flight(&ranged.ammo) {
                    kit.bow = Some(Bow {
                        hand,
                        ranged,
                        flight,
                    });
                }
            }
            if let (None, Some(guard)) = (&kit.shield, w.guard) {
                kit.shield = Some(Shield { hand, guard });
            }
        }
        if kit.swing.is_none() {
            kit.swing = unarmed.map(|melee| Swing {
                hand: None,
                melee: melee.clone(),
            });
        }
        kit
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Loadout {
    pub name: String,
    pub weight: u32,
    pub watch: u32,
    pub kit: Kit,
}

/// Keeps the loadouts whose every item is `known`, in row order, once per name; returns the
/// reason for each one left out.
pub fn admit(
    specs: Vec<LoadoutSpec>,
    known: impl Fn(&str) -> bool,
) -> (Vec<LoadoutSpec>, Vec<String>) {
    let mut kept: Vec<LoadoutSpec> = Vec::with_capacity(specs.len());
    let mut dropped = Vec::new();
    for spec in specs {
        if kept.iter().any(|k| k.name == spec.name) {
            dropped.push(format!("loadout '{}' is listed twice", spec.name));
            continue;
        }
        let missing = [&spec.main, &spec.off]
            .into_iter()
            .flatten()
            .find(|item| !known(item));
        match missing {
            Some(item) => dropped.push(format!(
                "loadout '{}' needs '{item}', which is not registered",
                spec.name
            )),
            None => kept.push(spec),
        }
    }
    (kept, dropped)
}

/// Index of the loadout `roll` lands on. Watch posts weigh by `watch` while any loadout has a
/// watch weight; `None` when nothing has weight.
pub fn pick(loadouts: &[Loadout], watch: bool, roll: u64) -> Option<usize> {
    let use_watch = watch && loadouts.iter().any(|l| l.watch > 0);
    let weight = |l: &Loadout| u64::from(if use_watch { l.watch } else { l.weight });
    let total: u64 = loadouts.iter().map(weight).sum();
    if total == 0 {
        return None;
    }
    let mut ticket = roll % total;
    loadouts.iter().position(|l| {
        let w = weight(l);
        if ticket < w {
            true
        } else {
            ticket -= w;
            false
        }
    })
}

/// Every loadout a skeleton can be given, read once at init.
#[derive(Default)]
pub struct Kits {
    pub loadouts: Vec<Loadout>,
    /// Empty hands: what a skeleton with no loadout fights with.
    pub bare: Kit,
}

impl Kits {
    pub fn index_of(&self, name: &str) -> Option<usize> {
        self.loadouts.iter().position(|l| l.name == name)
    }

    pub fn kit(&self, index: Option<usize>) -> &Kit {
        index
            .and_then(|i| self.loadouts.get(i))
            .map_or(&self.bare, |loadout| &loadout.kit)
    }

    pub fn load(skeleton: MobId) -> Kits {
        let wields = wield_rows();
        let unarmed =
            row_data::<Melee>(skeleton, UNARMED_DATA).filter(|melee| match melee.check() {
                Ok(()) => true,
                Err(reason) => {
                    log(&row_error(UNARMED_DATA, SKELETON, &reason));
                    false
                }
            });
        let specs = row_data::<Vec<LoadoutSpec>>(skeleton, LOADOUTS_DATA).unwrap_or_default();
        let (specs, dropped) = admit(specs, |item| resolve_item(item).is_some());
        for reason in dropped {
            log(&format!("skeleton {reason}; left out"));
        }
        let wield = |item: &str| {
            wields
                .iter()
                .find(|(n, _)| n == item)
                .map(|(_, w)| w.clone())
        };
        let flight = |ammo: &str| {
            let Some(id) = resolve_item(ammo) else {
                log(&format!("skeleton ammo '{ammo}' is not registered"));
                return None;
            };
            Some(
                item_data(id, PROJECTILE_DATA)
                    .and_then(|raw| parse_row_data::<Flight>(&String::from_utf8_lossy(&raw)).ok())
                    .unwrap_or_default(),
            )
        };
        let loadouts = specs
            .into_iter()
            .map(|spec| Loadout {
                kit: Kit::of(
                    [spec.main.clone(), spec.off.clone()],
                    wield,
                    flight,
                    unarmed.as_ref(),
                ),
                watch: spec.watch.unwrap_or(spec.weight),
                weight: spec.weight,
                name: spec.name,
            })
            .collect();
        let bare = Kit::of([None, None], |_| None, |_| None, unarmed.as_ref());
        Kits { loadouts, bare }
    }
}

fn row_data<T: serde::de::DeserializeOwned>(mob: MobId, key: &str) -> Option<T> {
    let raw = mob_data(mob, key)?;
    match parse_row_data::<T>(&String::from_utf8_lossy(&raw)) {
        Ok(value) => Some(value),
        Err(reason) => {
            log(&row_error(key, SKELETON, &reason));
            None
        }
    }
}

fn wield_rows() -> Vec<(String, Wield)> {
    let rows = items_with_data_as::<Wield>(WIELD_DATA);
    let names = item_names(rows.iter().map(|(id, _)| *id).collect());
    rows.into_iter()
        .zip(names)
        .filter_map(|((id, mut wield), name)| {
            let name = name.unwrap_or_else(|| format!("{id:?}"));
            if let Some(ranged) = &mut wield.ranged {
                for frame in &mut ranged.pull {
                    if resolve_item(frame).is_none() {
                        log(&format!("skeleton bow frame '{frame}' is not registered"));
                        frame.clear();
                    }
                }
            }
            match wield.check() {
                Ok(()) => Some((name, wield)),
                Err(reason) => {
                    log(&row_error(WIELD_DATA, &name, &reason));
                    None
                }
            }
        })
        .collect()
}
