//! furniture — craftable wooden furniture as bbmodel blocks, plus the
//! chain/cauldron custom shapes and pot dyeing.
//!
//! Concerns live in their modules; this root wires them into the engine:
//!
//! - [`seats`]: sitting as pure mod policy over the actor-pose primitive —
//!   seat tables, the always-claim interact consumer + its client mirror,
//!   and the broken-piece release.
//! - [`chains`]: the three axis rows sharing one custom shape (axis = block
//!   identity), their link-ring geometry, and the face-normal placement rule.
//! - [`lanterns`]: the standing/hanging pair over one custom shape (the same
//!   pattern), the bail geometry, and the underside-hangs-it placement rule.
//! - [`cauldron`]: the pot shape, its fill-state rows (fill = block
//!   identity), bucket fill/scoop, and DYEING — pigment declarations,
//!   subtractive mixing, the per-cell color/uses KV, and the tinted
//!   render-only fluid surface.

use mod_sdk::*;

mod cauldron;
mod chains;
mod lanterns;
mod seats;

use cauldron::{resolve_cauldron, Cauldron};
use chains::{resolve_chains, Chains};
use lanterns::{resolve_lanterns, Lanterns};
use seats::{release_broken_piece_sitters, ResolvedPiece, PIECES};

/// A box is a STRUT when it is slender in TWO of its three axes — a chain
/// link's bar, the wall bracket's beam. Slender in one axis is a PLATE, which
/// still has a broad face and is not this.
fn is_strut(b: &ShapeAabb) -> bool {
    let mut dims = [0; 3].map(|_| 0.0f32);
    for (k, d) in dims.iter_mut().enumerate() {
        *d = b.max[k] - b.min[k];
    }
    dims.sort_by(|a, b| a.partial_cmp(b).expect("finite box"));
    dims[1] <= STRUT_SPAN
}

/// Slenderness, on the smaller two axes, at or under which a box is a strut.
const STRUT_SPAN: f32 = 2.0 / 16.0;

/// How dark the most enclosed strut is baked. The openness the rays measure is
/// NORMALIZED onto `DEEPEST..1.0` across the shape, so this is exactly the
/// contrast the shape shows — raw openness spans barely a tenth of its range on
/// a form as open as a chain, and used directly it bakes a uniform grey bar.
///
/// It is a multiply in LINEAR light, so the spread is `1.0 / DEEPEST`, and it
/// is a per-box multiply rather than per-texel paint — so it cannot produce the
/// row-on-row alternation a tile gradient does at any close range.
///
/// It is NOT free at distance, which is the reason for this exact number. A
/// vertex tint does not mipmap: looked at END-ON down a receding run, the boxes
/// alternate faster than a pixel and nothing averages them. Measured on a
/// north/south run at 16 and 28 blocks (`harness/src/bin/chainshot.rs`), against
/// a plain stone cube as the control:
///
///   0.55 (1.82x) 116 speckled px | 0.68 (1.47x) 77 | 0.72 (1.39x) 24
///   0.78 (1.28x)   0             | the stone cube itself: 25
///
/// 0.72 is the widest that still sits at the control, so it is the most
/// contrast the chain can carry without paying for it in the distance.
const DEEPEST: f32 = 0.72;

/// How far a ray looks for an occluder. Past about a link, nothing in a shape
/// this slender is shadowing anything, and a longer reach just drags every
/// strut towards the same value.
const OCCLUSION_REACH: f32 = 8.0 / 16.0;

