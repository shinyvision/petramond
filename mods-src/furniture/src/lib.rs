//! furniture — craftable wooden furniture as bbmodel blocks, plus the
//! chain/cauldron custom shapes and pot dyeing.
//!
//! Concerns live in their modules; this root wires them into the engine:
//!
//! - [`seats`]: sitting as pure mod policy over the actor-pose primitive —
//!   seat tables, the always-claim interact consumer + its client mirror,
//!   and the broken-piece release.
//! - [`chains`]: the three axis rows sharing one custom shape (axis = block
//!   identity), their plate geometry, and the face-normal placement rule.
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
    /// (per the bake purity rule) — the chain's axis plates or the cauldron's
    /// fixed pot. Light passes both: the chain's plates are thin and the
    /// cauldron is open-topped, so every cell reports the open aperture.
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
        cells
            .iter()
            .map(|cell| {
                let mut boxes: Vec<ShapeRenderBox> = self
                    .shape_boxes(shape_kind, cell.block_id)
                    .into_iter()
                    .map(Into::into)
                    .collect();
                boxes.extend(self.cauldron_fluid_box(shape_kind, cell, &uses_at));
                BakedRenderCell { boxes }
            })
            .collect()
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
        } else if self.cauldron.as_ref().is_some_and(|c| c.shape == shape_kind) {
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
            || self.lanterns.as_ref().is_some_and(|l| l.shape == shape_kind)
            || self.cauldron.as_ref().is_some_and(|c| c.shape == shape_kind)
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
        if self.cauldron.as_ref().is_some_and(|c| c.shape == shape_kind) {
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
