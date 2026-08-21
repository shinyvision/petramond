//! The single per-shape placement ladder: validity check → [`PlacementPlan`]
//! (resulting state write + cell footprint), evaluated by server placement
//! against the authoritative world and by the client place ghost against the
//! replica. The slab arm's `slab_layer_target_state` pattern ("one
//! placement-validity rule, shared by both sides") generalized to every
//! shape, so the two sides cannot drift into a prediction desync arm by arm.
//!
//! Side-specific policy stays with the callers: the server's `block_place_pre`
//! mod event, inventory consumption, and events; the client's ghost gates
//! (mod blocks, replace-in-place, the accept convention). Body occupancy is
//! side-specific too (server sessions + sim mobs vs the client's predicted
//! body + replicated rows), so the rules take it as a closure over the cells
//! and boxes the placed shape would occupy.

use super::data::WorldData;
use crate::block::SupportDir;
use crate::block::{Aabb, Block, CellPart, ShapeFamily, ShapeState};
use crate::block_state::{HeldBlockState, LogAxis, SlabState, StairHalf, StairState};
use crate::facing::Facing;
use crate::item::ItemType;
use crate::mathh::IVec3;
use crate::slab::{SlabRotation, SlabSlot};

/// Whether a click on `looked_at` REPLACES it where it stands instead of
/// building against its face: replaceable MATTER — tall grass, a snow layer,
/// water (so a bucket pours in place) — which air, being nothing, is not.
pub fn replaces_in_place(looked_at: Block) -> bool {
    looked_at.is_replaceable() && looked_at != Block::Air
}

/// THE build-position rule: which cell a click builds into. Server placement,
/// server item use, and both client prediction paths resolve through this, so
/// a change to what counts as replace-in-place cannot land on one side only
/// and desync the ghost from the authority.
///
/// Callers keep their own zero-normal guard where they have one: whether a
/// faceless hit is a refusal or a build in place is the CALLER's policy, not
/// part of this rule.
pub fn build_position(looked_at: Block, hit: IVec3, normal: IVec3) -> IVec3 {
    if replaces_in_place(looked_at) {
        hit
    } else {
        hit + normal
    }
}

fn rotation_count(block: crate::block::Block) -> u8 {
    if block.shape_family() == ShapeFamily::Slab {
        3
    } else {
        2
    }
}

fn rotatable_block(block: crate::block::Block) -> bool {
    matches!(block.shape_family(), ShapeFamily::Stair | ShapeFamily::Slab) || block.is_log()
}

/// The held block's placement-rotation state (the R-key cycle): which item the
/// cycle was armed on and the raw counter. Lives BOTH client-side (the client
/// owns the R key and previews the rotated held block) and session-side (the
/// placement paths read the session's copy, fed from `PlayerUpdate`'s raw
/// counter) — one struct so the two can never drift in logic.
#[derive(Clone, Debug, Default)]

pub struct HeldRotation {
    pub item: Option<ItemType>,
    pub rotation: u8,
}

impl HeldRotation {
    /// Cycle the rotation for `selected` (stairs upside-down, slab column/row,
    /// log axis). Selecting a non-rotatable item clears it.
    pub fn toggle(&mut self, selected: Option<ItemType>) {
        let Some(item) = selected else {
            self.clear();
            return;
        };
        if !item.as_block().is_some_and(rotatable_block) {
            self.clear();
            return;
        }
        if self.item == Some(item) {
            let count = item.as_block().map_or(1, rotation_count).max(1);
            self.rotation = (self.rotation + 1) % count;
        } else {
            self.item = Some(item);
            self.rotation = 1 % item.as_block().map_or(1, rotation_count).max(1);
        }
    }

    #[inline]
    pub fn clear(&mut self) {
        self.item = None;
        self.rotation = 0;
    }

