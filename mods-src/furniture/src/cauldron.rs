//! The cauldron: the mod's second custom shape, plus the dyeing it hosts.
//!
//! Rows are unoriented and all bake to [`CAULDRON_BOXES`], a hollow slate pot carved from the
//! row's `[top,bottom,side]` tiles. Fill state is block identity, like the chain's axis rows:
//! `furniture:cauldron_water` shares the shape kind, and only the render bake adds
//! [`WATER_SURFACE`], a collisionless sheet 1 px below the lip. The sim bake stays the empty pot,
//! so a body stands through the surface.
//!
//! Filling is an act-based `interact_attempt` consumer. A water bucket on the empty pot swaps
//! block and bucket, and an empty wooden bucket on the water pot scoops it back out. A water
//! bucket on a pot holding water or dye is absorbed, or the engine's pour ray would dump a source
//! over the pot. Anything else falls through. Both sides classify through one pure
//! [`Furniture::cauldron_action`] over replica-visible inputs, so client prediction is exact.
//!
//! Dyeing: an item declaring `furniture:pigment` stirs its color into a water or dye pot. Water
//! takes it straight (the `furniture:cauldron_dye` row); dye mixes subtractively ([`mix_dye`]).
//! The color is per-cell KV under [`DYE_KEY`], written server-side, replicated by the cell-KV
//! delta lane and passed to the render bake as `state_key` to tint [`WATER_SURFACE`]. The dye top
//! tile is bright and desaturated so the multiply tint carries the color.

use std::collections::HashMap;

use mod_sdk::*;

use super::{keys, Furniture};

const fn px(min: [f32; 3], max: [f32; 3]) -> ShapeAabb {
    ShapeAabb {
        min: [min[0] / 16.0, min[1] / 16.0, min[2] / 16.0],
        max: [max[0] / 16.0, max[1] / 16.0, max[2] / 16.0],
    }
}

/// Hollow pot built from overlapping cuboids so the silhouette reads rounded.
///
/// Wall ring (butted faces stay buried), belly plates with chamfered corners, overhanging lip above
/// a 2-px neck. Lip corners are cut 1x1 and overlap rather than butt, so the emitter's
/// coincident-face tie-break draws each shared plane once and the rim reads rounded like the belly.
/// Bottom mirrors the same graduation down to an inset floor slab at y 0, rounding all four
/// corners. Cavity is x/z 3..13 from y 3 up, open at top, with a 1-px shelf visible inside the lip.
///
/// One geometry source for sim, render, and item bakes. Fluid states are sibling rows on this shape
/// (block id = state); bake branches on the cell's block id.
pub(super) const CAULDRON_BOXES: [ShapeAabb; 13] = [
    px([2.0, 0.0, 2.0], [14.0, 3.0, 14.0]),
    px([1.0, 1.0, 1.0], [13.0, 14.0, 3.0]),
    px([13.0, 1.0, 1.0], [15.0, 14.0, 13.0]),
    px([3.0, 1.0, 13.0], [15.0, 14.0, 15.0]),
    px([1.0, 1.0, 3.0], [3.0, 14.0, 15.0]),
    // Belly plates (the rounded bulge). Each is only the SLIVER that stands
    // proud of its wall, and stops exactly on the wall's outer face rather than
    // reaching through it.
    //
    // `mesh::boxset` does not cull a face that straddles another box's plane.
    // Butted contact lets the emitter cull shared faces; interpenetration
    // leaves nearby same-facing quads that can fight at distance.
    px([2.0, 2.0, 0.5], [14.0, 12.0, 1.0]),
    px([2.0, 2.0, 15.0], [14.0, 12.0, 15.5]),
    px([0.5, 2.0, 2.0], [1.0, 12.0, 14.0]),
    px([15.0, 2.0, 2.0], [15.5, 12.0, 14.0]),
    px([1.0, 14.0, 0.0], [15.0, 16.0, 2.0]),
    px([1.0, 14.0, 14.0], [15.0, 16.0, 16.0]),
    px([0.0, 14.0, 1.0], [2.0, 16.0, 15.0]),
    px([14.0, 14.0, 1.0], [16.0, 16.0, 15.0]),
];

