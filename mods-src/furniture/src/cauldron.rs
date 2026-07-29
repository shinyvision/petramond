//! The cauldron: the mod's second custom shape and the dyeing it hosts.
//!
//! Unoriented rows whose bakes all return [`CAULDRON_BOXES`] — a hollow
//! slate pot the mesher carves from the row's `[top,bottom,side]` tiles.
//! FILL STATE is block identity, exactly like the chain's axis rows:
//! `furniture:cauldron_water` shares the shape kind, and only the RENDER
//! bake adds [`WATER_SURFACE`] for it — a collisionless sheet whose top sits
//! 1 px below the lip (the sim bake stays the empty pot, so a body stands
//! THROUGH the surface). Filling is an act-based `interact_attempt`
//! consumer: a held water bucket on the empty pot swaps block + bucket, an
//! empty wooden bucket on the water pot scoops the water back out (the
//! trough's symmetric swap); a water bucket on a pot holding water or dye is
//! ABSORBED (else the engine pour ray dumps a source over the pot); anything
//! else falls through. Both sides classify through ONE pure
//! [`Furniture::cauldron_action`], and every classification input is
//! replica-visible, so the client predictor is exact.
//!
//! DYEING: a `furniture:pigment`-declaring item on a water or dye pot stirs
//! its color in — water takes the pigment straight (`furniture:cauldron_dye`
//! row), an existing dye mixes SUBTRACTIVELY ([`mix_dye`], Beer–Lambert:
//! stains multiply transmittance so pigment ACCUMULATES — blue + yellows →
//! green → black — and the white flowers DILUTE, halving absorbance back
//! toward white). The color is per-cell KV under [`DYE_KEY`] — written
//! server-side, replicated by the engine's cell-KV delta lane, and handed
//! back to the RENDER bake as the shape's `state_key` input to tint
//! [`WATER_SURFACE`] (the sim bake never reads it — purity holds). The dye
//! top tile is deliberately desaturated/bright so the multiply tint carries
//! the color.

use std::collections::HashMap;

use mod_sdk::*;

use super::{held_places_a_block, Furniture};

/// Helper for the box tables: a [`ShapeAabb`] from authored 16ths.
const fn px(min: [f32; 3], max: [f32; 3]) -> ShapeAabb {
    ShapeAabb {
        min: [min[0] / 16.0, min[1] / 16.0, min[2] / 16.0],
        max: [max[0] / 16.0, max[1] / 16.0, max[2] / 16.0],
    }
}

/// The cauldron: a hollow single-cell pot built from overlapping cuboids so
/// the silhouette reads rounded — a pinwheel wall ring (butted contact faces
/// stay fully buried), belly plates protruding past the walls with chamfered
/// corners, and a slim overhanging LIP above a 2-px neck. The lip is a
/// 2×2 cross-section ring (y 14..16, 2 thick) whose plan-view corners are
/// cut 1×1 — the four side boxes each stop 1 short of the outline corners
/// (they overlap in the corners instead of butting; the emitter's
/// coincident-face tie-break draws each shared plane once) — so the rim
/// reads rounded like the belly. The BOTTOM mirrors the same graduation:
/// belly down to y 2, walls to y 1, and the inset floor slab (2 from every
/// side) at y 0 — a rounded foot on all four corners. Cavity is x/z 3..13
/// from y 3 up, open at the top, with a 1-px wall-top shelf visible inside
/// the lip. ONE geometry source for the sim, render, and item bakes, like
/// the chain's plates. Fluid states are sibling rows sharing this shape kind
/// (block id = state, the ladder-row pattern) with their own tiles; the bake
/// branches on the cell's block id.
pub(super) const CAULDRON_BOXES: [ShapeAabb; 13] = [
    px([2.0, 0.0, 2.0], [14.0, 3.0, 14.0]),   // floor slab (cavity floor + inset foot)
    px([1.0, 1.0, 1.0], [13.0, 14.0, 3.0]),   // wall ring, pinwheel
    px([13.0, 1.0, 1.0], [15.0, 14.0, 13.0]),
    px([3.0, 1.0, 13.0], [15.0, 14.0, 15.0]),
    px([1.0, 1.0, 3.0], [3.0, 14.0, 15.0]),
    px([2.0, 2.0, 0.5], [14.0, 12.0, 2.0]),   // belly plates (rounded bulge)
    px([2.0, 2.0, 14.0], [14.0, 12.0, 15.5]),
    px([0.5, 2.0, 2.0], [2.0, 12.0, 14.0]),
    px([14.0, 2.0, 2.0], [15.5, 12.0, 14.0]),
    px([1.0, 14.0, 0.0], [15.0, 16.0, 2.0]),  // lip ring, corners cut 1×1
    px([1.0, 14.0, 14.0], [15.0, 16.0, 16.0]),
    px([0.0, 14.0, 1.0], [2.0, 16.0, 15.0]),
    px([14.0, 14.0, 1.0], [16.0, 16.0, 15.0]),
];