    /// Latch the raw counter a `PlayerUpdate` carried. The wire carries ONLY
    /// the counter; the session re-derives the armed item as its own currently
    /// selected item whenever the counter changes to nonzero (the client
    /// resets the counter to 0 on every hotbar change, so a changed nonzero
    /// counter can only mean an R-press on the current selection). An
    /// unchanged counter keeps the armed item as-is, preserving the
    /// "rotation is remembered per item" activity check.
    pub fn apply_wire(&mut self, counter: u8, selected: Option<ItemType>) {
        if counter == self.rotation {
            return;
        }
        if counter == 0 {
            self.clear();
        } else {
            self.rotation = counter;
            self.item = selected;
        }
    }

    #[inline]
    fn active(&self, selected: Option<ItemType>) -> bool {
        let Some(item) = selected else {
            return false;
        };
        self.item == Some(item)
            && self.rotation != 0
            && item.as_block().is_some_and(rotatable_block)
    }

    #[inline]
    pub fn held_block_state(&self, selected: Option<ItemType>) -> HeldBlockState {
        let Some(block) = selected.and_then(ItemType::as_block) else {
            return HeldBlockState::None;
        };
        if block.shape_family() == ShapeFamily::Stair {
            return HeldBlockState::Stair(StairState::new(
                crate::block_model::DEFAULT_MODEL_FACING,
                self.stair_half(selected),
            ));
        }
        if block.shape_family() == ShapeFamily::Slab {
            let slot = crate::slab::slot_for_rotation(
                self.slab_rotation(selected),
                IVec3::ZERO,
                crate::facing::Facing::South,
            );
            return HeldBlockState::Slab(SlabState::single(slot.split, slot.index, block));
        }
        if block.is_log() {
            return HeldBlockState::Log(if self.active(selected) {
                LogAxis::X
            } else {
                LogAxis::Y
            });
        }
        HeldBlockState::None
    }

    #[inline]
    pub fn stair_half(&self, selected: Option<ItemType>) -> StairHalf {
        if self.active(selected) {
            StairHalf::Top
        } else {
            StairHalf::Bottom
        }
    }

    #[inline]
    pub fn slab_rotation(&self, selected: Option<ItemType>) -> crate::slab::SlabRotation {
        if self.active(selected) {
            crate::slab::SlabRotation::from_index(self.rotation)
        } else {
            crate::slab::SlabRotation::Bottom
        }
    }

    #[inline]
    pub fn log_axis_for_facing(
        &self,
        selected: Option<ItemType>,
        facing: crate::facing::Facing,
    ) -> LogAxis {
        if !self.active(selected) {
            return LogAxis::Y;
        }
        match facing {
            crate::facing::Facing::East | crate::facing::Facing::West => LogAxis::X,
            crate::facing::Facing::North | crate::facing::Facing::South => LogAxis::Z,
        }
    }
}

/// Player-derived inputs to the placement rules, resolved by each side from
/// its own session / held-rotation state before the ladder runs.
pub struct PlaceInputs {
    /// The clicked cell (the raycast hit).
    pub hit: IVec3,
    /// The clicked face's outward normal.
    pub normal: IVec3,
    /// The build cell: the hit cell when replacing a plant in place, else
    /// `hit + normal`.
    pub place_pos: IVec3,
    /// Whether the click replaces a replaceable non-air block in its own cell
    /// (drops a replacing torch to the floor mount).
    pub replacing_in_place: bool,
    pub player_facing: Facing,
    /// The held block's raw placement-rotation state (the R-key cycle) plus
    /// the held item it is armed on. GENERIC input-device state: each family
    /// derives its own reading (a stair's half, a slab's row/column, a log's
    /// axis) — the engine pre-derives nothing.
    pub held_rotation: HeldRotation,
    pub held: Option<crate::item::ItemType>,
}

