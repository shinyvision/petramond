//! The anvil: tool AUGMENTS — smithing, not casting.
//!
//! You put the tool in, you put the RAW MATERIAL in — diamonds — and the
//! anvil fits the augment. There is no intermediate "diamond tip" craftable
//! anywhere: the tip exists only as the augment's overlay art and its record
//! on the tool. That is the pack's own economy rule again (the mould, the
//! head and the tool are one object at three stages): the diamond, the
//! anchor on the sprite, and the augmented tool are one object at three
//! stages, and no craftable middle step can leak out of it.
//!
//! An augmented tool is INSTANCE DATA on the ordinary tool stack, never a new
//! item row: the anvil stamps three keys and the engine does the rest. Each
//! value is an ordered comma-list, so one tool carries any number of augments
//! (one per slot type its row declares):
//!
//! - `forge:augments` — this pack's own record of what is installed: the
//!   augments' IDENTITY names (each fit's canonical overlay item, e.g.
//!   `forge:diamond_tip`), in application order. Occupancy checks, the
//!   recompute below, and future removal read this; the engine never does.
//! - `petramond:tool` — the ENGINE's per-stack tool override, stated as
//!   ABSOLUTES: the harvest gate goes to the material at the cutting edge
//!   (max tier across base and every augment), speed and damage are the base
//!   tool's resolved values times every augment's multipliers. Stamping
//!   absolutes keeps the engine's read a dumb merge and keeps stacking
//!   policy entirely here. Every apply RECOMPUTES the whole override from
//!   the base row plus the full `forge:augments` record — never
//!   incrementally from the previous stamp — so results are independent of
//!   how the set was assembled and a rebalanced ladder re-stamps correctly
//!   on the next anvil visit.
//! - `petramond:overlay` — the overlay ART item names, composited in order
//!   over the tool's sprite at every render site; art is authored IN
//!   POSITION on its own transparent 16×16 and no coordinate crosses any
//!   boundary. Recomputed with the rest, so renamed or re-familied art heals
//!   on the next apply.
//!
//! WHICH TOOLS TAKE WHICH AUGMENTS IS DATA, twice over. A tool item carries
//! `forge:augment_slots` (patch rows on the engine tools) declaring its slot
//! types (the `at` sprite anchors in those rows are reserved for per-slot
//! presentation once tools grow more than one slot). An augment MATERIAL
//! carries `forge:augment`: a LIST of fits (tool kind, slot type, edge tier,
//! multipliers, material cost, overlay item), one per tool kind it augments.
//! Another pack extends either side with rows alone.
//!
//! APPLICATION IS STAGED, not automatic: a valid pair publishes a result
//! PREVIEW (the stat deltas the fit would stamp), and only the panel's
//! Augment button — a widget click recorded by [`AnvilSpec::request_apply`]
//! and honoured on the next step, when the driver hands this machine its
//! slots — consumes the material and transforms the tool.
//!
//! The base tool's resolved stats come from `item_info` (the engine answers
//! the tier ladder's derived speed/damage), so this module never restates the
//! engine's default ladder — the duplicated-constants trap.

use std::collections::{HashMap, HashSet};

use mod_sdk::*;

use machine_core::{write_changed_slots, Caches, Machine, MachineSpec, Presentation, StepCtx};

use crate::content::read_rows;

const STATE_KEY: &str = "forge:anvil_state";

const SLOT_TOOL: usize = 0;
const SLOT_AUGMENT: usize = 1;
const SLOTS: usize = 2;

/// This pack's own record of the installed augments (a comma-list of
/// identity names, in application order). Shared with the gold policy
/// (`gold.rs`), which reads it off a held stack to decide the augmented slip
/// chance.
pub(crate) const AUGMENTS_KEY: &str = "forge:augments";

/// The engine's per-value instance-data cap, mirrored here because the ABI
/// does not expose it: an over-cap value degrades the whole stack to plain
/// data, silently shedding every augment, so the anvil REFUSES an apply
/// whose record would cross it (the button just stays disabled — reachable
/// only far past the shipped slot counts).
const VALUE_CAP: usize = 64;

/// The panel's Augment button (`pack/ui/documents/anvil.gui.json`).
pub const WIDGET_AUGMENT: &str = "augment";