/// The water surface for the filled cauldron row — RENDER-ONLY (never in the
/// sim bake, so it has no collision). A 1-px sheet spanning the lip's inner
/// opening (2..14): its top plane sits at y 15, 1 px below the lip's top;
/// its sides are buried in the lip ring's inner faces and its underside rim
/// on the wall tops, so only the surface (and the enclosed cavity ceiling)
/// ever draws. The water row's top tile paints this whole 2..14 window as
/// water, covering the wall-top shelf.
const WATER_SURFACE: ShapeAabb = px([2.0, 14.0, 2.0], [14.0, 15.0, 14.0]);

/// AO strength (percent) on the fluid surface: the pot's lip would pocket
/// the plane's whole rim into shadow at full strength, but a liquid surface
/// reads flat and bright — keep only a hint of the corner darkening.
const WATER_AO: u8 = 30;

/// The cauldron family: the shared shape kind and its fill-state rows
/// (block id = fill state, the chain-row pattern). The DYE row's color is
/// per-cell KV under [`DYE_KEY`] — continuous state that cannot be block
/// identity.
pub(super) struct Cauldron {
    pub(super) shape: u8,
    pub(super) empty: BlockId,
    pub(super) water: BlockId,
    pub(super) dye: BlockId,
}

/// Cell KV key holding a dye cauldron's color: 3 raw bytes `[r, g, b]`.
/// Written server-side beside the block flip; replicated to every client by
/// the engine's cell-KV delta lane, where the render bake reads it back
/// (the shape's `state_key` input) to tint [`WATER_SURFACE`]. Dies with the
/// block.
const DYE_KEY: &str = "furniture:dye";

/// Cell KV key holding a dye pot's REMAINING USES: 1 raw byte, written beside
/// [`DYE_KEY`] on the first fill (absent = a stale pre-uses pot, read as
/// full). One wool dip = one use; at zero the pot empties (block flip, both
/// keys die with it). Replicated like [`DYE_KEY`], so the render bake lowers
/// the dye surface as the pot drains.
const USES_KEY: &str = "furniture:dye_uses";

/// A full pot dyes this many times.
const DYE_USES: u8 = 8;

/// Item-data key marking an item as pot-dyeable (`furniture:dyeable`,
/// value ignored — presence is the declaration). Like pigments, this is
/// DATA-SURFACE INTEROP: this pack patches the engine wool block/stairs/slab
/// items, and any pack can opt its own items in the same way. The dyed
/// give-back needs the item's registry NAME, resolved once at init.
const DYEABLE_KEY: &str = "furniture:dyeable";

/// Load every declared dyeable off the item-data surface, with its registry
/// name (the `give_item_data` vocabulary).
pub(super) fn load_dyeables() -> Vec<(ItemId, String)> {
    let ids: Vec<ItemId> = items_with_data(DYEABLE_KEY)
        .into_iter()
        .map(|(item, _)| item)
        .collect();
    let names = item_names(ids.clone());
    ids.into_iter()
        .zip(names)
        .filter_map(|(id, name)| Some((id, name?)))
        .collect()
}

/// One wool dip converts at most this many blocks from the held stack.
const WOOL_DIP_MAX: u8 = 32;

/// The ENGINE-consumed presentation key a dyed wool stack carries: the
/// renderer multiply-tints anything whose instance data (or cell KV) holds
/// it. The furniture pack's patch rows opt the engine wool rows into carrying
/// it across break/place (`petramond:carry`) and crafting
/// (`petramond:inherit`); this mod only ever WRITES the value.
const TINT_KEY: &str = "petramond:tint";

