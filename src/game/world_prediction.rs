//! Optimistic client prediction for world edits: the P1 place ghost
//! mirroring the server's `try_place` shape ladder, the full local break, the
//! P0 use-click jab verdict, and the shared body-occupancy check. Tick
//! orchestration (when a click ships, what rides the wire) stays in
//! [`tick`](super::tick); this module owns the prediction logic itself.

use super::prediction;
use super::tick::{GameInput, PlacePrediction, WorldEvent};
use super::Game;
use crate::mathh::IVec3;
use crate::net::protocol::{ClientToServer, PlayerAction};

impl Game {
    /// Full local break prediction at `pos`: clear the replica footprint, latch
    /// hand + world event, open a ledger entry, and queue `BreakFinished`.
    pub(super) fn apply_predicted_break(
        &mut self,
        pos: IVec3,
        expected_block: crate::block::Block,
        normal: Option<IVec3>,
    ) {
        // `predicted` tells the server whether we presented (echo strip): a
        // track-only finish never played sound/burst, so its BlockBroken must
        // still come back over the wire.
        let (request_id, predicted) = if self.prediction.can_predict() {
            match self.replica.clear_broken_block(pos) {
                Some((block, cells)) => {
                    debug_assert_eq!(block, expected_block);
                    let id = self
                        .prediction
                        .begin(prediction::PredictionSnapshot::World {
                            inventory: None,
                            cells: cells.clone(),
                        });
                    self.local_broke_block = Some(block);
                    for (c, _) in &cells {
                        self.predicted_presentation_cells.insert(*c);
                    }
                    // Initial prediction blocks on the complete exact light ->
                    // mesh footprint so the click exposes no stale shading.
                    self.replica.present_predicted_edit(&cells);
                    self.pending_events
                        .world
                        .push(WorldEvent::BlockBroken { pos, block, normal });
                    (id, true)
                }
                // Cell already gone / unbreakable on the replica — still ask
                // the server; track-only so we don't invent a restore.
                None => (self.prediction.begin_track_only(), false),
            }
        } else {
            (self.prediction.begin_track_only(), false)
        };
        // No duration claim rides the wire: the server validates the finish
        // against ITS OWN observed mining window (breaking.rs).
        let tool_item_id = self.self_view.inventory.selected().map(|st| st.item.0);
        self.outbox
            .push(ClientToServer::Action(PlayerAction::BreakFinished {
                request_id,
                pos,
                tool_item_id,
                predicted,
            }));
    }

    /// Whether the client can foresee this use click doing anything: a mob
    /// use/shear target, an interactable block under the crosshair
    /// (non-sneak), or a held item with its own use (food, bucket). Gates the
    /// P0 jab only — the click ships regardless.
    pub(super) fn use_click_predicts_effect(&self, input: &GameInput, use_mob: Option<u64>) -> bool {
        if use_mob.is_some() {
            return true;
        }
        if let Some(stack) = self.self_view.inventory.selected() {
            if stack.item.food().is_some() || stack.item.item_use().is_some() {
                return true;
            }
        }
        let Some(look) = self.look else {
            return false;
        };
        let target = crate::block::Block::from_id(self.replica.chunk_block(
            look.block.x,
            look.block.y,
            look.block.z,
        ));
        !input.movement.sneak && target.interaction() != crate::block::BlockInteraction::None
    }