/// One fit out of an augment material's `forge:augment` list.
#[derive(Clone)]
struct Fit {
    /// The tool KIND it fits (`"pickaxe"`, matched against the base tool's).
    tool: String,
    /// The slot type it fills (`"tip"`, `"blade"`).
    slot: String,
    /// The harvest gate the augment's material reaches — the edge's tier.
    tier: u8,
    speed_mult: f32,
    damage_mult: f32,
    /// How many of the material one application consumes.
    cost: u8,
    /// The item whose sprite becomes the tool's overlay AND the recorded
    /// augment identity (the default-family art).
    overlay: String,
    /// Per-silhouette-family art overrides (`"stone"` → the stone-family
    /// overlay item). Tool sprites differ per material family, and overlay
    /// art is authored IN POSITION, so one drawing cannot hug two contours.
    overlays: Vec<(String, String)>,
}

impl Fit {
    /// The overlay ART item for a tool of `family` (the identity recorded on
    /// the stack stays [`Fit::overlay`] regardless).
    fn overlay_for(&self, family: &str) -> &str {
        self.overlays
            .iter()
            .find(|(f, _)| f == family)
            .map(|(_, o)| o.as_str())
            .unwrap_or(&self.overlay)
    }
}

/// A tool's whole `forge:augment_slots` row: its slot type names plus the
/// silhouette FAMILY its sprite belongs to (`"stone"`; absent = the default
/// family — iron, whose silhouette copper and gold share by derivation). The
/// family picks which of a fit's overlay drawings hugs this tool's contour.
/// (The rows' per-slot `at` sprite anchors are not read here any more — the
/// augment slot sits in a fixed panel row since the 2026-08-05 redesign.)
struct ToolSlots {
    family: String,
    slots: Vec<String>,
}

/// A base tool's engine-resolved stats (`item_info`, cached at init).
struct ToolStats {
    kind: String,
    tier: u8,
    speed: f32,
    damage: [f32; 2],
}

pub type Anvil = Machine<AnvilSpec>;

#[derive(Default)]
pub struct AnvilSpec {
    /// augment material item name -> its fits.
    augments: HashMap<String, Vec<Fit>>,
    /// installed-augment IDENTITY name -> its fit, for reading a tool's
    /// `forge:augments` record back into fits (occupancy + recompute). An
    /// identity names exactly ONE fit — packs mint one overlay item per fit.
    by_identity: HashMap<String, Fit>,
    /// augmentable tool item name -> (its slots + family, resolved stats).
    tools: HashMap<String, (ToolSlots, ToolStats)>,
    /// Anchors whose panel's Augment button was clicked, honoured (and
    /// cleared) on that machine's next step.
    pending: HashSet<[i32; 3]>,
}

impl AnvilSpec {
    /// The panel's Augment button was clicked at `pos`; apply on the next
    /// step, where the driver hands this machine its slots.
    pub fn request_apply(&mut self, pos: [i32; 3]) {
        self.pending.insert(pos);
    }
}

