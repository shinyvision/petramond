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

use crate::block::{Aabb, Block, ShapeFamily};
use crate::block_state::{LogAxis, StairHalf, StairState};
use crate::facing::Facing;
use crate::mathh::IVec3;
use crate::slab::{SlabRotation, SlabSlot};
use crate::torch::TorchPlacement;

use super::store::World;

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
    pub stair_half: StairHalf,
    pub slab_rotation: SlabRotation,
    /// The held log rotation already resolved against `player_facing`.
    pub log_axis: LogAxis,
}

/// The per-shape state write a validated placement will commit.
pub(crate) enum PlacementWrite {
    /// A plain block id write (also panes: their connections carry no state).
    Cube,
    /// A directional-view cube (chest, furnace): block id + front facing.
    DirectionalCube(Facing),
    Torch(TorchPlacement),
    /// A ladder-shaped wall panel: the facing selects the sibling BLOCK ROW
    /// to commit (`Block::wall_panel_row` — facing is block identity, no
    /// per-cell state).
    WallPanel(Facing),
    Log(LogAxis),
    Stair(StairState),
    Slab(SlabSlot),
    Door(Facing),
    Model(Facing),
    /// A Layer-3 custom shape's placement (the shape's own WASM
    /// `shape_placement_plan` decided orientation): a plain single-cell id write,
    /// exactly like [`Cube`](Self::Cube). Placement is stateless — a custom cell
    /// carries no per-cell KV, so there is no new section map to write.
    Custom { block_id: u8 },
}

