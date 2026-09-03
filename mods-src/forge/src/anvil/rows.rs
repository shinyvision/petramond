//! The anvil's ROW vocabulary: what a fit, an augmentable tool and a wear
//! config are, and the loaders that resolve each table out of item row data.
//!
//! Both sides of "which tools take which augments" are data, so both arrive
//! here and nowhere else — the machine holds tables, never item names. The
//! loaders RETURN their tables rather than filling the spec, which keeps
//! `init` a short assembly and the vocabulary readable on its own.

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use crate::anvil::{BASE_SOCKETS, SOCKETS};
use crate::augments::AUGMENT_KEY;
use crate::content::read_rows;

/// One fit out of an augment material's `forge:augment` list.
#[derive(Clone)]
pub(super) struct Fit {
    /// The tool KIND it fits (`"pickaxe"`, matched against the base tool's).
    pub(super) tool: String,
    /// The harvest gate the augment's material reaches — the edge's tier.
    pub(super) tier: u8,
    pub(super) speed_mult: f32,
    pub(super) damage_mult: f32,
    /// Multiplier on the tool's knockback — how much harder its hits shove.
    pub(super) knockback_mult: f32,
    /// How many of the material one application consumes.
    pub(super) cost: u8,
    /// The recorded augment identity, and the overlay ART fallback for any
    /// family `overlays` does not name. A fit may name EVERY family there
    /// (the gold inlay does, per kind) so several kinds can share one
    /// identity — an identity rename strands saved records, art names don't.
    pub(super) overlay: String,
    /// Per-silhouette-family art overrides (`"stone"` → the stone-family
    /// overlay item). Tool sprites differ per material family, and overlay
    /// art is authored IN POSITION, so one drawing cannot hug two contours.
    pub(super) overlays: Vec<(String, String)>,
    /// The gentle-mining CHANCE the fit grants, when it grants one (the
    /// policy itself lives in `gold.rs`; the anvil reads it for the panel's
    /// preview line, and refuses fitting it to a tool whose row already
    /// carries the behaviour innately — a gold pickaxe gains nothing from a
    /// gold inlay).
    pub(super) gentle: Option<u8>,
    /// When and how fast the augment WEARS (absent = never). `max` is the
    /// advertised pool in qualifying EVENTS; storage stays in quanta.
    pub(super) wear: Option<Wear>,
}

/// The event class a fit's condition wears on.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum WearOn {
    /// Every block broken with the tool (the diamond edge).
    Break,
    /// Every mob hit with the tool (the fang).
    Hit,
    /// Every time the granting mod's own behaviour fires (the gold
    /// inlay's gentle proc — rolled by `gold.rs`, not by events).
    Proc,
}

/// A fit's wear config: one condition quantum (1% of base) is lost with
/// probability `100 / max` per qualifying event, so `max` sets the expected
/// pool in EVENTS while the stored value space stays 0..=250 — every
/// distinct stored value mints an interned variant, and the intern table
/// never evicts.
#[derive(Clone, Copy)]
pub(super) struct Wear {
    pub(super) on: WearOn,
    pub(super) max: u32,
}

impl Fit {
    /// The overlay ART item for a tool of `family` (the identity recorded on
    /// the stack stays [`Fit::overlay`] regardless).
    pub(super) fn overlay_for(&self, family: &str) -> &str {
        self.overlays
            .iter()
            .find(|(f, _)| f == family)
            .map(|(_, o)| o.as_str())
            .unwrap_or(&self.overlay)
    }
}

/// A tool's `forge:augment_slots` row: how many LOCKABLE sockets it has
/// beyond the base one, plus the silhouette FAMILY its sprite belongs to
/// (`"stone"`; absent = the default family — iron, whose silhouette copper
/// and gold share by derivation). The family picks which of a fit's overlay
/// drawings hugs this tool's contour.
pub(super) struct ToolSlots {
    pub(super) family: String,
    pub(super) lockable: u8,
}