    /// Optimistic full place when the look target can accept the held block.
    /// Mirrors the placement checks the client CAN evaluate on its replica —
    /// a ghost the server is known to refuse is never drawn — and the server's
    /// per-shape STATE write (torch mount, stair state, log axis, slab layer,
    /// door pair, chest/furnace front), so the mesh built this frame matches
    /// what the authoritative delta will confirm instead of rendering a
    /// default orientation for a frame (SP) or an RTT (MP). Placements the
    /// accept convention denies by design (replace-in-place, slab stack into
    /// the hit cell, an oriented model's shifted base) are never ghosted —
    /// they classify [`PlacePrediction::Plausible`] so the click still jabs.
    /// On predict: cell(s), hotbar decrement, hand pop, and a local
    /// `WorldEvent::BlockPlaced`.
    pub(super) fn try_predict_place_ghost(&mut self, sneak: bool) -> PlacePrediction {
        use crate::block::RenderShape;

        /// The server-mirrored world write a predicted place will commit.
        enum PredictedPlace {
            Bare,
            Torch(crate::torch::TorchPlacement),
            Facing(crate::facing::Facing),
            Log(crate::block_state::LogAxis),
            Stair(crate::block_state::StairState),
            Slab(crate::slab::SlabSlot),
            Door,
            Model(crate::facing::Facing),
        }

        let Some(look) = self.look else {
            return PlacePrediction::No;
        };
        if look.normal == IVec3::ZERO {
            return PlacePrediction::No; // eye inside the cell — the server never places
        }
        let Some(block) = self
            .self_view
            .inventory
            .selected()
            .and_then(|s| s.item.as_block())
        else {
            return PlacePrediction::No;
        };
        // A dual-natured item (both food and placeable — contextual placeable
        // food, e.g. a plantable carrot) resolves place-vs-eat server-side
        // through mod placement rules the replica cannot evaluate. Never
        // ghost it: jab only, and a real placement arrives unpredicted.
        if self
            .self_view
            .inventory
            .selected()
            .is_some_and(|s| s.item.food().is_some())
        {
            return PlacePrediction::Plausible;
        }
        // A MOD-registered block's placement may be governed by mod law the
        // replica cannot evaluate (`block_place_pre` — a crop plants only on
        // farmland). Never ghost one: jab only, and a real placement arrives
        // unpredicted through the authoritative delta. Engine blocks keep
        // full prediction; a mod cancelling THOSE accepts rollback jank.
        if !block.is_engine() {
            return PlacePrediction::Plausible;
        }
        // A non-sneak click on an interactable block opens/uses it instead of
        // placing (the server's interact ladder) — no ghost, or the client
        // would render a phantom block the server never places.
        let target = crate::block::Block::from_id(self.replica.chunk_block(
            look.block.x,
            look.block.y,
            look.block.z,
        ));
        if !sneak && target.interaction() != crate::block::BlockInteraction::None {
            return PlacePrediction::No;
        }
        // Replace-in-place (clicking short grass, a fern…): the server
        // overwrites the CLICKED cell, which can never match the ghost
        // convention (`target + normal`), so the request denies by design —
        // plausible (jab), never ghosted.
        if target.is_replaceable() && target != crate::block::Block::Air {
            return PlacePrediction::Plausible;
        }
        let place_pos = look.block + look.normal;
        let prev = self
            .replica
            .chunk_block(place_pos.x, place_pos.y, place_pos.z);
        if prev != crate::block::Block::Air.0 {
            return PlacePrediction::No;
        }
        let held = self.self_view.inventory.selected().map(|s| s.item);
        let player_facing = crate::server::placement::facing_from_forward(self.player.forward());

        // The shape ladder, mirroring the server's `try_place`: each arm runs
        // the same validity checks against the replica and picks the same
        // world write. `cells` lists every replica cell the write touches —
        // the deny-rollback footprint.
        let mut cells = vec![(place_pos, prev)];
        let write = match block.render_shape() {
            RenderShape::Slab => {
                let rotation = self.held_rotation.slab_rotation(held);
                // A stack lands in the CLICKED cell — off the ghost
                // convention, denied by design. Plausible: jab, no ghost.
                if crate::slab::is_slab(target) {
                    if let Some(slot) =
                        crate::slab::stack_slot(rotation, look.normal, player_facing)
                    {
                        if crate::slab::can_add_layer(
                            self.replica
                                .slab_state_at(look.block.x, look.block.y, look.block.z),
                            slot,
                        ) {
                            return PlacePrediction::Plausible;
                        }
                    }
                }
                let slot = crate::slab::slot_for_rotation(rotation, look.normal, player_facing);
                let Some(state) = self.replica.slab_layer_target_state(place_pos, block, slot)
                else {
                    return PlacePrediction::No;
                };
                if self.placement_blocked_by_body(place_pos, crate::slab::boxes_for_state(state)) {
                    return PlacePrediction::No;
                }
                PredictedPlace::Slab(slot)
            }
            RenderShape::Model(kind) => {
                let multi_cell = crate::block_model::instance(kind).cells.len() > 1;
                if multi_cell || block.directional_view() {
                    // The oriented base anchor usually shifts off the clicked
                    // cell, which the accept convention denies — no ghost.
                    // Still mirror the placement checks so the jab fires only
                    // when the model will actually land.
                    let facing = crate::block_model::def(kind)
                        .orientation
                        .apply(player_facing);
                    let base =
                        crate::block_model::base_from_front_left_anchor(place_pos, kind, facing);
                    if !self
                        .replica
                        .model_footprint_clear_facing(base, kind, facing)
                    {
                        return PlacePrediction::No;
                    }
                    let blocked = crate::block_model::oriented_footprint_cells(base, kind, facing)
                        .into_iter()
                        .any(|(c, off)| {
                            self.placement_blocked_by_body(
                                c,
                                crate::block_model::collision_boxes_oriented(kind, off, facing),
                            )
                        });
                    return if blocked {
                        PlacePrediction::No
                    } else {
                        PlacePrediction::Plausible
                    };
                }
                let facing = crate::block_model::DEFAULT_MODEL_FACING;
                if !self
                    .replica
                    .model_footprint_clear_facing(place_pos, kind, facing)
                {
                    return PlacePrediction::No;
                }
                let blocked = crate::block_model::oriented_footprint_cells(place_pos, kind, facing)
                    .into_iter()
                    .any(|(c, off)| {
                        self.placement_blocked_by_body(
                            c,
                            crate::block_model::collision_boxes_oriented(kind, off, facing),
                        )
                    });
                if blocked {
                    return PlacePrediction::No;
                }
                PredictedPlace::Model(facing)
            }
            RenderShape::Door => {
                if !self.replica.door_footprint_clear(place_pos) {
                    return PlacePrediction::No;
                }
                let upper = place_pos + IVec3::new(0, 1, 0);
                let closed = |top: bool| {
                    crate::door::collision_boxes(crate::door::DoorState {
                        facing: player_facing,
                        open: false,
                        top,
                    })
                };
                if self.placement_blocked_by_body(place_pos, closed(false))
                    || self.placement_blocked_by_body(upper, closed(true))
                {
                    return PlacePrediction::No;
                }
                cells.push((upper, self.replica.chunk_block(upper.x, upper.y, upper.z)));
                PredictedPlace::Door
            }
            RenderShape::Stair => {
                let state = crate::block_state::StairState::new(
                    player_facing,
                    self.held_rotation.stair_half(held),
                );
                if self.placement_blocked_by_body(
                    place_pos,
                    self.replica.resolved_stair_boxes(place_pos, state),
                ) {
                    return PlacePrediction::No;
                }
                PredictedPlace::Stair(state)
            }
            RenderShape::Pane => {
                if self.placement_blocked_by_body(place_pos, self.replica.pane_boxes_at(place_pos))
                {
                    return PlacePrediction::No;
                }
                PredictedPlace::Bare
            }
            _ => {
                // The general path. The client KNOWS these placements fail
                // server-side: unrooted substrate, unsupported torch, or a
                // body in the cell (own body included — no ghost where the
                // player stands).
                let below = self
                    .replica
                    .physics_block(place_pos.x, place_pos.y - 1, place_pos.z);
                if !block.can_root_on(below) {
                    return PlacePrediction::No;
                }
                let write = if block == crate::block::Block::Torch {
                    match crate::torch::TorchPlacement::from_place_normal(look.normal) {
                        Some(tp) if self.replica.torch_supported_at(place_pos, tp) => {
                            PredictedPlace::Torch(tp)
                        }
                        _ => return PlacePrediction::No,
                    }
                } else if block.render_shape() == RenderShape::Ladder {
                    // Mirror the server's wall-mount gate (shape-keyed, like the
                    // server's): a floor/ceiling click, a missing wall face, or a
                    // body overlapping the panel is a KNOWN refusal, so no ghost
                    // and no jab.
                    match crate::facing::Facing::from_horizontal_normal(look.normal) {
                        Some(f)
                            if self.replica.ladder_supported_at(place_pos, f)
                                && !self.placement_blocked_by_body(
                                    place_pos,
                                    crate::ladder::collision_boxes(f),
                                ) =>
                        {
                            PredictedPlace::Facing(f)
                        }
                        _ => return PlacePrediction::No,
                    }
                } else if block.is_log() {
                    PredictedPlace::Log(self.held_rotation.log_axis_for_facing(held, player_facing))
                } else if block.directional_view() {
                    PredictedPlace::Facing(player_facing)
                } else {
                    PredictedPlace::Bare
                };
                if self.placement_blocked_by_body(place_pos, block.collision_boxes()) {
                    return PlacePrediction::No;
                }
                write
            }
        };

        if !self.prediction.can_predict() {
            return PlacePrediction::TrackOnly(self.prediction.begin_track_only());
        }
        let previous_cells = cells.clone();
        let snapshot = prediction::PredictionSnapshot::World {
            inventory: Some(self.self_view.inventory.clone()),
            cells,
        };
        let id = self.prediction.begin(snapshot);
        // The same World write the server commits. Deny-rollback restores the
        // previous block ids, which wipes each cell's sparse state, so a
        // stale predicted state cannot leak.
        match write {
            PredictedPlace::Bare => {
                let _ = self
                    .replica
                    .set_block_world(place_pos.x, place_pos.y, place_pos.z, block);
            }
            PredictedPlace::Torch(tp) => {
                let _ = self
                    .replica
                    .set_block_world(place_pos.x, place_pos.y, place_pos.z, block);
                self.replica.insert_torch(place_pos, tp);
            }
            PredictedPlace::Facing(facing) => {
                let _ = self
                    .replica
                    .set_block_world(place_pos.x, place_pos.y, place_pos.z, block);
                self.replica.insert_entity_facing(place_pos, facing);
            }
            PredictedPlace::Log(axis) => {
                let _ = self.replica.place_log(place_pos, block, axis);
            }
            PredictedPlace::Stair(state) => {
                let _ = self.replica.place_stair(place_pos, block, state);
            }
            PredictedPlace::Slab(slot) => {
                let _ = self.replica.place_slab_layer(place_pos, block, slot);
            }
            PredictedPlace::Door => {
                let _ = self.replica.place_door(place_pos, block, player_facing);
            }
            PredictedPlace::Model(facing) => {
                let _ = self
                    .replica
                    .place_model_block_facing(place_pos, block, facing);
            }
        }
        // Same synchronous prediction presentation as breaking: exact local
        // light and geometry are installed before the ghost is exposed.
        self.replica.present_predicted_edit(&previous_cells);
        self.self_view.inventory.decrement_selected();
        self.place_ghost = Some((place_pos, block.0));
        self.local_placed_block = Some(block);
        self.predicted_presentation_cells.insert(place_pos);
        self.pending_events.world.push(WorldEvent::BlockPlaced {
            pos: place_pos,
            block,
        });
        PlacePrediction::Predicted(id)
    }

    /// Client mirror of the server's `placement_occupied_by_body`: the own
    /// predicted body plus every replicated mob / remote-player row.
    pub(super) fn placement_blocked_by_body(
        &self,
        cell: IVec3,
        boxes: &[crate::block::Aabb],
    ) -> bool {
        if boxes.is_empty() {
            return false; // collisionless blocks (torch, grass) trap nothing
        }
        if self.player.body().overlaps_block_boxes(cell, boxes) {
            return true;
        }
        for entry in self.replicated_mobs.iter() {
            if entry.curr.dead {
                continue;
            }
            let size = crate::mob::def(crate::mob::Mob(entry.curr.kind_id)).size;
            if crate::mob::body_overlaps_block_boxes(
                entry.curr.pos,
                entry.curr.yaw,
                size,
                cell,
                boxes,
            ) {
                return true;
            }
        }
        for p in self.remote_players.iter() {
            let row = &p.curr;
            if !row.visible || !row.alive {
                continue;
            }
            let body = crate::body::Body::new(
                row.transform.pos,
                crate::player::HALF_W,
                crate::player::HEIGHT,
            );
            if body.overlaps_block_boxes(cell, boxes) {
                return true;
            }
        }
        false
    }
}
