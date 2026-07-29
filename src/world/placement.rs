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

use crate::block::{Aabb, Block, CellPart, ShapeFamily, ShapeState};
use crate::facing::Facing;
use crate::mathh::IVec3;
use crate::slab::{SlabRotation, SlabSlot};

use super::store::World;

/// Whether a click on `looked_at` REPLACES it where it stands instead of
/// building against its face: replaceable MATTER — tall grass, a snow layer,
/// water (so a bucket pours in place) — which air, being nothing, is not.
pub(crate) fn replaces_in_place(looked_at: Block) -> bool {
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
pub(crate) fn build_position(looked_at: Block, hit: IVec3, normal: IVec3) -> IVec3 {
    if replaces_in_place(looked_at) {
        hit
    } else {
        hit + normal
    }
}

/// Player-derived inputs to the placement rules, resolved by each side from
/// its own session / held-rotation state before the ladder runs.
pub(crate) struct PlaceInputs {
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
    pub held_rotation: crate::server::player::HeldRotation,
    pub held: Option<crate::item::ItemType>,
}

/// One cell a [`PlacementPlan`] writes: the block row and the opaque initial
/// cell-state bytes, plus whether the write claims the WHOLE cell or just one
/// of its parts.
#[derive(Copy, Clone)]
pub(crate) struct CellWrite {
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
pub(crate) struct PlacementPlan {
    pub anchor: IVec3,
    pub writes: Vec<CellWrite>,
}

impl PlacementPlan {
    /// The common single-cell plan: a whole-cell write of the cell's one part.
    pub(crate) fn single(cell: IVec3, block: Block, state: ShapeState) -> Self {
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
    pub(crate) fn single_part(
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
    pub(crate) fn whole(cell: IVec3, block: Block, state: ShapeState) -> CellWrite {
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
    pub(crate) fn cells(&self) -> impl Iterator<Item = IVec3> + '_ {
        self.writes.iter().map(|w| w.cell)
    }

    /// The part the ANCHOR write claims — what the carry courier restores
    /// into.
    pub(crate) fn anchor_part(&self) -> CellPart {
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
pub(crate) enum PlacementOutcome {
    /// The click cannot place here (no floor for a door, a slab cell already
    /// full, a body in the way).
    Refused,
    /// This family has no bespoke placement: use the generic single-cell path
    /// ([`World::general_placement_plan`]) — cube/log/directional blocks,
    /// plants, and any family that never overrides.
    General,
    /// A fully-resolved placement.
    Plan(PlacementPlan),
}

/// The placement seam every shape family implements. The engine holds NO
/// per-family placement dispatch: `World::placement_plan` asks the cell's
/// shape kind, and a mod family answers exactly as an engine one does.
pub(crate) trait ShapePlacement: Send + Sync + 'static {
    /// Resolve a placement of `block` for this click, or defer. Reads the
    /// world for support/occupancy through `w`; `occupied` reports whether a
    /// gameplay body overlaps the given boxes at a cell (side-specific — the
    /// server's sessions+mobs, the client's predicted+replicated bodies).
    fn placement_plan(
        &self,
        _w: &World,
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
pub(crate) fn validate_custom_plan(
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

impl World {
    /// The slot a held slab would stack INTO the clicked cell, or `None` when
    /// the click builds a fresh layer into the adjacent cell instead. Shared
    /// by the placement rule and the server's `block_place_pre` position (the
    /// event announces the cell the commit will actually target).
    pub(crate) fn slab_stack_slot_in_hit(
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

    /// The placement ladder: whether `block` can be placed at all for this
    /// click, and if so which state write lands where. `None` is a refused
    /// spot — the click neither places nor consumes the held item. `occupied`
    /// answers whether a gameplay body overlaps the given boxes at a cell;
    /// collisionless shapes (torch, plants) pass empty boxes and trap nothing.
    pub(crate) fn placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        // The family owns its placement — the engine holds no per-family
        // placement dispatch. A family either resolves the plan, refuses, or
        // defers to the generic single-cell path.
        match block
            .shape_kind_def()
            .placement
            .placement_plan(self, block, inputs, occupied)
        {
            PlacementOutcome::Plan(plan) => Some(plan),
            PlacementOutcome::Refused => None,
            PlacementOutcome::General => self.general_placement_plan(block, inputs, occupied),
        }
    }

    /// The generic single-cell placement path: the write a plain cube / log /
    /// directional / plant block commits, gated by substrate, replaceability,
    /// and body occupancy. Every family that does not override its placement
    /// (and the torch / ladder families, which pre-gate then delegate here)
    /// reaches this.
    pub(crate) fn general_placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        let state = if block.is_log() {
            // The log's own reading of the held rotation; the default
            // vertical axis stays stateless (the codec elides it).
            let axis = inputs
                .held_rotation
                .log_axis_for_facing(inputs.held, inputs.player_facing);
            crate::block::CellCodec::to_cell(&axis)
        } else if block.directional_view() {
            crate::block::CellCodec::to_cell(&crate::block_state::EntityFront(inputs.player_facing))
        } else {
            ShapeState::NONE
        };
        self.finish_single_cell_placement(
            block,
            inputs.place_pos,
            state,
            block.collision_boxes(),
            occupied,
        )
    }

    /// Whether the support at `s` presents the face shape `block`'s row
    /// demands, `normal` pointing from the support toward the placed cell.
    /// The support FAMILY answers the geometry (`ShapeSim::full_face`), so no
    /// family is named here; the opaque test is the same material rule a wall
    /// mount applies to a cube face, and is what keeps air, glass and leaves
    /// out of the accept set.
    fn roots_face_ok(&self, block: Block, normal: IVec3, s: IVec3, ground: Block) -> bool {
        match block.roots_face() {
            crate::block::RootsFace::Any => true,
            crate::block::RootsFace::FullCube => {
                ground.is_opaque()
                    && crate::block::full_face_at(self, s, normal)
                        == Some(crate::block::FullFace::Cube)
            }
        }
    }

    /// The shared single-cell placement tail: substrate gate, replaceability,
    /// and the body-occupancy gate against `boxes`. The generic path and the
    /// torch / ladder families (which compute their own write + pre-gate)
    /// finish here, so the gate rules live in exactly one place.
    pub(crate) fn finish_single_cell_placement(
        &self,
        block: Block,
        p: IVec3,
        state: ShapeState,
        boxes: &[Aabb],
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        // Substrate gate: a block that roots in a particular ground places
        // only when its SUPPORT cell — the one its row declares, below for a
        // plant and above for a hanging block — holds a ground it accepts.
        // Blocks with no such rule accept anything. Staying put once placed is
        // the separate job of the FRAGILE behaviour, which reads the same cell.
        let s = block.support_dir().support_cell(p);
        let ground = self.physics_block(s.x, s.y, s.z);
        if !block.can_root_on(ground) {
            return None;
        }
        // …and the SHAPE half of the same gate: the support's face toward this
        // cell. The direction falls out of the declared support cell, so a
        // hanging row reads its ceiling's underside with no second rule.
        if !self.roots_face_ok(block, p - s, s, ground) {
            return None;
        }
        // A HANGING fragile row has no substrate vocabulary to gate on:
        // `roots_on` names GROUNDS and this row's support is a ceiling, so the
        // two rules above accept open air and the FRAGILE tick would shatter
        // the block one tick later — eating the item. Gate it on the fragile
        // rule itself, so placement and survival agree by construction (the
        // ladder's rule, which the torch and ladder families already reach
        // through their own pre-gate).
        if block.is_fragile()
            && block.support_dir() == crate::block::SupportDir::Above
            && !self.fragile_supported(p, block)
        {
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

    /// Commit a validated plan's world write — ONE generic path for every
    /// family, the same write on both sides, which is what keeps a predicted
    /// ghost's mesh identical to the authoritative delta that confirms it.
    /// Blocks + states land raw across the whole footprint first (the region
    /// is consistent before any announce), then one region refresh relights,
    /// remeshes, announces, and runs the refine cascade.
    /// `with_block_entities` is true on the authoritative world (a placed
    /// engine container fabricates its empty machine state at once); the
    /// client replica passes false — container/furnace machine state is
    /// server-owned and arrives with the delta. That fabrication is
    /// block-ENTITY vocabulary (see `world::container`), not shape knowledge.
    pub(crate) fn commit_placement(
        &mut self,
        plan: &PlacementPlan,
        with_block_entities: bool,
    ) -> bool {
        for w in &plan.writes {
            if !self.materialize_section_at(w.cell) {
                return false;
            }
        }
        let mut cells = Vec::with_capacity(plan.writes.len());
        for &CellWrite {
            cell: c,
            block: b,
            state,
            augments,
            ..
        } in &plan.writes
        {
            let Some((section, lx, ly, lz)) = self.chunk_at_world_mut(c.x, c.y, c.z) else {
                return false;
            };
            // An AUGMENTING write adds a part to a cell that already holds
            // others, so their data has to survive `set_block`'s wholesale KV
            // clear (see `CellWrite::augments`). Detach + re-attach around the
            // write; the courier then overwrites just the claimed part's keys.
            let kept = augments.then(|| section.cell_kv_take(lx, ly, lz)).flatten();
            section.set_block(lx, ly, lz, b);
            if let Some(map) = kept {
                section.cell_kv_restore(lx, ly, lz, map);
            }
            if !state.is_empty() {
                section.set_cell_state(lx, ly, lz, state);
            }
            section.modified = true;
            cells.push(c);
        }
        for &CellWrite {
            cell: c,
            block: b,
            state,
            ..
        } in &plan.writes
        {
            // The WASM-bake fan-out + deep-visibility invalidation every
            // block write owes (the cube path had them via `set_block_world`;
            // the old per-family commits skipped them).
            self.mark_custom_bake_edit(c.x, c.y, c.z, b);
            if b.directional_view() {
                self.note_block_entity_change(c);
            }
            if with_block_entities {
                let facing =
                    <crate::block_state::EntityFront as crate::block::CellView>::from_cell(state).0;
                if b == Block::Furnace {
                    self.insert_furnace(c, facing);
                } else if b == Block::Chest {
                    self.insert_chest(c, facing);
                }
            }
        }
        self.terrain.vis_dirty = true;
        self.refresh_region(&cells);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guest_plan(
        anchor: IVec3,
        cells: &[IVec3],
        block: Option<Block>,
    ) -> mod_api::ShapePlacementResult {
        mod_api::ShapePlacementResult {
            accepted: true,
            anchor: anchor.to_array(),
            cells: cells.iter().map(|c| c.to_array()).collect(),
            block: block.map(|b| mod_api::BlockId(b.id())),
        }
    }

    #[test]
    fn custom_plan_validation_gates_the_guests_answer() {
        // Two plain cubes share one shape-kind row; a ladder is another kind.
        let held = Block::Stone;
        let kind = held.shape_kind().0;
        assert_eq!(Block::Dirt.shape_kind().0, kind);
        assert_ne!(Block::Ladder.shape_kind().0, kind);
        let pp = IVec3::new(10, 64, -3);

        // The default writes the held row at the anchor; an empty `cells`
        // claims just the anchor.
        let (anchor, write) =
            validate_custom_plan(&guest_plan(pp, &[], None), held, kind, pp).expect("held write");
        assert_eq!((anchor, write), (pp, held));
        // A sibling row of the SAME shape kind is a legal orientation override.
        let (_, write) =
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Dirt)), held, kind, pp)
                .expect("sibling override");
        assert_eq!(write, Block::Dirt);
        // A row of a DIFFERENT shape kind can never be written — a plan may
        // not reach outside its own variant family.
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp], Some(Block::Ladder)), held, kind, pp)
                .is_none()
        );
        // A wider footprint than the anchor cell is refused, as is a `cells`
        // list naming a different cell.
        let other = pp + IVec3::new(0, 1, 0);
        assert!(
            validate_custom_plan(&guest_plan(pp, &[pp, other], None), held, kind, pp).is_none()
        );
        assert!(validate_custom_plan(&guest_plan(pp, &[other], None), held, kind, pp).is_none());
        // The anchor is bounded to the click's neighbourhood (Chebyshev ≤ 2).
        let near = pp + IVec3::new(2, -2, 0);
        assert!(validate_custom_plan(&guest_plan(near, &[near], None), held, kind, pp).is_some());
        let far = pp + IVec3::new(3, 0, 0);
        assert!(validate_custom_plan(&guest_plan(far, &[far], None), held, kind, pp).is_none());
    }
}
