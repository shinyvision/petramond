//! Optimistic client prediction for world edits: the P1 place ghost (the
//! SHARED per-shape placement rule `World::placement_plan` evaluated against
//! the replica), the full local break, the P0 use-click jab verdict, and the
//! client-side body-occupancy check. Tick orchestration (when a click ships,
//! what rides the wire) stays in [`tick`](super::tick); this module owns the
//! prediction logic itself.

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
    pub(super) fn use_click_predicts_effect(
        &self,
        input: &GameInput,
        use_mob: Option<u64>,
    ) -> bool {
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
    /// Runs the SAME per-shape placement rule the server runs
    /// (`World::placement_plan`) against the replica — a ghost the server is
    /// known to refuse is never drawn — and commits the same per-shape STATE
    /// write (torch mount, stair state, log axis, slab layer, door pair,
    /// chest/furnace front), so the mesh built this frame matches what the
    /// authoritative delta will confirm instead of rendering a default
    /// orientation for a frame (SP) or an RTT (MP). Placements the
    /// accept convention denies by design (replace-in-place, slab stack into
    /// the hit cell, an oriented model's shifted base) are never ghosted —
    /// they classify [`PlacePrediction::Plausible`] so the click still jabs.
    /// On predict: cell(s), hotbar decrement, hand pop, and a local
    /// `WorldEvent::BlockPlaced`.
    pub(super) fn try_predict_place_ghost(&mut self, sneak: bool) -> PlacePrediction {
        use crate::block::RenderShape;

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
        // plausible (jab), never ghosted. Replacing a block with ITSELF is a
        // KNOWN refusal (the shared ladder rejects the no-op rewrite), so the
        // click stays silent.
        if target.is_replaceable() && target != crate::block::Block::Air {
            return if target == block {
                PlacePrediction::No
            } else {
                PlacePrediction::Plausible
            };
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

        // The SHARED per-shape placement ladder (`World::placement_plan`, the
        // same rule the server evaluates against its world), run against the
        // replica: same validity checks, same state write. Only the
        // body-occupancy answer is client-side (the predicted own body plus
        // the replicated rows).
        let inputs = crate::world::placement::PlaceInputs {
            hit: look.block,
            normal: look.normal,
            // Replace-in-place classified Plausible above, so the build cell
            // is always `hit + normal` here — the ghost convention's cell.
            place_pos,
            replacing_in_place: false,
            player_facing,
            stair_half: self.held_rotation.stair_half(held),
            slab_rotation: self.held_rotation.slab_rotation(held),
            log_axis: self.held_rotation.log_axis_for_facing(held, player_facing),
        };
        let plan = self
            .replica
            .placement_plan(block, &inputs, &mut |cell, boxes| {
                self.placement_blocked_by_body(cell, boxes)
            });
        let Some(plan) = plan else {
            // A KNOWN refusal (unrooted substrate, unsupported mount, blocked
            // footprint, a body in the cell — own body included): no ghost
            // and no jab.
            return PlacePrediction::No;
        };
        // Placements the accept convention denies by design (accept ⇔ landed
        // exactly at `target + normal`) are never ghosted: an anchor off the
        // build cell (a slab stack into the hit cell, an oriented model's
        // shifted base) — and oriented/multi-cell models even when the base
        // happens to coincide. Plausible: the jab fires (the click WILL
        // place), the request ships unpredicted.
        let oriented_model = match block.render_shape() {
            RenderShape::Model(kind) => {
                block.directional_view() || crate::block_model::instance(kind).cells.len() > 1
            }
            _ => false,
        };
        if plan.anchor != place_pos || oriented_model {
            return PlacePrediction::Plausible;
        }

        if !self.prediction.can_predict() {
            return PlacePrediction::TrackOnly(self.prediction.begin_track_only());
        }
        // `cells` lists every replica cell the write touches, with its
        // previous id — the deny-rollback footprint.
        let previous_cells: Vec<(IVec3, u8)> = plan
            .cells
            .iter()
            .map(|&c| (c, self.replica.chunk_block(c.x, c.y, c.z)))
            .collect();
        let snapshot = prediction::PredictionSnapshot::World {
            inventory: Some(self.self_view.inventory.clone()),
            cells: previous_cells.clone(),
        };
        let id = self.prediction.begin(snapshot);
        // The same World write the server commits (facing only for the engine
        // containers — machine state is server-owned). Deny-rollback restores
        // the previous block ids, which wipes each cell's sparse state, so a
        // stale predicted state cannot leak.
        let _ = self.replica.commit_placement(block, &plan, false);
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