/// One cell a [`PlacementPlan`] writes: the block row and the opaque initial
/// cell-state bytes, plus whether the write claims the WHOLE cell or just one
/// of its parts.
#[derive(Copy, Clone)]
pub struct CellWrite {
    pub cell: IVec3,
    pub block: Block,
    pub state: ShapeState,
    /// The sub-cell [`CellPart`] this write claims — where the carry courier
    /// lands the placed stack's data. `0` for every single-part family, which
    /// is the bare, un-suffixed KV address.
    pub part: CellPart,
    /// Whether this write AUGMENTS the cell (adds a part to one that already
    /// holds others) rather than replacing it whole.
    ///
    /// A block write clears the cell's whole KV map, which is exactly right
    /// when the cell is being replaced — air holds no data — and wrong when a
    /// second slab stacks into a dyed one, where it would silently eat the
    /// sibling layer's colour. An augmenting write carries the map across;
    /// the courier then overwrites only `part`'s own keys.
    pub augments: bool,
}

/// A validated placement: the cell it anchors on (the commit target — the
/// clicked cell for a slab stack, the oriented base for a model, the lower
/// cell for a door) and every [`CellWrite`] the write lands.
/// The commit is GENERIC: there is no per-family write vocabulary — a family
/// expresses ANY placement as block ids plus opaque cell-state bytes, and the
/// refine cascade resolves the neighbour-dependent remainder after the write.
/// A family may write sibling block rows (a wall panel's facing row, a
/// chain's axis row) or several cells (a door's pair, a model's footprint) —
/// the engine never knows which family it is committing.
pub struct PlacementPlan {
    pub anchor: IVec3,
    pub writes: Vec<CellWrite>,
}

impl PlacementPlan {
    /// The common single-cell plan: a whole-cell write of the cell's one part.
    pub fn single(cell: IVec3, block: Block, state: ShapeState) -> Self {
        Self {
            anchor: cell,
            writes: vec![Self::whole(cell, block, state)],
        }
    }

    /// A single-cell plan whose write claims one sub-cell `part`. `augments`
    /// is true when the cell already holds OTHER parts whose data must survive
    /// the block write (stacking a second slab layer into a dyed cell), false
    /// for a fresh cell that happens to fill a non-zero part (a lone top slab
    /// hung under a ceiling).
    pub fn single_part(
        cell: IVec3,
        block: Block,
        state: ShapeState,
        part: CellPart,
        augments: bool,
    ) -> Self {
        Self {
            anchor: cell,
            writes: vec![CellWrite {
                cell,
                block,
                state,
                part,
                augments,
            }],
        }
    }

    /// A whole-cell write of `block` at `cell`.
    pub fn whole(cell: IVec3, block: Block, state: ShapeState) -> CellWrite {
        CellWrite {
            cell,
            block,
            state,
            part: 0,
            augments: false,
        }
    }

    /// Every cell the write touches — the prediction ledger / rollback
    /// footprint.
    pub fn cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.writes.iter().map(|w| w.cell)
    }

    /// The part the ANCHOR write claims — what the carry courier restores
    /// into.
    pub fn anchor_part(&self) -> CellPart {
        self.writes
            .iter()
            .find(|w| w.cell == self.anchor)
            .map_or(0, |w| w.part)
    }
}

/// A shape family's answer to a placement click — the SEAM that replaced the
/// engine's per-family placement match. A family either fully owns the
/// placement (a stair's facing+half, a slab's stack slot, a door's two cells)
/// or defers to the generic single-cell path.
pub enum PlacementOutcome {
    /// The click cannot place here (no floor for a door, a slab cell already
    /// full, a body in the way).
    Refused,
    /// This family has no bespoke placement: use the generic single-cell path
    /// (`World::general_placement_plan`) — cube/log/directional blocks,
    /// plants, and any family that never overrides.
    General,
    /// A fully-resolved placement.
    Plan(PlacementPlan),
}