/// Water surface for the filled cauldron. Render only, not in the sim bake, no collision.
///
/// 1px sheet over the lip's inner opening (2..14). Top at y15, 1px below the lip top. Sides bury
/// into the lip ring's inner faces, underside rim sits on wall tops, so only the surface and cavity
/// ceiling draw. Water row's top tile paints the whole 2..14 window as water, covering the wall-top
/// shelf.
const WATER_SURFACE: ShapeAabb = px([2.0, 14.0, 2.0], [14.0, 15.0, 14.0]);

const WATER_AO: u8 = 30;

pub(super) struct Cauldron {
    pub(super) shape: u16,
    pub(super) empty: BlockId,
    pub(super) water: BlockId,
    pub(super) dye: BlockId,
}

const DYE_KEY: &str = "furniture:dye";

const USES_KEY: &str = "furniture:dye_uses";

const DYE_USES: u8 = 8;

const DYEABLE_KEY: &str = keys::DYEABLE;

/// A `furniture:dyeable` entry: `true` keeps the item it is on, `{"becomes": "<item>"}` hands back
/// another item wearing the dye, for an item whose own look a dye would not show through.
#[derive(serde::Deserialize)]
#[serde(untagged)]
enum DyeableSpec {
    Itself(bool),
    Becomes { becomes: String },
}

/// Every dyeable item with the name of the item a dip gives back for it.
pub(super) fn load_dyeables() -> Vec<(ItemId, String)> {
    let rows = items_with_data_as::<DyeableSpec>(DYEABLE_KEY);
    let names = item_names(rows.iter().map(|(item, _)| *item).collect());
    rows.into_iter()
        .zip(names)
        .filter_map(|((item, spec), name)| {
            let dyed = match spec {
                DyeableSpec::Itself(true) => name?,
                DyeableSpec::Itself(false) => return None,
                DyeableSpec::Becomes { becomes } => {
                    resolve_item_logged(&becomes)?;
                    becomes
                }
            };
            Some((item, dyed))
        })
        .collect()
}

const WOOL_DIP_MAX: u8 = 32;

const TINT_KEY: &str = "petramond:tint";

/// The dye surface for a draining pot: the full-pot sheet is [`WATER_SURFACE`]
/// (top at y15); each spent use sinks the sheet 1 px, bottoming out just
/// above the basin floor.
fn dye_surface(uses: u8) -> ShapeAabb {
    let top = 15.0 - (DYE_USES.saturating_sub(uses)) as f32;
    px([2.0, top - 1.0, 2.0], [14.0, top, 14.0])
}

/// Pigments aren't a compiled table, they're data interop: any item with a `furniture:pigment`
/// data entry (`{"color": [r,g,b], "dilute": true?}`) counts as a pigment, no matter who ships
/// it. This pack hangs the entry on the seven engine flowers via patch/data rows in its own
/// `items.json`; a berries pack could do the same with no furniture code involved. Loaded once
/// at init ([`load_pigments`], registry-only, so the client predictor matches exactly); bad
/// values get skipped with a log line. Keep stain colors saturated (high pass, low stop
/// channels). Stop channels do the mixing, and a half-high pass channel darkens every brew
/// it touches.
///
/// Mixing is subtractive ([`mix_dye`], Beer-Lambert): pot RGB is per-channel transmittance, and
/// a stain flower adds half a layer of absorbance on top. Transmittances multiply, never
/// average, so pigment accumulates. A blue pot plus yellow flowers goes green (blue's
/// red-absorption never leaves, yellow kills the blue channel), and stirring in more just
/// pushes the pot toward black. Dilutant flowers work the opposite way: they halve the pot's
/// absorbance per flower (plus their own faint half-layer), so whites brighten any dye, even
/// near-black, and pure-white daisies converge on the mixing grid's white, `[248; 3]`.
const PIGMENT_KEY: &str = keys::PIGMENT;

pub(super) fn load_pigments() -> Vec<(ItemId, [u8; 3], bool)> {
    items_with_data_as::<PigmentSpec>(PIGMENT_KEY)
        .into_iter()
        .map(|(item, spec)| (item, spec.color, spec.dilute))
        .collect()
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PigmentSpec {
    color: [u8; 3],
    #[serde(default)]
    dilute: bool,
}

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
        let rounded = if dilute {
            scaled.ceil()
        } else {
            scaled.floor()
        };
        let snapped = (rounded / 8.0).round() * 8.0;
        out[c] = snapped.clamp(8.0, 248.0) as u8;
    }
    out
}