/// A base tool's engine-resolved stats (`item_info`, cached at init).
pub(super) struct ToolStats {
    pub(super) kind: String,
    pub(super) tier: u8,
    pub(super) speed: f32,
    pub(super) damage: [f32; 2],
    pub(super) knockback: f32,
}

/// Every augment MATERIAL and the fits its row lists.
pub(super) fn augment_fits() -> HashMap<String, Vec<Fit>> {
    read_rows(AUGMENT_KEY, |value| {
        let fits: Vec<Fit> = value
            .as_array()?
            .iter()
            .filter_map(|f| {
                Some(Fit {
                    tool: f.get("tool")?.as_str()?.to_owned(),
                    tier: f.get("tier").and_then(|t| t.as_u8()).unwrap_or(0),
                    speed_mult: f.get("speed_mult").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
                    damage_mult: f.get("damage_mult").and_then(|v| v.as_f64()).unwrap_or(1.0)
                        as f32,
                    knockback_mult: f
                        .get("knockback_mult")
                        .and_then(|v| v.as_f64())
                        .unwrap_or(1.0) as f32,
                    cost: f.get("cost").and_then(|v| v.as_u8()).unwrap_or(1).max(1),
                    overlay: f.get("overlay")?.as_str()?.to_owned(),
                    overlays: f
                        .get("overlays")
                        .and_then(|o| o.as_object())
                        .map(|kv| {
                            kv.iter()
                                .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                                .collect()
                        })
                        .unwrap_or_default(),
                    gentle: f.get("gentle").map(|g| {
                        g.get("chance")
                            .and_then(|c| c.as_u8())
                            .unwrap_or(100)
                            .clamp(1, 100)
                    }),
                    wear: f.get("wear").and_then(|w| {
                        let on = match w.get("on")?.as_str()? {
                            "break" => WearOn::Break,
                            "hit" => WearOn::Hit,
                            "proc" => WearOn::Proc,
                            _ => return None,
                        };
                        let max = w.get("max")?.as_f64()? as u32;
                        Some(Wear {
                            on,
                            max: max.max(100),
                        })
                    }),
                })
            })
            .collect();
        (!fits.is_empty()).then_some(fits)
    })
}

/// The reverse index a tool's RECORD is read through: installed IDENTITY →
/// its fits (one per tool KIND — several kinds may share one identity when
/// the art is kind-agnostic, like a handle inlay), and identity → the
/// MATERIAL it is applied from (the panel's grayed socket icon).
///
/// Both AMBIGUITIES a fit list can carry are logged here, because neither is
/// a load error and both are otherwise invisible: a material offering one
/// tool kind two different augments (only the first can ever be staged), and
/// one identity claiming a kind twice.
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

/// Each identity's display name — the socket tooltip's first line, resolved
/// from the art item's row once at init.
pub(super) fn display_names(by_identity: &HashMap<String, Vec<Fit>>) -> HashMap<String, String> {
    by_identity
        .keys()
        .filter_map(|id| Some((id.clone(), item_info(id)?.display_name)))
        .collect()
}

/// Every augmentable tool: its socket row + family, and the engine's own
/// resolved stats, so the mod never restates the tier ladder.
pub(super) fn augmentable_tools() -> HashMap<String, (ToolSlots, ToolStats)> {
    let slotted = read_rows("forge:augment_slots", |value| {
        Some(ToolSlots {
            family: value
                .get("family")
                .and_then(|f| f.as_str())
                .unwrap_or("default")
                .to_owned(),
            lockable: value
                .get("lockable")
                .and_then(|l| l.as_u8())
                .unwrap_or(0)
                .min(SOCKETS as u8 - BASE_SOCKETS),
        })
    });
    slotted
        .into_iter()
        .filter_map(|(name, slots)| {
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

/// The items carrying `key` at all — membership is the whole vocabulary for
/// the socket gem and the innately gentle tools.
pub(super) fn items_with(key: &str) -> HashSet<String> {
    read_rows(key, |_| Some(())).into_keys().collect()
}