/// A validated placement: the cell it anchors on (the commit target — the
/// clicked cell for a slab stack, the oriented base for a model, the lower
/// cell for a door), every cell the write touches (the prediction ledger /
/// rollback footprint), and the state write itself.
pub(crate) struct PlacementPlan {
    pub anchor: IVec3,
    pub cells: Vec<IVec3>,
    pub write: PlacementWrite,
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
        let p = inputs.place_pos;
        match block.shape_family() {
            ShapeFamily::Slab => {
                // A stack lands in the CLICKED cell when the clicked face
                // fronts the half it would fill; otherwise a fresh layer
                // builds into the adjacent cell.
                let (target, slot) = match self.slab_stack_slot_in_hit(
                    block,
                    inputs.hit,
                    inputs.slab_rotation,
                    inputs.normal,
                    inputs.player_facing,
                ) {
                    Some(slot) => (inputs.hit, slot),
                    None => (
                        p,
                        crate::slab::slot_for_rotation(
                            inputs.slab_rotation,
                            inputs.normal,
                            inputs.player_facing,
                        ),
                    ),
                };
                let target_block = Block::from_id(self.chunk_block(target.x, target.y, target.z));
                if !crate::slab::is_slab(target_block) && !self.placement_cell_open(target) {
                    return None;
                }
                let next = self.slab_layer_target_state(target, block, slot)?;
                if occupied(target, crate::slab::boxes_for_state(next)) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: target,
                    cells: vec![target],
                    write: PlacementWrite::Slab(slot),
                })
            }
            // A bbmodel block places its WHOLE footprint: every occupied cell
            // must be loaded + replaceable AND clear of blocking bodies, or
            // the placement fails as a unit. Multi-cell models, and models
            // marked directionalView, are oriented from the player's facing
            // through the model's own placement orientation; the anchor
            // shifts to the oriented base.
            ShapeFamily::Model => {
                let kind = block.model_kind().expect("model family carries a model kind");
                let oriented =
                    block.directional_view() || crate::block_model::instance(kind).cells.len() > 1;
                let facing = if oriented {
                    crate::block_model::def(kind)
                        .orientation
                        .apply(inputs.player_facing)
                } else {
                    crate::block_model::DEFAULT_MODEL_FACING
                };
                let base = if oriented {
                    crate::block_model::base_from_front_left_anchor(p, kind, facing)
                } else {
                    p
                };
                if !self.model_footprint_clear_facing(base, kind, facing) {
                    return None;
                }
                let footprint = crate::block_model::oriented_footprint_cells(base, kind, facing);
                if footprint.iter().any(|&(c, off)| {
                    occupied(
                        c,
                        crate::block_model::collision_boxes_oriented(kind, off, facing),
                    )
                }) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: base,
                    cells: footprint.into_iter().map(|(c, _)| c).collect(),
                    write: PlacementWrite::Model(facing),
                })
            }
            // A door is a 2-tall thin block: both cells must be loaded +
            // replaceable with a floor to stand on, and the closed slab must
            // not trap a body. It sits on the edge nearest the placer.
            ShapeFamily::Door => {
                if !self.door_footprint_clear(p) {
                    return None;
                }
                let upper = p + IVec3::new(0, 1, 0);
                let closed = |top: bool| {
                    crate::door::collision_boxes(crate::door::DoorState {
                        facing: inputs.player_facing,
                        open: false,
                        top,
                    })
                };
                if occupied(p, closed(false)) || occupied(upper, closed(true)) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: p,
                    cells: vec![p, upper],
                    write: PlacementWrite::Door(inputs.player_facing),
                })
            }
            ShapeFamily::Stair => {
                let state = StairState::new(inputs.player_facing, inputs.stair_half);
                if !self.placement_cell_open(p) {
                    return None;
                }
                if occupied(p, self.resolved_stair_boxes(p, state)) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: p,
                    cells: vec![p],
                    write: PlacementWrite::Stair(state),
                })
            }
            // A pane occupies only its resolved post + arms, so the overlap
            // gate tests those thin boxes. No stored state: connections are
            // re-resolved from neighbours wherever the shape is read.
            // A connection shape occupies only its resolved post + arms, so the
            // overlap gate tests those thin boxes — from the BLOCK's own params,
            // since the cell is still empty (a placed shape reads its own params
            // via the collision facet). No stored state: connections re-resolve
            // from neighbours wherever the shape is read.
            ShapeFamily::Pane => {
                if !self.placement_cell_open(p) {
                    return None;
                }
                let c = block
                    .shape_kind_def()
                    .params
                    .connection()
                    .expect("pane carries connection params");
                if occupied(p, self.connection_boxes_at(p, c, ShapeFamily::Pane)) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: p,
                    cells: vec![p],
                    write: PlacementWrite::Cube,
                })
            }
            ShapeFamily::Fence => {
                if !self.placement_cell_open(p) {
                    return None;
                }
                let c = block
                    .shape_kind_def()
                    .params
                    .connection()
                    .expect("fence carries connection params");
                if occupied(p, self.connection_boxes_at(p, c, ShapeFamily::Fence)) {
                    return None;
                }
                Some(PlacementPlan {
                    anchor: p,
                    cells: vec![p],
                    write: PlacementWrite::Cube,
                })
            }
            _ => self.general_placement_plan(block, inputs, occupied),
        }
    }

    /// The general (single-cell, non-shape-branched) arm of the ladder:
    /// torch/ladder mount gates, the substrate gate, replaceability, and the
    /// body gate against the block's own collision boxes.
    fn general_placement_plan(
        &self,
        block: Block,
        inputs: &PlaceInputs,
        occupied: &mut dyn FnMut(IVec3, &[Aabb]) -> bool,
    ) -> Option<PlacementPlan> {
        let p = inputs.place_pos;
        // A torch-shaped block only mounts on a floor or wall (never a
        // ceiling) and needs a usable support face. When REPLACING a plant it
        // always drops to the FLOOR of that cell — right-clicking grass from
        // any angle stands a floor torch where the grass was. Keyed on the
        // shape, not the engine block: a pack row declaring the torch shape
        // gets the same mount rule.
        let write = if block.shape_family() == ShapeFamily::Torch {
            let tp = if inputs.replacing_in_place {
                TorchPlacement::Floor
            } else {
                TorchPlacement::from_place_normal(inputs.normal)?
            };
            if !self.torch_supported_at(p, tp) {
                return None;
            }
            PlacementWrite::Torch(tp)
        } else if block.shape_family() == ShapeFamily::Ladder {
            // A ladder-shaped block only mounts on a vertical wall face and
            // needs a complete face behind its panel. The clicked face's
            // normal names the panel front even when replacing a plant in
            // place. The panel is real collision, so like every colliding
            // shape it may not be placed inside a gameplay body.
            let facing = Facing::from_horizontal_normal(inputs.normal)?;
            if !self.ladder_supported_at(p, facing) {
                return None;
            }
            let (t, h) = block.ladder_dims();
            if occupied(p, crate::ladder::collision_boxes_dim(facing, t, h)) {
                return None;
            }
            PlacementWrite::WallPanel(facing)
        } else if block.is_log() {
            PlacementWrite::Log(inputs.log_axis)
        } else if block.directional_view() {
            PlacementWrite::DirectionalCube(inputs.player_facing)
        } else {
            PlacementWrite::Cube
        };
        // Substrate gate: a block that roots in a particular ground places
        // only when the cell directly below is a ground it accepts. Blocks
        // with no such rule accept anything; a torch is gated by its own
        // support-face check above. Staying put once placed is the separate
        // job of the FRAGILE behaviour.
        let below = self.physics_block(p.x, p.y - 1, p.z);
        if !block.can_root_on(below) {
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
        if occupied(p, block.collision_boxes()) {
            return None;
        }
        Some(PlacementPlan {
            anchor: p,
            cells: vec![p],
            write,
        })
    }

    /// Commit a validated plan's world write — the same write on both sides,
    /// which is what keeps a predicted ghost's mesh identical to the
    /// authoritative delta that confirms it. `with_block_entities` is true on
    /// the authoritative world (a placed chest/furnace gets its empty machine
    /// state at once); the client replica passes false and records the front
    /// facing only — container/furnace machine state is server-owned and
    /// arrives with the delta.
    pub(crate) fn commit_placement(
        &mut self,
        block: Block,
        plan: &PlacementPlan,
        with_block_entities: bool,
    ) -> bool {
        let a = plan.anchor;
        match &plan.write {
            PlacementWrite::Cube => self.set_block_world(a.x, a.y, a.z, block),
            PlacementWrite::DirectionalCube(facing) => {
                if !self.set_block_world(a.x, a.y, a.z, block) {
                    return false;
                }
                if with_block_entities {
                    // The authoritative side fabricates the block-entity for
                    // the engine containers; other directional cubes keep no
                    // stored facing (matching the pre-split server ladder).
                    if block == Block::Furnace {
                        self.insert_furnace(a, *facing);
                    } else if block == Block::Chest {
                        self.insert_chest(a, *facing);
                    }
                } else {
                    self.insert_entity_facing(a, *facing);
                }
                true
            }
            PlacementWrite::Torch(tp) => {
                if !self.set_block_world(a.x, a.y, a.z, block) {
                    return false;
                }
                self.insert_torch(a, *tp);
                true
            }
            PlacementWrite::WallPanel(facing) => {
                // The facing IS the block row (the sapling-stage pattern):
                // commit the sibling row whose panel fronts the clicked
                // normal. One plain id write — nothing enters the
                // entity-facing map, so a ladder never classifies its
                // section as a block-entity section.
                self.set_block_world(a.x, a.y, a.z, block.wall_panel_row(*facing))
            }
            PlacementWrite::Log(axis) => self.place_log(a, block, *axis),
            PlacementWrite::Stair(state) => self.place_stair(a, block, *state),
            PlacementWrite::Slab(slot) => self.place_slab_layer(a, block, *slot),
            PlacementWrite::Door(facing) => self.place_door(a, block, *facing),
            PlacementWrite::Model(facing) => self.place_model_block_facing(a, block, *facing),
            PlacementWrite::Custom { block_id } => {
                // Single-cell stateless custom placement: a plain id write at the
                // anchor (re-dirties the cell's bake), exactly like Cube.
                self.set_block_world(a.x, a.y, a.z, Block::from_id(*block_id))
            }
        }
    }
}