/// The placement seam every shape family implements. The engine holds NO
/// per-family placement dispatch: `World::placement_plan` asks the cell's
/// shape kind, and a mod family answers exactly as an engine one does.
pub trait ShapePlacement: Send + Sync + 'static {
    /// Resolve a placement of `block` for this click, or defer. Reads the
    /// world for support/occupancy through `w`; `occupied` reports whether a
    /// gameplay body overlaps the given boxes at a cell (side-specific — the
    /// server's sessions+mobs, the client's predicted+replicated bodies).
    fn placement_plan(
        &self,
        _w: &WorldData,
        _block: Block,
        _inputs: &PlaceInputs,
        _occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> PlacementOutcome {
        PlacementOutcome::General
    }
}

/// The SHARED validation of an accepted custom-shape placement plan — evaluated
/// by the server against the authoritative world and by the client place
/// ghost against the replica, so the two sides compute the same write by
/// construction (one rule, never two hand-kept copies). Refuses: a plan
/// writing more than the anchor cell, an anchor more than Chebyshev 2 from
/// `place_pos`, or a `block` override that is not a sibling row of the same
/// shape kind (orientation as block identity; a kind belongs to one pack, so
/// a plan can never reach across packs). Returns the anchor and the row to
/// write (the held row by default).
pub fn validate_custom_plan(
    result: &mod_api::ShapePlacementResult,
    held: Block,
    shape_kind: u8,
    place_pos: IVec3,
) -> Option<(IVec3, Block)> {
    let write_block = match result.block {
        None => held,
        Some(b) => {
            let b = Block::from_id(b.0);
            if b.shape_kind().0 != shape_kind {
                return None;
            }
            b
        }
    };
    let anchor = IVec3::new(result.anchor[0], result.anchor[1], result.anchor[2]);
    // Placement is SINGLE-CELL and stateless: the guest may claim only the
    // anchor cell (an empty `cells`, or exactly `[anchor]`). A wider
    // footprint is refused here — the host cannot yet atomically gate,
    // re-bake, or remove a multi-cell custom object, so shipping the wire
    // field is fine but honouring more than one cell is not.
    let single_cell = result.cells.is_empty()
        || (result.cells.len() == 1 && result.cells[0] == anchor.to_array());
    if !single_cell {
        return None;
    }
    // Bound the anchor to a small neighbourhood of the click so a plan
    // cannot place kilometres from where the player aimed.
    let (dx, dy, dz) = (
        (anchor.x - place_pos.x).abs(),
        (anchor.y - place_pos.y).abs(),
        (anchor.z - place_pos.z).abs(),
    );
    if dx.max(dy).max(dz) > 2 {
        return None;
    }
    Some((anchor, write_block))
}

impl WorldData {
    pub fn fragile_supported(&self, pos: IVec3, block: Block) -> bool {
        // A shape whose support comes from its PLACEMENT answers where it
        // grips; the face-completeness test is then the shared one. No family
        // is named here, so a pack's wall lantern supports itself for free.
        let k = block.shape_kind_def();
        if let Some(m) = k.sim.mount(&k.params, self, pos, block) {
            return self.mount_face_complete(m.cell, m.normal);
        }
        let dir = block.support_dir();
        let s = dir.support_cell(pos);
        match dir {
            SupportDir::Below => {
                let ground = self.physics_block(s.x, s.y, s.z);
                // DECLARED beats derived. A row that stated what its floor must
                // look like keeps that same rule once placed, so the gate that
                // let it be placed and the rule that keeps it there cannot
                // disagree. It has to come first: `rests_flat_on_floor` probes
                // octant VOLUMES, so anything with a foot on the floor — a
                // lantern's 8-wide base — reads as lying flat and would take
                // the cover rule instead of its own.
                if block.roots_face() != crate::block::RootsFace::Any {
                    return self.roots_face_ok(block, crate::mathh::IVec3::Y, s, ground);
                }
                if crate::block::rests_flat_on_floor(self, pos, block) {
                    return super::query::full_unit_cube(self.collision_boxes_at(s.x, s.y, s.z));
                }
                ground.is_opaque()
            }
            SupportDir::Above => {
                super::query::full_unit_cube(self.collision_boxes_at(s.x, s.y, s.z))
                    || Block::from_id(self.chunk_block(s.x, s.y, s.z)).support_dir()
                        == SupportDir::Above
            }
            // A WALL holds it exactly when a wall torch would hold: the
            // support's face toward this cell is complete — an opaque cube's,
            // or any shaped face that is geometrically whole (a stair's flat
            // side, a counter's back). The same test the torch/ladder mounts
            // run, reached here through the row's declaration instead of
            // through stored placement state.
            _ => self.mount_face_complete(s, pos - s),
        }
    }