/// The dye surface for a draining pot: the full-pot sheet is [`WATER_SURFACE`]
/// (top at y15); each spent use sinks the sheet 1 px, bottoming out just
/// above the basin floor.
fn dye_surface(uses: u8) -> ShapeAabb {
    let top = 15.0 - (DYE_USES.saturating_sub(uses)) as f32;
    px([2.0, top - 1.0, 2.0], [14.0, top, 14.0])
}

/// Pigments are DATA-SURFACE INTEROP, not a compiled table: any item whose
/// row carries a `furniture:pigment` data entry —
/// `{"color": [r, g, b], "dilute": true?}` — is a pigment, whoever ships it.
/// This pack attaches the entry to the seven engine flowers via catalog
/// `{"patch", "data"}` rows in its own `items.json`; a berries pack does the
/// same on its rows with no furniture involvement. Loaded once at init
/// ([`load_pigments`] — registry-only calls, so the CLIENT predictor builds
/// the same set and stays exact); a malformed value is skipped with a log
/// line. Stain colors should be deliberately SATURATED (high pass channels,
/// low stop channels): under accumulation the stop channels do the mixing
/// work, and a half-high pass channel darkens every brew it touches.
///
/// Mixing is genuinely SUBTRACTIVE ([`mix_dye`], Beer–Lambert): the pot's
/// RGB is a per-channel TRANSMITTANCE, and a STAIN flower ADDS half a layer
/// of its absorbance on top — transmittances multiply, they never average —
/// so pigment ACCUMULATES: a blue pot plus yellow flowers turns GREEN (the
/// blue's red-absorption stays in the pot forever while the yellow kills
/// the blue channel), and stirring everything in keeps absorbing more light
/// until the pot sits at (near) BLACK. A DILUTANT flower is the inverse:
/// it thins the brew, HALVING the pot's absorbance per flower (plus its own
/// faint half-layer), so whites brighten any dye — a near-black pot
/// included — and repeated pure-white daisies converge to the mixing grid's
/// white, `[248; 3]` (see [`mix_dye`]'s snap).
const PIGMENT_KEY: &str = "furniture:pigment";

/// Load every declared pigment off the item-data surface.
pub(super) fn load_pigments() -> Vec<(ItemId, [u8; 3], bool)> {
    items_with_data(PIGMENT_KEY)
        .into_iter()
        .filter_map(|(item, text)| {
            let Some((color, dilute)) = parse_pigment(&text) else {
                log(&format!("ignoring malformed {PIGMENT_KEY} data: {text}"));
                return None;
            };
            Some((item, color, dilute))
        })
        .collect()
}

/// Parse one `furniture:pigment` value: `{"color": [r, g, b]}` with an
/// optional `"dilute": bool` (default stain).
fn parse_pigment(text: &str) -> Option<([u8; 3], bool)> {
    let v = json::Value::parse(text)?;
    let [r, g, b] = v.get("color")?.as_array()? else {
        return None;
    };
    let color = [r.as_u8()?, g.as_u8()?, b.as_u8()?];
    let dilute = match v.get("dilute") {
        Some(d) => d.as_bool()?,
        None => false,
    };
    Some((color, dilute))
}