/// BAKED occlusion for one box of a shape's box list, as a flat RGB multiply —
/// `None` where the renderer should shade the box the ordinary way.
///
/// Ambient occlusion approximates contact darkening, and it only says something
/// on a face broad enough to hold a gradient. On a strut, every corner probe
/// lands against the strut's own neighbours instead, so what the runtime
/// computes is per-texel contrast with no contact behind it — and once a texel
/// shrinks under a screen pixel, that contrast alternates row-on-row and reads
/// as two colours fighting.
///
/// The same is true of anything the TILE could say: a chain's rods are one
/// texel thick, so there is no room across a rod for a gradient, and every
/// texel of variation painted there is misread by some face that samples the
/// tile down a different axis. Both roads end in per-texel noise.
///
/// So the occlusion is baked here instead — cast a fixed ray set out of the box
/// and take the fraction that escape the shape — and the tiles are flat. The
/// result is ONE multiply for the whole box, which makes the smallest thing
/// that can change brightness a box face rather than a texel: the artifact is
/// not tuned down, it is made unrepresentable. Pair it with `ao: Some(0)`, or
/// the runtime probe this replaces applies on top of it.
fn baked_occlusion(boxes: &[ShapeAabb], i: usize) -> Option<[u8; 3]> {
    if !is_strut(&boxes[i]) {
        return None;
    }
    let occluders = occluders(boxes);
    let raw: Vec<f32> = (0..boxes.len())
        .map(|k| openness(&occluders, &boxes[k]))
        .collect();
    let (lo, hi) = raw
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), &v| (l.min(v), h.max(v)));
    // Normalize across the shape. A form as open as a chain measures 0.38..0.73
    // raw, which used directly is a 4% spread — invisible. The SHAPE's own
    // range is the meaningful one.
    let span = hi - lo;
    let t = if span > 1e-4 {
        (raw[i] - lo) / span
    } else {
        1.0
    };
    let v = DEEPEST + (1.0 - DEEPEST) * t;
    Some([(v * 255.0).round().clamp(0.0, 255.0) as u8; 3])
}

/// The fraction of a fixed ray set leaving `b`'s centre that escapes.
fn openness(occluders: &[ShapeAabb], b: &ShapeAabb) -> f32 {
    let from = [0, 1, 2].map(|k| (b.min[k] + b.max[k]) * 0.5);
    let (mut open, mut cast) = (0u32, 0u32);
    for dx in -1..=1i32 {
        for dy in -1..=1i32 {
            for dz in -1..=1i32 {
                if (dx, dy, dz) == (0, 0, 0) {
                    continue;
                }
                let d = [dx as f32, dy as f32, dz as f32];
                let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                let dir = [d[0] / len, d[1] / len, d[2] / len];
                cast += 1;
                // `b` itself is in `occluders`; a ray starting at its centre
                // always "enters" it, so the box's own volume is skipped by
                // identity of extent rather than by index.
                let blocked = occluders.iter().any(|o| o != b && ray_enters(from, dir, o));
                open += u32::from(!blocked);
            }
        }
    }
    open as f32 / cast as f32
}

/// The occluder set for a bake: the shape's own boxes, plus a copy one cell
/// either way along every axis the shape SPANS — touches both boundaries of.
///
/// A chain spans its cell, so in place it is a continuous run and its end
/// links are not open at all. Baking a lone cell says they are, which lifts
/// the boxes at both boundaries to the brightest value in the shape and paints
/// a lit band across every cell seam of every run — a repeating artifact, and
/// a worse one than the flat bar it was meant to fix.
fn occluders(boxes: &[ShapeAabb]) -> Vec<ShapeAabb> {
    let mut out = boxes.to_vec();
    for axis in 0..3 {
        let spans =
            boxes.iter().any(|b| b.min[axis] <= 0.0) && boxes.iter().any(|b| b.max[axis] >= 1.0);
        if !spans {
            continue;
        }
        for step in [-1.0f32, 1.0] {
            out.extend(boxes.iter().map(|b| {
                let mut c = *b;
                c.min[axis] += step;
                c.max[axis] += step;
                c
            }));
        }
    }
    out
}

/// Slab test: does `from + t * dir` enter `b` for some `t` in
/// `0..OCCLUSION_REACH`?
fn ray_enters(from: [f32; 3], dir: [f32; 3], b: &ShapeAabb) -> bool {
    let (mut near, mut far) = (0.0f32, OCCLUSION_REACH);
    for k in 0..3 {
        if dir[k].abs() < 1e-6 {
            if from[k] < b.min[k] || from[k] > b.max[k] {
                return false;
            }
            continue;
        }
        let (a, c) = ((b.min[k] - from[k]) / dir[k], (b.max[k] - from[k]) / dir[k]);
        near = near.max(a.min(c));
        far = far.min(a.max(c));
    }
    near <= far
}