    pub fn roots_face_ok(&self, block: Block, normal: IVec3, s: IVec3, ground: Block) -> bool {
        match block.roots_face() {
            crate::block::RootsFace::Any => true,
            crate::block::RootsFace::FullCube => {
                ground.is_opaque()
                    && crate::block::full_face_at(self, s, normal)
                        == Some(crate::block::FullFace::Cube)
            }
            crate::block::RootsFace::SolidFace => self.mount_face_complete(s, normal),
        }
    }

    pub fn slab_stack_slot_in_hit(
        &self,
        block: Block,
        hit: IVec3,
        rotation: SlabRotation,
        normal: IVec3,
        player_facing: Facing,
    ) -> Option<SlabSlot> {
        if block.shape_family() != ShapeFamily::Slab {
            return None;
        }
        let looked_at = Block::from_id(self.chunk_block(hit.x, hit.y, hit.z));
        if !crate::slab::is_slab(looked_at) {
            return None;
        }
        let slot = crate::slab::stack_slot(rotation, normal, player_facing)?;
        crate::slab::can_add_layer(self.slab_state_at(hit.x, hit.y, hit.z), slot).then_some(slot)
    }
    pub fn finish_single_cell_placement(
        &self,
        block: Block,
        p: IVec3,
        state: ShapeState,
        boxes: &[Aabb],
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        // Substrate + support gate: a block that roots in a particular ground,
        // or hangs from a ceiling, or brackets off a wall, places only when the
        // SUPPORT cell its row declares actually holds it. Blocks with no such
        // rule accept anything. Staying put once placed is the separate job of
        // the FRAGILE behaviour, which reads the same cell.
        if !self.placement_support_ok(block, p) {
            return None;
        }
        let target = Block::from_id(self.chunk_block(p.x, p.y, p.z));
        // Replacing a block with ITSELF (short grass clicked while holding
        // short grass) would rewrite the same state invisibly while still
        // consuming the held item — refuse it like any unplaceable spot.
        if !target.is_replaceable() || target == block {
            return None;
        }
        // A block with no collision box (a torch, grass, a fern, …) traps
        // nothing, so it may be placed inside an entity; a block that WOULD
        // collide cannot be placed where its shape overlaps a gameplay body.
        if occupied(p, boxes) {
            return None;
        }
        Some(PlacementPlan::single(p, block, state))
    }
    pub fn placement_support_ok(&self, block: Block, p: IVec3) -> bool {
        let s = block.support_dir().support_cell(p);
        let ground = self.physics_block(s.x, s.y, s.z);
        if !block.can_root_on(ground) {
            return false;
        }
        if !self.roots_face_ok(block, p - s, s, ground) {
            return false;
        }
        // A fragile row whose support is NOT the ground below has no substrate
        // vocabulary to gate on — `roots_on` names GROUNDS, and this row's
        // support is a ceiling or a wall — so the two rules above accept open
        // air and the FRAGILE tick would shatter the block one tick later,
        // eating the item. Gate on the fragile rule itself, so placement and
        // survival agree by construction (the ladder's rule, which the torch
        // and ladder families already reach through their own pre-gate).
        !(block.is_fragile()
            && block.support_dir() != crate::block::SupportDir::Below
            && !self.fragile_supported(p, block))
    }
}