/// One flower stirred in, per channel on transmittance `t = value/255`:
///
/// - stain: `t · √t_pigment` — half a layer of pigment LAYERS ON TOP (the
///   pot's own absorption is never washed out; hue accumulates and the brew
///   only darkens), biased down (floor before the snap);
/// - dilutant: `√t · √t_pigment` — the absorbance halves (thinning the
///   brew) plus the flower's own faint half-layer, biased up (ceil before
///   the snap) so brightening genuinely climbs.
///
/// The result then SNAPS to an 8-step per-channel grid clamped to `8..=248`
/// (steps this size are visually indistinguishable), so mixing can only
/// ever mint a BOUNDED palette — at most 31³ ≈ 29.8k distinct colors,
/// comfortably inside the engine's 65,535-row variant table (every distinct
/// tint interns one row). The floor keeps a channel off zero — a zero
/// transmittance is multiplicatively ABSORBING and no amount of white could
/// ever recover it — and the ceiling keeps the grid inside u8, so the fixed
/// points are 8 ("black") and 248 ("white"): dilution converges to
/// `[248; 3]`, never literal 255s. Within half a grid step of an extreme
/// the snap can eat one mix's progress; the grid exists to bound the
/// palette, not to make every single stir visible.
fn mix_dye(pot: [u8; 3], pigment: [u8; 3], dilute: bool) -> [u8; 3] {
    let mut out = [0u8; 3];
    for c in 0..3 {
        let t = f64::from(pot[c]) / 255.0;
        let p = (f64::from(pigment[c]) / 255.0).max(1.0 / 255.0);
        let mixed = if dilute {
            t.max(1.0 / 255.0).sqrt() * p.sqrt()
        } else {
            t * p.sqrt()
        };
        let scaled = mixed * 255.0;
        let rounded = if dilute { scaled.ceil() } else { scaled.floor() };
        let snapped = (rounded / 8.0).round() * 8.0;
        out[c] = snapped.clamp(8.0, 248.0) as u8;
    }
    out
}

/// What a click on a cauldron does — the ONE classification the
/// authoritative consumer and the client predictor share, a pure function of
/// the cell's block, the actor snapshot, and the pot's dye color, so the
/// gate cannot fork.
enum CauldronSwap {
    /// Not a cauldron / no matching held item / sneak-defer: fall through.
    None,
    /// Water bucket on a pot already holding water or dye: claim with no
    /// effect but suppressing the engine pour ray.
    Absorb,
    /// Water bucket on the empty pot: fill.
    Fill,
    /// Empty wooden bucket on the water pot: scoop the water back out. (Dye
    /// is not scoopable — there is no dye bucket; it belongs to the dyeing
    /// follow-ups.)
    Scoop,
    /// A pigment flower on a water or dye pot: stir the pigment in. Carries
    /// the flower's item (the consume), its pigment, and whether it dilutes
    /// (the white flowers) instead of staining.
    Dye(ItemId, [u8; 3], bool),
    /// Wool blocks on a dye pot: dye up to [`WOOL_DIP_MAX`] of the held
    /// stack in the pot's color. Carries the `Furniture::dyeables` index the
    /// classification matched (so the action never re-derives the lookup it
    /// already proved) and the dip count. One dip = one use.
    DyeWool { dyeable: usize, count: u8 },
}

/// Parse a cell's dye-KV bytes: exactly `[r, g, b]`, anything else is not a
/// color (a foreign or truncated value reads as absent — the stale-pot path).
fn parse_dye(v: Vec<u8>) -> Option<[u8; 3]> {
    <[u8; 3]>::try_from(v.as_slice()).ok()
}

fn cell_center(pos: [i32; 3]) -> [f32; 3] {
    [
        pos[0] as f32 + 0.5,
        pos[1] as f32 + 0.5,
        pos[2] as f32 + 0.5,
    ]
}

/// Resolve the cauldron family at init: the shape kind and its fill-state
/// rows. Registry-only, legal on any instance; `None` when the pack content
/// didn't load — the rest of the mod keeps working and the cauldron falls
/// back to a plain shape block with no fill interaction.
pub(super) fn resolve_cauldron() -> Option<Cauldron> {
    Some(Cauldron {
        shape: resolve_shape("furniture:cauldron")?,
        empty: resolve_block("furniture:cauldron")?,
        water: resolve_block("furniture:cauldron_water")?,
        dye: resolve_block("furniture:cauldron_dye")?,
    })
}