const ON_INTERACT: u32 = 1;
const ON_BLOCK_BROKEN: u32 = 2;

#[derive(Default)]
struct Furniture {
    pieces: Vec<ResolvedPiece>,
    chains: Option<Chains>,
    /// The lantern family (`None`: pack content didn't load).
    lanterns: Option<Lanterns>,
    /// The cauldron family (`None`: pack content didn't load).
    cauldron: Option<Cauldron>,
    /// The engine water bucket (`None`: base content missing) — the item the
    /// cauldron fill consumes.
    water_bucket: Option<ItemId>,
    /// The empty wooden bucket — the item the scoop-out consumes.
    wooden_bucket: Option<ItemId>,
    /// Every item declaring `furniture:dyeable`, with its registry name
    /// (see `cauldron::load_dyeables`).
    dyeables: Vec<(ItemId, String)>,
    /// Every item declaring a `furniture:pigment` data entry, with its
    /// parsed color and dilute flag (see `cauldron::load_pigments`).
    pigments: Vec<(ItemId, [u8; 3], bool)>,
    /// Running as the CLIENT instance: only PREDICT the sit claim against the
    /// replica — sim host calls are unavailable on this side.
    client: bool,
}

impl Mod for Furniture {
    fn init(&mut self) {
        self.pieces = PIECES
            .iter()
            .filter_map(|piece| {
                let block = resolve_block(piece.block)?;
                Some(ResolvedPiece { block, piece })
            })
            .collect();
        self.client = runtime_side() == RuntimeSide::Client;
        self.chains = resolve_chains();
        self.lanterns = resolve_lanterns();
        self.cauldron = resolve_cauldron();
        self.water_bucket = resolve_item("petramond:water_bucket");
        self.wooden_bucket = resolve_item("petramond:wooden_bucket");
        self.dyeables = cauldron::load_dyeables();
        self.pigments = cauldron::load_pigments();
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT);
        if !self.client {
            register_event_handler(EventKind::BlockBroken, 0, ON_BLOCK_BROKEN);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (
                ON_INTERACT,
                EventPayload::InteractAttempt {
                    block: Some(pos),
                    player,
                    ..
                },
            ) => {
                let actor = player_state();
                let claimed = if self.client {
                    self.predict_use_cauldron(*pos, &actor) || self.predict_sit(*pos, &actor)
                } else {
                    self.try_use_cauldron(*pos, &actor) || self.try_sit(*pos, *player, &actor)
                };
                if claimed {
                    Outcome::Cancel
                } else {
                    Outcome::Continue
                }
            }
            (ON_BLOCK_BROKEN, EventPayload::BlockBroken { pos, block, .. }) => {
                if let Some(resolved) = self.pieces.iter().find(|p| p.block == *block) {
                    release_broken_piece_sitters(resolved.block, resolved.piece, *pos);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    /// SIM bake (deterministic — server and client replica): the box list for
    /// the cell's shape kind, a pure function of the cell's block id alone
    /// (per the bake purity rule) — the chain's link rings, the lantern's lamp,
    /// or the cauldron's fixed pot. Light passes them all: the rings and the
    /// lamp are slender and the cauldron is open-topped, so every cell reports
    /// the open aperture.
    fn bake_shape_sim(&mut self, shape_kind: u8, cells: &[CellInput]) -> Vec<BakedSimCell> {
        if !self.owns_shape(shape_kind) {
            return Vec::new();
        }
        cells
            .iter()
            .map(|cell| BakedSimCell {
                collision_boxes: self.shape_boxes(shape_kind, cell.block_id),
                light_aperture: LightAperture::Open,
            })
            .collect()
    }

    /// RENDER bake (client): the same boxes the sim bake reports — so the
    /// drawn boxes, the selection union, and the collision agree — plus the
    /// one deliberate divergence: the filled cauldron's fluid sheet, which
    /// draws (tinted, for dye) but never collides (see
    /// `Furniture::cauldron_fluid_box`).
    fn bake_shape_render(&mut self, shape_kind: u8, cells: &[CellInput]) -> Vec<BakedRenderCell> {
        if !self.owns_shape(shape_kind) {
            return Vec::new();
        }
        let uses_at = self.cauldron_dye_uses(shape_kind, cells);
        // The box list is a pure function of the block id (the bake purity
        // rule), and so is the occlusion baked off it — so a run of chain,
        // which is one id repeated, bakes its rays once and not per cell.
        let mut baked: Vec<(BlockId, Vec<ShapeRenderBox>)> = Vec::new();
        let mut out = Vec::with_capacity(cells.len());
        for cell in cells {
            if !baked.iter().any(|(id, _)| *id == cell.block_id) {
                let raw = self.shape_boxes(shape_kind, cell.block_id);
                let cooked = raw
                    .iter()
                    .enumerate()
                    .map(|(i, aabb)| {
                        let tint = baked_occlusion(&raw, i);
                        ShapeRenderBox {
                            tint,
                            ao: tint.map(|_| 0),
                            ..(*aabb).into()
                        }
                    })
                    .collect();
                baked.push((cell.block_id, cooked));
            }
            let mut boxes = baked
                .iter()
                .find(|(id, _)| *id == cell.block_id)
                .expect("just baked")
                .1
                .clone();
            boxes.extend(self.cauldron_fluid_box(shape_kind, cell, &uses_at));
            out.push(BakedRenderCell { boxes });
        }
        out
    }

    /// ITEM bake (client, once at load): the chain's icon / in-hand /
    /// dropped form is always the VERTICAL plate pair, however the block row
    /// the item links is oriented — a held chain reads like the vanilla
    /// item. The cauldron is unoriented; its one box list is the item too.
    fn bake_shape_item(&mut self, shape_kind: u8, _block: BlockId) -> BakedItemGeometry {
        let boxes = if self.chains.as_ref().is_some_and(|c| c.shape == shape_kind) {
            chains::cell_links()
        } else if let Some(lanterns) = self.lanterns.as_ref().filter(|l| l.shape == shape_kind) {
            lanterns.item_boxes()
        } else if self
            .cauldron
            .as_ref()
            .is_some_and(|c| c.shape == shape_kind)
        {
            cauldron::CAULDRON_BOXES.to_vec()
        } else {
            Vec::new()
        };
        BakedItemGeometry { boxes }
    }

    /// Placement (chain + cauldron): accept the click cell; for a chain,
    /// write the axis row for the clicked face's normal (vertical off the
    /// top/bottom faces, north/south or east/west off the side faces) as the
    /// plan's block override — the cauldron is unoriented and keeps the held
    /// row (`block: None`). The host owns every world gate — loaded,
    /// replaceable, body occupancy.
    fn shape_placement_plan(
        &mut self,
        shape_kind: u8,
        _block: BlockId,
        inputs: &PlaceInputsView,
    ) -> ShapePlacementResult {
        let row = self
            .chains
            .as_ref()
            .filter(|c| c.shape == shape_kind)
            .map(|c| c.row_for_normal(inputs.normal))
            .or_else(|| {
                self.lanterns
                    .as_ref()
                    .filter(|l| l.shape == shape_kind)
                    .map(|l| l.row_for_normal(inputs.normal))
            });
        ShapePlacementResult {
            accepted: true,
            anchor: inputs.place_pos,
            cells: vec![inputs.place_pos],
            block: row,
        }
    }
}

impl Furniture {
    /// Whether a bake dispatch's shape kind is one of this mod's shapes.
    fn owns_shape(&self, shape_kind: u8) -> bool {
        self.chains.as_ref().is_some_and(|c| c.shape == shape_kind)
            || self
                .lanterns
                .as_ref()
                .is_some_and(|l| l.shape == shape_kind)
            || self
                .cauldron
                .as_ref()
                .is_some_and(|c| c.shape == shape_kind)
    }

    /// The box list for a placed cell — the one geometry source the sim and
    /// render bakes share so collision, selection, and the mesh can't drift.
    fn shape_boxes(&self, shape_kind: u8, block: BlockId) -> Vec<ShapeAabb> {
        if let Some(chains) = self.chains.as_ref().filter(|c| c.shape == shape_kind) {
            return chains.links_for(block);
        }
        if let Some(lanterns) = self.lanterns.as_ref().filter(|l| l.shape == shape_kind) {
            return lanterns.boxes_for(block);
        }
        if self
            .cauldron
            .as_ref()
            .is_some_and(|c| c.shape == shape_kind)
        {
            return cauldron::CAULDRON_BOXES.to_vec();
        }
        Vec::new()
    }
}

/// Whether the held item places a block (its row carries a `block` link) —
/// the gate the sneak-defer rule reads. Registry-only, legal on any
/// instance; an unresolvable id reads as "not a block".
fn held_places_a_block(held: Option<ItemId>) -> bool {
    let Some(id) = held else {
        return false;
    };
    item_names(vec![id])
        .into_iter()
        .next()
        .flatten()
        .and_then(|name| item_info(&name))
        .is_some_and(|info| info.block.is_some())
}

mod_sdk::register_mod!(Furniture);

#[cfg(test)]
mod bake_tests {
    use super::*;

    /// EVERY chain link box must classify as a strut. The bake is what keeps
    /// the chain's shading off the texel — a link that stops being a strut
    /// silently regains the runtime AO probe, and with it the row-on-row
    /// alternation that reads as z-fighting. Thickening a link is exactly the
    /// kind of edit that would do it.
    #[test]
    fn every_chain_link_is_a_strut() {
        let links = chains::cell_links();
        assert!(!links.is_empty());
        for (i, b) in links.iter().enumerate() {
            assert!(is_strut(b), "link box {i} is not a strut: {b:?}");
        }
    }

    /// A PLATE is not a strut. Slender in one axis is a broad face that still
    /// wants its contact shadow — flattening those too would take the grounding
    /// off the lantern's foot and the cauldron's walls for no gain.
    #[test]
    fn a_plate_keeps_its_contact_shadow() {
        let plate = ShapeAabb {
            min: [4.0 / 16.0, 0.0, 4.0 / 16.0],
            max: [12.0 / 16.0, 1.0 / 16.0, 12.0 / 16.0],
        };
        assert!(!is_strut(&plate));
    }

    /// The bake must be PERIODIC along a run: link k and the link two above it
    /// are the same box in the same surroundings, so they must bake the same.
    ///
    /// They only do because the occluder set repeats the cell (`occluders`).
    /// Bake a lone cell and the boxes at both boundaries measure open, come out
    /// the brightest in the shape, and paint a lit band across every cell seam
    /// of every run — which is a repeating artifact rather than a subtle one,
    /// and no screenshot of a single block would ever show it.
    #[test]
    fn baked_occlusion_is_periodic_along_a_run() {
        let links = chains::cell_links();
        let per_link = 4;
        let bake = |i| baked_occlusion(&links, i).expect("every link bakes")[0];
        for i in 0..links.len() - 2 * per_link {
            assert_eq!(
                bake(i),
                bake(i + 2 * per_link),
                "box {i} and the matching box two links up bake differently"
            );
        }
        // And the ends specifically: this is where a lone-cell bake shows.
        assert_eq!(
            bake(0),
            bake(2 * per_link),
            "the run's first link is special-cased"
        );
    }

    /// The bake must SPEND its range. Raw openness spans about a tenth of 0..1
    /// on a form this open, so an un-normalized bake produces a uniform grey
    /// bar that looks like a fix and ships no shading at all.
    #[test]
    fn baked_occlusion_uses_its_range() {
        let links = chains::cell_links();
        let tints: Vec<u8> = (0..links.len())
            .filter_map(|i| baked_occlusion(&links, i).map(|t| t[0]))
            .collect();
        assert_eq!(tints.len(), links.len(), "every link should bake");
        let (lo, hi) = (
            *tints.iter().min().expect("links"),
            *tints.iter().max().expect("links"),
        );
        let spread = f32::from(hi) / f32::from(lo);
        let want = 1.0 / DEEPEST;
        assert!(
            (spread - want).abs() < 0.05,
            "baked spread {spread:.2}x should be DEEPEST's {want:.2}x"
        );
    }
}