enum CauldronSwap {
    None,
    Absorb,
    Bucket(BucketSwap),
    Dye(ItemId, [u8; 3], bool),
    DyeWool { dyeable: usize, count: u8 },
}

fn parse_dye(v: Vec<u8>) -> Option<[u8; 3]> {
    <[u8; 3]>::try_from(v.as_slice()).ok()
}

pub(super) fn resolve_cauldron() -> Option<Cauldron> {
    Some(Cauldron {
        shape: resolve_shape(keys::CAULDRON_SHAPE)?,
        empty: resolve_block(keys::CAULDRON)?,
        water: resolve_block(keys::CAULDRON_WATER)?,
        dye: resolve_block(keys::CAULDRON_DYE)?,
    })
}

impl Furniture {
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
            return CauldronSwap::None;
        }
        if let Some(buckets) = &self.buckets {
            if held == buckets.full && (block == cauldron.water || block == cauldron.dye) {
                return CauldronSwap::Absorb;
            }
            let vessel_full = match block {
                b if b == cauldron.empty => Some(false),
                b if b == cauldron.water => Some(true),
                _ => None,
            };
            if let Some(swap) = vessel_full.and_then(|full| buckets.swap_for(held, full)) {
                return CauldronSwap::Bucket(swap);
            }
        }
        let stale_dye_pot = block == cauldron.dye && dye.is_none();
        if (block == cauldron.water || block == cauldron.dye) && !stale_dye_pot {
            if let Some(&(_, pigment, dilute)) = self.pigments.iter().find(|(id, _, _)| *id == held)
            {
                if actor.sneak && held_item_places_block(Some(held)) {
                    return CauldronSwap::None;
                }
                return CauldronSwap::Dye(held, pigment, dilute);
            }
        }
        if block == cauldron.dye && !stale_dye_pot {
            if let Some(dyeable) = self.dyeables.iter().position(|(id, _)| *id == held) {
                if actor.sneak && held_item_places_block(Some(held)) {
                    return CauldronSwap::None;
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
        let Some((cauldron, block, dye, swap)) = self.cauldron_gate(&SideWorld::Server, pos, actor)
        else {
            return false;
        };
        match swap {
            CauldronSwap::None => false,
            CauldronSwap::Absorb => true,
            CauldronSwap::Bucket(swap) => {
                let Some(buckets) = &self.buckets else {
                    return false;
                };
                let next = match swap {
                    BucketSwap::Pour => cauldron.water,
                    BucketSwap::Scoop => cauldron.empty,
                };
                buckets.perform(swap, pos, || {
                    set_block(pos, next);
                })
            }
            CauldronSwap::Dye(flower, pigment, dilute) => {
                if !consume_held(flower, 1) {
                    return false;
                }
                let color = if block == cauldron.dye {
                    let Some(old) = dye else {
                        return false;
                    };
                    mix_dye(old, pigment, dilute)
                } else {
                    set_block(pos, cauldron.dye);
                    pigment
                };
                section_kv_set(pos, DYE_KEY, color.to_vec());
                if block != cauldron.dye {
                    section_kv_set(pos, USES_KEY, vec![DYE_USES]);
                }
                emit_sound(WATER_SPLASH_SOUND, Some(block_center(pos)));
                true
            }
            CauldronSwap::DyeWool { dyeable, count } => {
                let Some(color) = dye else {
                    return false;
                };
                let Some((held_id, dyed_name)) = self.dyeables.get(dyeable) else {
                    return false;
                };
                if !consume_held(*held_id, count as u32) {
                    return false;
                }
                give_item_data(dyed_name, count, &[(TINT_KEY, &color)]);
                let uses = section_kv_get(pos, USES_KEY)
                    .and_then(|v| v.first().copied())
                    .unwrap_or(DYE_USES);
                if uses <= 1 {
                    set_block(pos, cauldron.empty);
                } else {
                    section_kv_set(pos, USES_KEY, vec![uses - 1]);
                }
                emit_sound(WATER_SPLASH_SOUND, Some(block_center(pos)));
                true
            }
        }
    }

    pub(super) fn cauldron_dye_uses(
        &self,
        shape_kind: u16,
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

    pub(super) fn cauldron_fluid_box(
        &self,
        shape_kind: u16,
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
                dyed: tint.is_some(),
                ao: Some(WATER_AO),
            });
        }
        None
    }

    fn cauldron_gate(
        &self,
        world: &impl WorldView,
        pos: [i32; 3],
        actor: &PlayerSnapshot,
    ) -> Option<(&Cauldron, BlockId, Option<[u8; 3]>, CauldronSwap)> {
        let cauldron = self.cauldron.as_ref()?;
        let block = world.block(pos)?;
        let dye = (block == cauldron.dye)
            .then(|| world.cell_kv(pos, DYE_KEY).and_then(parse_dye))
            .flatten();
        let swap = self.cauldron_action(cauldron, block, actor, dye);
        Some((cauldron, block, dye, swap))
    }

    pub(super) fn cauldron_claims(
        &self,
        world: &impl WorldView,
        pos: [i32; 3],
        actor: &PlayerSnapshot,
    ) -> bool {
        self.cauldron_gate(world, pos, actor)
            .is_some_and(|(.., swap)| !matches!(swap, CauldronSwap::None))
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;

    /// NO TWO SAME-FACING FACES OF ONE SHAPE MAY SIT CLOSER THAN A TEXEL AND
    /// OVERLAP.
    ///
    /// `mesh::boxset` never culls a face that merely STRADDLES another box's
    /// plane, so a box reaching through another leaves the buried face retained
    /// a fraction of a texel behind the visible one. Two same-facing quads that
    /// close together fight as soon as the depth buffer stops separating them —
    /// at distance. The cauldron's belly plates butt against its walls so the
    /// emitter culls their shared faces.
    ///
    /// A full texel apart is a real step and fine — the floor slab sits inside
    /// the wall ring that way. It is the sub-texel offsets that fight.
    #[test]
    fn no_shape_in_this_pack_buries_a_face_a_sliver_deep() {
        let shapes: [(&str, Vec<ShapeAabb>); 2] = [
            ("cauldron", CAULDRON_BOXES.to_vec()),
            ("chain", crate::chains::cell_links()),
        ];
        const TEXEL: f32 = 1.0 / 16.0;
        const E: f32 = 1e-6;
        for (name, boxes) in shapes {
            for (i, a) in boxes.iter().enumerate() {
                for (j, b) in boxes.iter().enumerate() {
                    if i == j {
                        continue;
                    }
                    for axis in 0..3 {
                        let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                        if !(a.min[u] < b.max[u] - E
                            && b.min[u] < a.max[u] - E
                            && a.min[v] < b.max[v] - E
                            && b.min[v] < a.max[v] - E)
                        {
                            continue;
                        }
                        for (p, front, inward) in [
                            (a.min[axis], b.min[axis], true),
                            (a.max[axis], b.max[axis], false),
                        ] {
                            let straddles = b.min[axis] < p - E && p < b.max[axis] - E;
                            if !straddles {
                                continue;
                            }
                            let depth = if inward { p - front } else { front - p };
                            assert!(
                                depth >= TEXEL - E,
                                "{name}: box {j} buries box {i}'s face only {:.3} texels deep on axis {axis}",
                                depth * 16.0
                            );
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mix_dye;

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

    #[test]
    fn channels_never_collapse_to_zero() {
        let mut pot = [248u8; 3];
        for _ in 0..64 {
            pot = mix_dye(pot, [0, 0, 0], false);
        }
        assert_eq!(pot, [8; 3], "repeated max stain bottoms out at grid black");
    }

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

    #[test]
    fn stains_accumulate_and_never_brighten() {
        let mut pot = [248u8, 248, 248];
        let yellow = [255u8, 255, 16];
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

    #[test]
    fn blue_plus_yellow_mixes_green() {
        let mut pot = [32u8, 64, 224];
        let yellow = [240u8, 224, 16];
        for _ in 0..8 {
            pot = mix_dye(pot, yellow, false);
        }
        let [r, g, b] = pot;
        assert!(g > r && g > b, "expected green-dominant, got {pot:?}");
    }
}