impl Furniture {
    /// Classify a cauldron click — shared VERBATIM by the authoritative
    /// consumer and the client predictor. Inputs are the cell's block, the
    /// actor snapshot, and the pot's dye color (each side reads its own KV
    /// view: `section_kv_get` / `client_cell_kv_at` — both replicated), so
    /// prediction is exact. A sneak click holding a placeable item (flowers
    /// are blocks!) defers to the placement consumer — the furniture
    /// sneak-defer rule.
    ///
    /// THE stale-pot story, in one place: a dye-row cell whose color KV is
    /// missing (a pre-dye save) refuses every dye interaction — no recolor,
    /// no wool dip — and falls through unclaimed on BOTH sides; breaking the
    /// pot is the recovery. Refusal here is what keeps the predictor exact
    /// for stale pots too.
    fn cauldron_action(
        &self,
        cauldron: &Cauldron,
        block: BlockId,
        actor: &PlayerSnapshot,
        dye: Option<[u8; 3]>,
    ) -> CauldronSwap {
        let Some(held) = actor.held else {
            return CauldronSwap::None;
        };
        if actor.held_count == 0 {
            // `held` with a zero count never names a spendable stack; refuse
            // up front so no arm can claim (and predict) a spend that the
            // consume would fail.
            return CauldronSwap::None;
        }
        if Some(held) == self.water_bucket {
            if block == cauldron.empty {
                return CauldronSwap::Fill;
            }
            if block == cauldron.water || block == cauldron.dye {
                return CauldronSwap::Absorb;
            }
        }
        if Some(held) == self.wooden_bucket && block == cauldron.water {
            return CauldronSwap::Scoop;
        }
        let stale_dye_pot = block == cauldron.dye && dye.is_none();
        if (block == cauldron.water || block == cauldron.dye) && !stale_dye_pot {
            if let Some(&(_, pigment, dilute)) =
                self.pigments.iter().find(|(id, _, _)| *id == held)
            {
                if actor.sneak && held_places_a_block(Some(held)) {
                    return CauldronSwap::None; // sneak-to-build against the pot
                }
                return CauldronSwap::Dye(held, pigment, dilute);
            }
        }
        if block == cauldron.dye && !stale_dye_pot {
            if let Some(dyeable) = self.dyeables.iter().position(|(id, _)| *id == held) {
                if actor.sneak && held_places_a_block(Some(held)) {
                    return CauldronSwap::None; // sneak-to-build against the pot
                }
                return CauldronSwap::DyeWool {
                    dyeable,
                    count: actor.held_count.min(WOOL_DIP_MAX),
                };
            }
        }
        CauldronSwap::None
    }

    pub(super) fn try_use_cauldron(&self, pos: [i32; 3], actor: &PlayerSnapshot) -> bool {
        let Some(cauldron) = &self.cauldron else {
            return false;
        };
        let Some(block) = get_block(pos) else {
            return false;
        };
        let dye = (block == cauldron.dye)
            .then(|| section_kv_get(pos, DYE_KEY).and_then(parse_dye))
            .flatten();
        match self.cauldron_action(cauldron, block, actor, dye) {
            CauldronSwap::None => false,
            CauldronSwap::Absorb => true, // keep the pour ray off the full pot
            CauldronSwap::Fill => {
                if !replace_held_one(self.water_bucket.unwrap(), "petramond:wooden_bucket") {
                    return false;
                }
                set_block(pos, cauldron.water);
                emit_sound("petramond:water_splash_small", Some(cell_center(pos)));
                true
            }
            CauldronSwap::Scoop => {
                if !replace_held_one(self.wooden_bucket.unwrap(), "petramond:water_bucket") {
                    return false;
                }
                set_block(pos, cauldron.empty);
                emit_sound("petramond:water_splash_small", Some(cell_center(pos)));
                true
            }
            CauldronSwap::Dye(flower, pigment, dilute) => {
                if !consume_held(flower, 1) {
                    return false;
                }
                // Water takes the pigment straight; an existing dye mixes it
                // in. The block flip (water → dye) runs FIRST — a block write
                // wipes the cell's KV on both sides, so the color must land
                // after it (and the engine replicates them in that order).
                let color = if block == cauldron.dye {
                    // The classifier refuses a stale pot, so the color the
                    // classification read is present.
                    let Some(old) = dye else {
                        return false;
                    };
                    mix_dye(old, pigment, dilute)
                } else {
                    set_block(pos, cauldron.dye);
                    pigment
                };
                section_kv_set(pos, DYE_KEY, color.to_vec());
                // A FRESH fill (water → dye) starts at full capacity; stirring
                // a pigment into an existing pot recolors WITHOUT refilling —
                // capacity is the fluid, and no fluid was added.
                if block != cauldron.dye {
                    section_kv_set(pos, USES_KEY, vec![DYE_USES]);
                }
                emit_sound("petramond:water_splash_small", Some(cell_center(pos)));
                true
            }
            CauldronSwap::DyeWool { dyeable, count } => {
                // The classification proved both: the pot's color (stale pots
                // refuse) and the dyeable row the index names.
                let Some(color) = dye else {
                    return false;
                };
                let Some((held_id, held_name)) = self.dyeables.get(dyeable) else {
                    return false;
                };
                if !consume_held(*held_id, count as u32) {
                    return false;
                }
                give_item_data(held_name, count, &[(TINT_KEY, &color)]);
                let uses = section_kv_get(pos, USES_KEY)
                    .and_then(|v| v.first().copied())
                    .unwrap_or(DYE_USES);
                if uses <= 1 {
                    // The pot is spent: the flip to empty wipes both keys.
                    set_block(pos, cauldron.empty);
                } else {
                    section_kv_set(pos, USES_KEY, vec![uses - 1]);
                }
                emit_sound("petramond:water_splash_small", Some(cell_center(pos)));
                true
            }
        }
    }