impl MachineSpec for AnvilSpec {
    const KIND_KEY: &'static str = "forge:anvil";
    const BLOCK_KEY: &'static str = "forge:anvil";
    const VARIANT_KEYS: &'static [&'static str] = &[];
    const ANCHORS_KEY: &'static str = "forge:anvils";
    const STATE_KEY: &'static str = STATE_KEY;

    fn init(&mut self) {
        self.augments = read_rows("forge:augment", |value| {
            let fits: Vec<Fit> = value
                .as_array()?
                .iter()
                .filter_map(|f| {
                    Some(Fit {
                        tool: f.get("tool")?.as_str()?.to_owned(),
                        slot: f.get("slot")?.as_str()?.to_owned(),
                        tier: f.get("tier").and_then(|t| t.as_u8()).unwrap_or(0),
                        speed_mult: f
                            .get("speed_mult")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32,
                        damage_mult: f
                            .get("damage_mult")
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32,
                        cost: f.get("cost").and_then(|v| v.as_u8()).unwrap_or(1).max(1),
                        overlay: f.get("overlay")?.as_str()?.to_owned(),
                        overlays: f
                            .get("overlays")
                            .and_then(|o| o.as_object())
                            .map(|kv| {
                                kv.iter()
                                    .filter_map(|(k, v)| {
                                        Some((k.clone(), v.as_str()?.to_owned()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect();
            (!fits.is_empty()).then_some(fits)
        });
        self.by_identity = HashMap::new();
        for fit in self.augments.values().flatten() {
            if self
                .by_identity
                .insert(fit.overlay.clone(), fit.clone())
                .is_some()
            {
                log(&format!(
                    "forge: augment identity '{}' names more than one fit",
                    fit.overlay
                ));
            }
        }
        let slotted = read_rows("forge:augment_slots", |value| {
            let slots: Vec<String> = value
                .get("slots")?
                .as_array()?
                .iter()
                .filter_map(|s| Some(s.get("slot")?.as_str()?.to_owned()))
                .collect();
            (!slots.is_empty()).then_some(ToolSlots {
                family: value
                    .get("family")
                    .and_then(|f| f.as_str())
                    .unwrap_or("default")
                    .to_owned(),
                slots,
            })
        });
        self.tools = slotted
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
                        },
                    ),
                ))
            })
            .collect();
        // The member counts are the only cheap signal the pack's data keys and
        // this module's constants are the same strings.
        log(&format!(
            "forge: {} augment materials, {} augmentable tools",
            self.augments.len(),
            self.tools.len()
        ));
    }

    fn step(
        &mut self,
        ctx: &StepCtx<'_>,
        _caches: &mut Caches,
        slots: Option<Vec<Option<ItemStackData>>>,
        _stored: &mut Vec<u8>,
        _out: &mut Presentation,
    ) {
        let mut slots = slots.unwrap_or_default();
        slots.resize(SLOTS, None);
        let before = slots.clone();

        // Apply only on a recorded Augment click, and only when the pair in
        // the slots is valid AND affordable: the material is consumed and the
        // tool transforms in place. A click over a misfit (wrong kind,
        // occupied tool, too few) clears the request and changes nothing.
        if self.pending.remove(&ctx.pos) {
            if let Some((fitted, cost)) = self.fitted_tool(&slots) {
                slots[SLOT_TOOL] = Some(fitted);
                take(&mut slots[SLOT_AUGMENT], cost);
            }
        }
        write_changed_slots(ctx.pos, &before, &slots);

        if ctx.gui_open() {
            self.publish_stage(ctx, &slots);
        }
    }

    fn forget(&mut self, pos: [i32; 3]) {
        self.pending.remove(&pos);
    }
}

/// Remove `n` items from a slot (empties it at zero).
fn take(slot: &mut Option<ItemStackData>, n: u8) {
    if let Some(s) = slot {
        s.count = s.count.saturating_sub(n);
        if s.count == 0 {
            *slot = None;
        }
    }
}

impl AnvilSpec {
    /// The fits `tool`'s `forge:augments` record names, resolved through the
    /// identity table, in application order (empty for a bare tool). `None`
    /// when any recorded identity is unknown: a record this build cannot
    /// reason about takes no further augments — refusing is the only answer
    /// that cannot corrupt a stack from a richer pack set.
    fn installed_fits(&self, tool: &ItemStackData) -> Option<Vec<&Fit>> {
        let Some((_, v)) = tool.data.iter().find(|(k, _)| k == AUGMENTS_KEY) else {
            return Some(Vec::new());
        };
        let names = std::str::from_utf8(v).ok()?;
        names
            .split(',')
            .map(|name| self.by_identity.get(name.trim()))
            .collect()
    }

    /// The fit the current pair would apply, before the cost gate:
    /// `(fit, tool row, base stats, material count in the slot)`. One
    /// augment per SLOT TYPE: a fit whose slot the record already fills is
    /// no fit.
    fn pair_fit(
        &self,
        slots: &[Option<ItemStackData>],
    ) -> Option<(&Fit, &ToolSlots, &ToolStats, u8)> {
        let tool = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0)?;
        let augment = slots[SLOT_AUGMENT].as_ref().filter(|s| s.count > 0)?;
        let (tool_slots, stats) = self.tools.get(&tool.item)?;
        let occupied: HashSet<&str> = self
            .installed_fits(tool)?
            .iter()
            .map(|f| f.slot.as_str())
            .collect();
        let fit = self.augments.get(&augment.item)?.iter().find(|f| {
            f.tool == stats.kind
                && tool_slots.slots.contains(&f.slot)
                && !occupied.contains(f.slot.as_str())
        })?;
        Some((fit, tool_slots, stats, augment.count))
    }

    /// The tool slot's stack with the augment fitted plus the material cost,
    /// when the pair is valid AND affordable — `None` leaves both slots
    /// exactly as they are.
    fn fitted_tool(&self, slots: &[Option<ItemStackData>]) -> Option<(ItemStackData, u8)> {
        let (fit, tool_slots, stats, have) = self.pair_fit(slots)?;
        if have < fit.cost {
            return None;
        }
        let tool = slots[SLOT_TOOL].as_ref()?;
        let mut fits = self.installed_fits(tool)?;
        fits.push(fit);

        // Recompute the WHOLE override from the base row plus the full
        // record, never incrementally from the previous stamp: the gate goes
        // to the highest edge, speed and damage are the base's resolved
        // values times every augment's multipliers.
        let tier = fits.iter().fold(stats.tier, |t, f| t.max(f.tier));
        let speed = fits.iter().fold(stats.speed, |s, f| s * f.speed_mult);
        let damage = fits.iter().fold(stats.damage, |d, f| {
            [d[0] * f.damage_mult, d[1] * f.damage_mult]
        });

        // The IDENTITIES recorded are the fits' canonical overlays; the ART
        // drawn is the family-resolved list, so a stone pickaxe's tip hugs
        // the stone silhouette while both record "forge:diamond_tip".
        let identities = fits
            .iter()
            .map(|f| f.overlay.as_str())
            .collect::<Vec<_>>()
            .join(",");
        let arts = fits
            .iter()
            .map(|f| f.overlay_for(&tool_slots.family))
            .collect::<Vec<_>>()
            .join(",");
        if identities.len() > VALUE_CAP || arts.len() > VALUE_CAP {
            return None;
        }
        let mut data = tool.data.clone();
        data.retain(|(k, _)| k != AUGMENTS_KEY && k != TOOL_OVERRIDE_KEY && k != OVERLAY_DATA_KEY);
        data.push((AUGMENTS_KEY.to_owned(), identities.into_bytes()));
        data.push((
            TOOL_OVERRIDE_KEY.to_owned(),
            tool_override_json(tier, speed, damage).into_bytes(),
        ));
        data.push((OVERLAY_DATA_KEY.to_owned(), arts.into_bytes()));
        data.sort_by(|a, b| a.0.cmp(&b.0));
        Some((
            ItemStackData {
                data,
                ..tool.clone()
            },
            fit.cost,
        ))
    }

    /// Publish the panel: the enlarged tool (a composited LAYER LIST on one
    /// item-view hook), the stage hint, the result PREVIEW (stat delta
    /// lines) for the fit the current pair would apply, and the Augment
    /// button's enabled state. Published every open tick so the panel can
    /// never show a stale tool — values persist in the state map until
    /// overwritten, and an empty string both blanks and HIDES its label
    /// (`visible` binds the same key).
    fn publish_stage(&self, ctx: &StepCtx<'_>, slots: &[Option<ItemStackData>]) {
        let tool = slots[SLOT_TOOL].as_ref().filter(|s| s.count > 0);
        let tool_name = tool.map(|s| s.item.as_str()).unwrap_or("");
        let pair = self.pair_fit(slots);

        // The stage previews the RESULT: the tool's sprite, then exactly
        // what the engine composites in the world (the stack's own stamped
        // art list), then the overlay the current pair WOULD apply — so
        // inserting a diamond shows the augmented tool before the player
        // commits. A prospective fit never collides with an installed one:
        // `pair_fit` only fits FREE slots.
        let mut layers = vec![tool_name];
        let applied = tool
            .and_then(|s| s.data.iter().find(|(k, _)| k == OVERLAY_DATA_KEY))
            .and_then(|(_, v)| std::str::from_utf8(v).ok())
            .unwrap_or("");
        if !applied.is_empty() {
            layers.push(applied);
        }
        if let Some((fit, tool_slots, _, _)) = &pair {
            layers.push(fit.overlay_for(&tool_slots.family));
        }

        // The one state the player cannot see from the slots themselves: a
        // valid material that is short of its cost.
        let hint = if tool.is_none() {
            "Insert a tool".to_owned()
        } else {
            match &pair {
                Some((fit, _, _, have)) if *have < fit.cost => format!("Needs {}", fit.cost),
                _ => String::new(),
            }
        };

        // The preview shows for any valid FIT, affordable or not — the hint
        // covers the shortfall; only the button gates on the cost.
        let (p_speed, p_damage) = match &pair {
            Some((fit, _, _, _)) => (
                delta_line("Speed", fit.speed_mult),
                delta_line("Damage", fit.damage_mult),
            ),
            None => Default::default(),
        };

        let can_augment = self.fitted_tool(slots).is_some();
        ctx.publish("forge:tool_view", GuiValue::Str(layers.join(",")));
        ctx.publish("forge:anvil_hint", GuiValue::Str(hint));
        ctx.publish("forge:preview_speed", GuiValue::Str(p_speed));
        ctx.publish("forge:preview_damage", GuiValue::Str(p_damage));
        ctx.publish("forge:can_augment", GuiValue::I32(can_augment as i32));
    }
}

/// `"Speed +50%"` from a multiplier — empty (which hides the label) at 1.0.
fn delta_line(label: &str, mult: f32) -> String {
    let delta = ((mult - 1.0) * 100.0).round() as i32;
    if delta == 0 {
        String::new()
    } else {
        format!("{label} {delta:+}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> AnvilSpec {
        let mut s = AnvilSpec::default();
        s.augments.insert(
            "petramond:diamond".into(),
            vec![
                Fit {
                    tool: "pickaxe".into(),
                    slot: "tip".into(),
                    tier: 4,
                    speed_mult: 1.5,
                    damage_mult: 2.0,
                    cost: 2,
                    overlay: "forge:diamond_tip".into(),
                    overlays: vec![("stone".into(), "forge:diamond_tip_stone".into())],
                },
                Fit {
                    tool: "pickaxe".into(),
                    slot: "blade".into(),
                    tier: 4,
                    speed_mult: 2.0,
                    damage_mult: 2.0,
                    cost: 1,
                    overlay: "forge:diamond_blade".into(),
                    overlays: Vec::new(),
                },
                Fit {
                    tool: "shovel".into(),
                    slot: "blade".into(),
                    tier: 4,
                    speed_mult: 1.5,
                    damage_mult: 2.0,
                    cost: 1,
                    overlay: "forge:diamond_shovel_blade".into(),
                    overlays: Vec::new(),
                },
            ],
        );
        s.by_identity = s
            .augments
            .values()
            .flatten()
            .map(|f| (f.overlay.clone(), f.clone()))
            .collect();
        s.tools.insert(
            "petramond:stone_pickaxe".into(),
            (
                ToolSlots {
                    family: "stone".into(),
                    slots: vec!["tip".into()],
                },
                ToolStats {
                    kind: "pickaxe".into(),
                    tier: 2,
                    speed: 4.0,
                    damage: [1.0, 2.5],
                },
            ),
        );
        s
    }

    fn stack(item: &str, count: u8) -> Option<ItemStackData> {
        Some(ItemStackData {
            item: item.into(),
            count,
            data: Vec::new(),
        })
    }

    /// The three stamped keys and their arithmetic: the gate goes to the
    /// edge, speed/damage multiply the base's RESOLVED values, the overlay
    /// names the fit's overlay item, and the fit's cost comes back for the
    /// consume.
    #[test]
    fn a_valid_pair_stamps_the_three_keys_with_the_edge_tier() {
        let s = spec();
        let slots = vec![
            stack("petramond:stone_pickaxe", 1),
            stack("petramond:diamond", 3),
        ];
        let (fitted, cost) = s.fitted_tool(&slots).expect("the pair fits");
        assert_eq!(cost, 2, "the pickaxe fit costs two diamonds");
        let get = |key: &str| {
            fitted
                .data
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| String::from_utf8(v.clone()).unwrap())
        };
        // The recorded identity is the canonical overlay; the ART is the
        // stone-family drawing, because the stone pickaxe's silhouette is not
        // the iron family's.
        assert_eq!(get(AUGMENTS_KEY).as_deref(), Some("forge:diamond_tip"));
        assert_eq!(
            get(OVERLAY_DATA_KEY).as_deref(),
            Some("forge:diamond_tip_stone")
        );
        assert_eq!(
            get(TOOL_OVERRIDE_KEY).as_deref(),
            Some(r#"{"tier":4,"speed":6.0000,"damage":[2.0000,5.0000]}"#)
        );
        // Keys are sorted — the canonical order the ABI ingest expects.
        let keys: Vec<&String> = fitted.data.iter().map(|(k, _)| k).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    /// The refusals: a wrong-kind material, an occupied tool, too little
    /// material, and a bare slot all leave both slots exactly as they are.
    #[test]
    fn a_pair_that_does_not_fit_is_left_alone() {
        let s = spec();
        // The material fits pickaxes/shovels, but the tool is not augmentable
        // (no forge:augment_slots row resolved for it).
        let slots = vec![stack("petramond:stone_axe", 1), stack("petramond:diamond", 9)];
        assert!(s.fitted_tool(&slots).is_none());
        // Too little material: one diamond against a cost of two.
        let slots = vec![
            stack("petramond:stone_pickaxe", 1),
            stack("petramond:diamond", 1),
        ];
        assert!(s.fitted_tool(&slots).is_none(), "cost gates the apply");
        // An installed tip fills the pickaxe's only slot type: no second fit.
        let mut occupied = stack("petramond:stone_pickaxe", 1).unwrap();
        occupied
            .data
            .push((AUGMENTS_KEY.into(), b"forge:diamond_tip".to_vec()));
        let slots = vec![Some(occupied), stack("petramond:diamond", 9)];
        assert!(s.fitted_tool(&slots).is_none());
        // A record naming an identity this build does not know (a richer
        // pack set wrote it) refuses further augments rather than guessing.
        let mut foreign = stack("petramond:stone_pickaxe", 1).unwrap();
        foreign
            .data
            .push((AUGMENTS_KEY.into(), b"gone:mod_augment".to_vec()));
        let slots = vec![Some(foreign), stack("petramond:diamond", 9)];
        assert!(s.fitted_tool(&slots).is_none(), "unknown identity refused");
        // No material inserted.
        let slots = vec![stack("petramond:stone_pickaxe", 1), None];
        assert!(s.fitted_tool(&slots).is_none());
    }

    /// The multi-augment invariant: a second augment lands in its own FREE
    /// slot type, and the stamped override is RECOMPUTED from the base row
    /// plus the full record — a stale (even nonsensical) previous stamp on
    /// the stack has no bearing on the result.
    #[test]
    fn a_second_augment_recomputes_from_the_base_and_the_full_record() {
        let mut s = spec();
        s.tools
            .get_mut("petramond:stone_pickaxe")
            .unwrap()
            .0
            .slots
            .push("blade".into());
        let mut tipped = stack("petramond:stone_pickaxe", 1).unwrap();
        tipped
            .data
            .push((AUGMENTS_KEY.into(), b"forge:diamond_tip".to_vec()));
        // A stale stamp from a rebalanced past — recompute must ignore it.
        tipped.data.push((
            TOOL_OVERRIDE_KEY.into(),
            br#"{"tier":9,"speed":99.0,"damage":[9.0,9.0]}"#.to_vec(),
        ));
        let slots = vec![Some(tipped), stack("petramond:diamond", 1)];
        let (fitted, cost) = s.fitted_tool(&slots).expect("the blade slot is free");
        assert_eq!(cost, 1, "the blade fit's own cost, not the tip's");
        let get = |key: &str| {
            fitted
                .data
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| String::from_utf8(v.clone()).unwrap())
        };
        assert_eq!(
            get(AUGMENTS_KEY).as_deref(),
            Some("forge:diamond_tip,forge:diamond_blade"),
            "identities accumulate in application order"
        );
        assert_eq!(
            get(OVERLAY_DATA_KEY).as_deref(),
            Some("forge:diamond_tip_stone,forge:diamond_blade"),
            "art recomputed per family for the whole record"
        );
        // Base 4.0 speed × 1.5 (tip) × 2.0 (blade); damage [1.0,2.5] × 2 × 2.
        assert_eq!(
            get(TOOL_OVERRIDE_KEY).as_deref(),
            Some(r#"{"tier":4,"speed":12.0000,"damage":[4.0000,10.0000]}"#)
        );
    }
}
