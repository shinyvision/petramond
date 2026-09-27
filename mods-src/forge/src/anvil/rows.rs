use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use crate::anvil::{BASE_SOCKETS, SOCKETS};
use crate::keys;
use crate::schema::{read_rows, FitSpec, GentleSpec, SlotsSpec, WearOnSpec};

#[derive(Clone)]
pub(super) struct Fit {
    pub(super) tool: String,
    pub(super) tier: u8,
    pub(super) speed_mult: f32,
    pub(super) damage_mult: f32,
    pub(super) knockback_mult: f32,
    pub(super) cost: u8,
    pub(super) overlay: String,
    pub(super) overlays: Vec<(String, String)>,
    pub(super) gentle: Option<u8>,
    pub(super) wear: Option<Wear>,
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WearOn {
    Break,
    Hit,
    Proc,
}

#[derive(Clone, Copy)]
pub(super) struct Wear {
    pub(super) on: WearOn,
    pub(super) max: u32,
}

impl Fit {
    pub(super) fn overlay_for(&self, family: &str) -> &str {
        self.overlays
            .iter()
            .find(|(f, _)| f == family)
            .map(|(_, o)| o.as_str())
            .unwrap_or(&self.overlay)
    }
}

pub(super) struct ToolSlots {
    pub(super) family: String,
    pub(super) lockable: u8,
}

pub(super) struct ToolStats {
    pub(super) kind: String,
    pub(super) tier: u8,
    pub(super) speed: f32,
    pub(super) damage: [f32; 2],
    pub(super) knockback: f32,
}

pub(super) fn augment_fits() -> HashMap<String, Vec<Fit>> {
    read_rows::<Vec<FitSpec>>(keys::AUGMENT_DATA)
        .into_iter()
        .filter(|(_, fits)| !fits.is_empty())
        .map(|(material, fits)| (material, fits.into_iter().map(Fit::from).collect()))
        .collect()
}

impl From<FitSpec> for Fit {
    fn from(f: FitSpec) -> Fit {
        Fit {
            tool: f.tool,
            tier: f.tier,
            speed_mult: f.speed_mult.unwrap_or(1.0),
            damage_mult: f.damage_mult.unwrap_or(1.0),
            knockback_mult: f.knockback_mult.unwrap_or(1.0),
            cost: f.cost.unwrap_or(1).max(1),
            overlay: f.overlay,
            overlays: f.overlays.into_iter().collect(),
            gentle: f.gentle.as_ref().map(GentleSpec::chance),
            wear: f.wear.map(|w| Wear {
                on: match w.on {
                    WearOnSpec::Break => WearOn::Break,
                    WearOnSpec::Hit => WearOn::Hit,
                    WearOnSpec::Proc => WearOn::Proc,
                },
                max: (w.max as u32).max(100),
            }),
        }
    }
}

pub(super) fn index_by_identity(
    augments: &HashMap<String, Vec<Fit>>,
) -> (HashMap<String, Vec<Fit>>, HashMap<String, String>) {
    let mut by_identity: HashMap<String, Vec<Fit>> = HashMap::new();
    let mut material_of: HashMap<String, String> = HashMap::new();
    for (material, fits) in augments {
        for (i, fit) in fits.iter().enumerate() {
            if let Some(first) = fits[..i].iter().find(|f| f.tool == fit.tool) {
                log(&format!(
                    "forge: '{material}' offers {} tools both '{}' and '{}'; only the first \
                     can be fitted",
                    fit.tool, first.overlay, fit.overlay
                ));
            }
            let per_kind = by_identity.entry(fit.overlay.clone()).or_default();
            if per_kind.iter().any(|f: &Fit| f.tool == fit.tool) {
                log(&format!(
                    "forge: augment identity '{}' names more than one {} fit",
                    fit.overlay, fit.tool
                ));
            }
            per_kind.push(fit.clone());
            material_of.insert(fit.overlay.clone(), material.clone());
        }
    }
    (by_identity, material_of)
}

pub(super) fn display_names(by_identity: &HashMap<String, Vec<Fit>>) -> HashMap<String, String> {
    by_identity
        .keys()
        .filter_map(|id| Some((id.clone(), item_info(id)?.display_name)))
        .collect()
}

pub(super) fn augmentable_tools() -> HashMap<String, (ToolSlots, ToolStats)> {
    let slotted = read_rows::<SlotsSpec>(keys::AUGMENT_SLOTS_DATA);
    slotted
        .into_iter()
        .filter_map(|(name, spec)| {
            let slots = ToolSlots {
                family: spec.family.unwrap_or_else(|| "default".to_owned()),
                lockable: spec.lockable.unwrap_or(0).min(SOCKETS as u8 - BASE_SOCKETS),
            };
            let info = item_info(&name)?;
            let tool = info.tool?;
            Some((
                name,
                (
                    slots,
                    ToolStats {
                        kind: tool.kind,
                        tier: tool.tier,
                        speed: tool.speed,
                        damage: tool.damage,
                        knockback: tool.knockback,
                    },
                ),
            ))
        })
        .collect()
}

pub(super) fn items_with(key: &str) -> HashSet<String> {
    read_rows::<serde::de::IgnoredAny>(key)
        .into_keys()
        .collect()
}