    /// Batch-read the remaining-uses KV for every dye-pot cell of a render
    /// bake window, KEYED BY CELL so the read can't silently desync from the
    /// per-cell loop (an absent/short value = a stale pot, drawn full).
    /// Client-only — the uses ride the replica's cell KV.
    pub(super) fn cauldron_dye_uses(
        &self,
        shape_kind: u8,
        cells: &[CellInput],
    ) -> HashMap<[i32; 3], u8> {
        let dye_cells: Vec<[i32; 3]> = match &self.cauldron {
            Some(c) if c.shape == shape_kind && self.client => cells
                .iter()
                .filter(|cell| cell.block_id == c.dye)
                .map(|cell| cell.world_pos)
                .collect(),
            _ => Vec::new(),
        };
        if dye_cells.is_empty() {
            return HashMap::new();
        }
        let values = client_cell_kv_at(USES_KEY, dye_cells.clone());
        dye_cells
            .into_iter()
            .zip(values)
            .filter_map(|(pos, v)| Some((pos, *v?.first()?)))
            .collect()
    }

    /// The render-only fluid sheet for a filled cauldron cell — the water
    /// surface, or the dye surface lowered by its spent uses and tinted by
    /// the cell's replicated color. The color rides the bake input itself:
    /// the cauldron shape declares `state_key: "furniture:dye"` in
    /// `shapes.json`, so the engine hands each cell its replicated dye bytes
    /// as `cell.state` — the stateful-shape primitive, no bespoke host call.
    /// `None` for the empty pot and every non-cauldron cell.
    pub(super) fn cauldron_fluid_box(
        &self,
        shape_kind: u8,
        cell: &CellInput,
        uses_at: &HashMap<[i32; 3], u8>,
    ) -> Option<ShapeRenderBox> {
        let c = self.cauldron.as_ref().filter(|c| c.shape == shape_kind)?;
        if cell.block_id == c.water {
            return Some(ShapeRenderBox {
                aabb: WATER_SURFACE,
                tint: None,
                ao: Some(WATER_AO),
                dyed: false,
            });
        }
        if cell.block_id == c.dye {
            let tint = cell
                .state
                .as_ref()
                .filter(|v| v.len() == 3)
                .map(|v| [v[0], v[1], v[2]]);
            let uses = uses_at.get(&cell.world_pos).copied().unwrap_or(DYE_USES);
            return Some(ShapeRenderBox {
                aabb: dye_surface(uses),
                tint,
                // The tint IS a dye color: sample the surface tile's
                // dye-base twin so it can whiten too.
                dyed: tint.is_some(),
                ao: Some(WATER_AO),
            });
        }
        None
    }

    /// CLIENT: gate-only mirror of [`Self::try_use_cauldron`] over replica
    /// reads — the SAME [`Self::cauldron_action`] classification, so the two
    /// sides cannot drift. Fill state is block identity, the held item rides
    /// the snapshot, and the dye color is read from the replica's cell KV
    /// (`client_cell_kv_at` — the same replicated bytes the server holds),
    /// so the mirror is EXACT — including the stale-pot refusal. A `None`
    /// replica cell never produces a claim.
    pub(super) fn predict_use_cauldron(&self, pos: [i32; 3], actor: &PlayerSnapshot) -> bool {
        let Some(cauldron) = &self.cauldron else {
            return false;
        };
        let Some(block) = client_blocks_at(vec![pos]).into_iter().next().flatten() else {
            return false;
        };
        let dye = (block == cauldron.dye)
            .then(|| {
                client_cell_kv_at(DYE_KEY, vec![pos])
                    .into_iter()
                    .next()
                    .flatten()
                    .and_then(parse_dye)
            })
            .flatten();
        !matches!(
            self.cauldron_action(cauldron, block, actor, dye),
            CauldronSwap::None
        )
    }
}

#[cfg(test)]
mod tests {
    use super::mix_dye;

    /// Every channel of every mix lands on the 8-step grid inside `8..=248` —
    /// the invariant that BOUNDS the palette (and with it the engine's
    /// variant table): 31 values per channel, ≤ 31³ colors ever mintable.
    #[test]
    fn mix_output_always_lands_on_the_bounded_grid() {
        let pots = [[8, 8, 8], [128, 33, 7], [248, 248, 248], [255, 0, 90]];
        let pigments = [[255, 255, 255], [0, 0, 0], [200, 30, 40], [17, 99, 3]];
        for pot in pots {
            for pigment in pigments {
                for dilute in [false, true] {
                    let out = mix_dye(pot, pigment, dilute);
                    for c in out {
                        assert!((8..=248).contains(&c), "{out:?} escapes the clamp");
                        assert_eq!(c % 8, 0, "{out:?} is off the 8-step grid");
                    }
                }
            }
        }
    }

    /// A channel can never reach 0: zero transmittance is multiplicatively
    /// absorbing — no dilutant could ever recover it — so "black" is 8s.
    #[test]
    fn channels_never_collapse_to_zero() {
        let mut pot = [248u8; 3];
        for _ in 0..64 {
            pot = mix_dye(pot, [0, 0, 0], false);
        }
        assert_eq!(pot, [8; 3], "repeated max stain bottoms out at grid black");
    }

    /// Dilution converges to the grid's white fixed point `[248; 3]` from
    /// anywhere — including grid black — and staining with a full-pass
    /// channel never brightens past it.
    #[test]
    fn dilution_converges_to_grid_white() {
        let mut pot = [8u8; 3];
        for _ in 0..64 {
            pot = mix_dye(pot, [255, 255, 255], true);
        }
        assert_eq!(pot, [248; 3], "white dilutant recovers even a black pot");
        assert_eq!(
            mix_dye(pot, [255, 255, 255], true),
            [248; 3],
            "grid white is a fixed point"
        );
    }

    /// Stains accumulate SUBTRACTIVELY: each low-channel stir keeps darkening
    /// the channel it absorbs (monotone non-increasing, strictly down until
    /// the floor), and a stain never raises any channel — the pot's own
    /// absorption is never washed out.
    #[test]
    fn stains_accumulate_and_never_brighten() {
        let mut pot = [248u8, 248, 248];
        let yellow = [255u8, 255, 16]; // absorbs blue
        for _ in 0..32 {
            let next = mix_dye(pot, yellow, false);
            for c in 0..3 {
                assert!(next[c] <= pot[c], "stain brightened {pot:?} -> {next:?}");
            }
            pot = next;
        }
        assert_eq!(pot[2], 8, "the absorbed channel reaches grid black");
        assert!(pot[0] > 128 && pot[1] > 128, "pass channels stay bright");
    }

    /// The classic subtractive story: blue pot + repeated yellow = GREEN
    /// (the blue's red-absorption stays; the yellow kills blue), not an
    /// additive average.
    #[test]
    fn blue_plus_yellow_mixes_green() {
        let mut pot = [32u8, 64, 224]; // a blue pot
        let yellow = [240u8, 224, 16];
        for _ in 0..8 {
            pot = mix_dye(pot, yellow, false);
        }
        let [r, g, b] = pot;
        assert!(g > r && g > b, "expected green-dominant, got {pot:?}");
    }
}
