use super::prediction;
use super::tick::{PlacePrediction, WorldEvent};
use super::Game;
use petramond::net::protocol::{ClientToServer, PlayerAction};
use petramond::rules::interact::ConsumerKind;
use petramond::rules::item_use::{self as item_rules, ActorView};
use petramond::rules::use_click::{self, Claim, UseClickConsumers, UseClickOutcome};
use petramond_math::math::{IVec3, Vec3};
use petramond_math::world_pos::WorldPos;
use petramond_world::inventory::Hand;
use petramond_world::item::ItemStack;

pub(super) struct PredictedActor<'a> {
    held: Option<&'a ItemStack>,
    spectator: bool,
    eye: WorldPos,
    dir: Vec3,
}

impl ActorView for PredictedActor<'_> {
    fn held(&self) -> Option<&ItemStack> {
        self.held
    }

    fn is_spectator(&self) -> bool {
        self.spectator
    }

    fn eye(&self) -> WorldPos {
        self.eye
    }

    fn look_dir(&self) -> Vec3 {
        self.dir
    }
}

struct ClientConsumers<'a> {
    game: &'a mut Game,
    sneak: bool,
    use_mob: Option<u64>,
}

fn place_claim(place: PlacePrediction) -> Claim<PlacePrediction> {
    match place {
        PlacePrediction::No => Claim::Pass,
        place => Claim::Placed(place),
    }
}

impl UseClickConsumers for ClientConsumers<'_> {
    type Placement = PlacePrediction;

    fn off_hand_occupied(&self) -> bool {
        self.game.replica.self_view.inventory.off_hand().is_some()
    }

    fn set_acting_hand(&mut self, hand: Hand) {
        self.game.local.player.acting_hand = hand;
    }

    fn offer(&mut self, kind: ConsumerKind) -> Claim<PlacePrediction> {
        let (sneak, use_mob) = (self.sneak, self.use_mob);
        let game = &mut *self.game;
        let claimed = match kind {
            ConsumerKind::Registered => game.predict_interact_claim(sneak, use_mob),
            ConsumerKind::Shear => game.predicts_shear(use_mob),
            ConsumerKind::BuiltinBlock => game.local.look.is_some_and(|l| {
                use_click::builtin_claims_at(game.replica.world.data(), l.block, sneak)
            }),
            ConsumerKind::ContextualPlace => {
                return if game.holds_contextual_placeable() {
                    place_claim(game.try_predict_place_ghost(sneak))
                } else {
                    Claim::Pass
                };
            }
            ConsumerKind::Eat => item_rules::eat_claims(&game.predicted_actor()),
            ConsumerKind::ItemUse => game.predicts_item_use(sneak),
            ConsumerKind::Place => {
                return if game.holds_contextual_placeable() {
                    Claim::Pass
                } else {
                    place_claim(game.try_predict_place_ghost(sneak))
                };
            }
        };
        if claimed {
            Claim::Claimed
        } else {
            Claim::Pass
        }
    }
}

impl Game {
    pub(super) fn predicted_held(&self) -> Option<&petramond_world::item::ItemStack> {
        self.replica
            .self_view
            .inventory
            .held_in(self.local.player.acting_hand)
    }

    pub(super) fn client_actor_snapshot(
        &self,
        sneak: bool,
        swing: mod_api::HandSwing,
    ) -> mod_api::PlayerSnapshot {
        mod_api::PlayerSnapshot {
            id: Some(mod_api::PlayerId(self.replica.entities.self_id().0)),
            pos: self.local.player.pos.to_array(),
            vel: self.local.player.vel.to_array(),
            yaw: self.local.player.yaw,
            pitch: self.local.player.pitch,
            health: self.replica.self_view.health,
            on_ground: self.local.player.on_ground,
            spectator: self.local.player.is_spectator(),
            sneak,
            held: self
                .predicted_held()
                .map(|st| mod_api::ItemId(st.item.id())),
            held_count: self.predicted_held().map_or(0, |st| st.count),
            off_held: self
                .replica
                .self_view
                .inventory
                .off_hand()
                .map(|st| mod_api::ItemId(st.item.id())),
            use_held: self.local.intent_use_held,
            holds_use: false,
            pose_anchor: self.replica.entities.own_mount().and_then(|m| match m {
                petramond::net::protocol::PlayerMount::Anchor { pos, .. } => Some(pos.to_array()),
                petramond::net::protocol::PlayerMount::Mob { .. } => None,
            }),
            swing,
            half_width: petramond::player::HALF_W,
            height: petramond::player::HEIGHT,
            eye_height: petramond::player::EYE,
            entombed: self.local.player.entombed(),
            conditions: Vec::new(),
        }
    }

    pub(super) fn predict_mod_claim(
        &mut self,
        sneak: bool,
        payload: mod_api::EventPayload,
    ) -> bool {
        let actor = self.client_actor_snapshot(sneak, Default::default());
        let claimed = self.client_mods.predict_claim(
            &self.replica.world,
            &actor,
            &self.replica.self_view.inventory,
            &payload,
        );
        self.queue_client_mod_events();
        claimed
    }

    pub(super) fn predicted_actor(&self) -> PredictedActor<'_> {
        PredictedActor {
            held: self.predicted_held(),
            spectator: self.local.player.is_spectator(),
            eye: self.local.cam.pos,
            dir: self.local.cam.forward(),
        }
    }

    fn holds_contextual_placeable(&self) -> bool {
        self.predicted_held()
            .is_some_and(|st| item_rules::is_contextual_placeable(st.item))
    }

    fn predict_interact_claim(&mut self, sneak: bool, use_mob: Option<u64>) -> bool {
        if !use_click::registered_offered(self.local.look.map(|l| l.block), use_mob) {
            return false;
        }
        let payload = mod_api::EventPayload::InteractAttempt {
            block: self.local.look.map(|l| l.block.to_array()),
            face: self.local.look.map(|l| l.normal.to_array()),
            mob: use_mob,
            player: mod_api::PlayerId(self.replica.entities.self_id().0),
        };
        self.predict_mod_claim(sneak, payload)
    }

    fn predicts_shear(&self, use_mob: Option<u64>) -> bool {
        let Some(id) = use_mob else {
            return false;
        };
        if !item_rules::holds_shears(&self.predicted_actor()) {
            return false;
        }
        self.replica.entities.mobs().iter().any(|e| {
            e.curr.id == id
                && item_rules::can_shear_coat(
                    petramond::mob::Mob(e.curr.kind_id),
                    e.curr.dead,
                    e.curr.shorn,
                )
        })
    }

    fn predicts_item_use(&mut self, sneak: bool) -> bool {
        let Some(item) = self.predicted_held().map(|st| st.item) else {
            return false;
        };
        let payload = mod_api::EventPayload::ItemUsePre {
            item: mod_api::ItemId(item.id()),
            target: self.local.look.map(|l| l.block.to_array()),
        };
        if self.predict_mod_claim(sneak, payload) {
            return true;
        }
        item_rules::engine_item_use_claims(&self.predicted_actor(), self.replica.world.data())
    }

    pub(super) fn predict_use_click(
        &mut self,
        sneak: bool,
        use_mob: Option<u64>,
    ) -> UseClickOutcome<PlacePrediction> {
        use_click::run_use_click(&mut ClientConsumers {
            game: self,
            sneak,
            use_mob,
        })
    }

    #[cfg(test)]
    pub(super) fn predict_main_hand_place(&mut self, sneak: bool) -> PlacePrediction {
        self.local.player.acting_hand = Hand::Main;
        let walked = use_click::walk_consumers(&mut ClientConsumers {
            game: self,
            sneak,
            use_mob: None,
        });
        match walked {
            Some((_, Claim::Placed(place))) => place,
            _ => PlacePrediction::No,
        }
    }

    pub(super) fn apply_predicted_break(
        &mut self,
        pos: IVec3,
        expected_block: petramond_world::block::Block,
        normal: Option<IVec3>,
    ) {
        // `predicted` tells the server whether we presented (echo strip): a
        // track-only finish never played sound/burst, so its BlockBroken must
        // still come back over the wire.
        // Read the cell's tint BEFORE the clear wipes its KV — the local
        // burst event needs it (same capture the server does).
        let broken_tint = self.replica.world.data().cell_burst_tint(pos);
        let (request_id, predicted) = if self.prediction.can_predict() {
            match self.replica.world.clear_broken_block(pos) {
                Some((block, cells)) => {
                    debug_assert_eq!(block, expected_block);
                    let id = self
                        .prediction
                        .begin(prediction::PredictionSnapshot::World {
                            inventory: None,
                            cells: cells.clone(),
                        });
                    self.hand.latch_break(block);
                    for (c, _) in &cells {
                        self.prediction.mark_presented(*c);
                    }
                    self.replica.world.present_predicted_edit(&cells);
                    self.present_predicted_world_event(WorldEvent::BlockBroken {
                        pos,
                        block,
                        normal,
                        tint: broken_tint,
                    });
                    (id, true)
                }
                None => (self.prediction.begin_track_only(), false),
            }
        } else {
            (self.prediction.begin_track_only(), false)
        };
        let tool_item_id = self
            .replica
            .self_view
            .inventory
            .selected()
            .map(|st| st.item.0);
        self.net
            .queue(ClientToServer::Action(PlayerAction::BreakFinished {
                request_id,
                pos,
                tool_item_id,
                predicted,
            }));
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
    ///
    /// Predicted-place mirror of the server's carry restore: copy the held
    /// stack's `petramond:carry` instance-data entries into REPLICA cell KV at
    /// the anchor, so a carried tint renders the frame the ghost appears
    /// instead of flashing plain until the server's KV delta lands (which
    /// overwrites this with the authoritative value; a deny-rollback's block
    /// restore wipes it like any block write).
    fn predict_restore_carry(
        &mut self,
        block: petramond_world::block::Block,
        anchor: IVec3,
        part: petramond_world::block::CellPart,
    ) {
        let carry = block.carry();
        if carry.is_empty() {
            return;
        }
        let Some(held) = self.predicted_held() else {
            return;
        };
        let Some(map) = petramond_world::item::variant::get(held.variant) else {
            return;
        };
        for &key in carry {
            if let Some(v) = map.get(key) {
                self.replica.world.cell_kv_set(
                    anchor.x,
                    anchor.y,
                    anchor.z,
                    petramond_world::block::part_kv_key(key, part),
                    v.clone(),
                );
            }
        }
    }

    pub(super) fn try_predict_place_ghost(&mut self, sneak: bool) -> PlacePrediction {
        let Some(look) = self.local.look else {
            return PlacePrediction::No;
        };
        if look.normal == IVec3::ZERO {
            return PlacePrediction::No;
        }
        let Some(block) = self
            .predicted_held()
            .filter(|s| !s.item.creative_only() || self.creative_mode())
            .and_then(|s| s.item.as_block())
        else {
            return PlacePrediction::No;
        };
        if self
            .predicted_held()
            .is_some_and(|s| !s.item.placement_variants().is_empty())
        {
            return PlacePrediction::Plausible;
        }
        let target = petramond_world::block::Block::from_id(self.replica.world.data().chunk_block(
            look.block.x,
            look.block.y,
            look.block.z,
        ));
        // A dual-natured item (food and placeable, like a plantable carrot) picks place or eat
        // on the server, through mod rules the replica can't evaluate. Jab only, never ghost.
        if self
            .predicted_held()
            .is_some_and(|s| s.item.food().is_some())
        {
            return PlacePrediction::Plausible;
        }
        // Mod blocks can gate placement via `block_place_pre` (a crop needs farmland), so ask
        // the mod's client predictor first with the same event the server dispatches. A predicted
        // cancel is a known refusal: no jab, no ghost. Past that gate both sides are deterministic.
        // Custom shapes run the full plan and bake pipeline and ghost the exact write; other mod
        // blocks fall through to the shared ladder like engine blocks.
        if !block.is_engine() {
            let looked_at = petramond_world::block::Block::from_id(
                self.replica
                    .world
                    .data()
                    .chunk_block(look.block.x, look.block.y, look.block.z),
            );
            let pre_pos =
                petramond::world::placement::build_position(looked_at, look.block, look.normal);
            let facing =
                petramond::rules::placement::facing_from_forward(self.local.player.forward());
            let payload = mod_api::EventPayload::BlockPlacePre {
                pos: pre_pos.to_array(),
                block: mod_api::BlockId(block.id()),
                facing: match facing {
                    petramond_math::facing::Facing::North => mod_api::Facing::North,
                    petramond_math::facing::Facing::South => mod_api::Facing::South,
                    petramond_math::facing::Facing::West => mod_api::Facing::West,
                    petramond_math::facing::Facing::East => mod_api::Facing::East,
                },
                actor: mod_api::EntityRef::Player(mod_api::PlayerId(
                    self.replica.entities.self_id().0,
                )),
            };
            if self.predict_mod_claim(sneak, payload) {
                return PlacePrediction::No;
            }
            if block.is_custom_shape() {
                if let Some(prediction) = self.try_predict_custom_place(sneak, block, look, pre_pos)
                {
                    return prediction;
                }
            }
        }
        if petramond::world::placement::replaces_in_place(target) {
            return if target == block {
                PlacePrediction::No
            } else {
                PlacePrediction::Plausible
            };
        }
        let place_pos = look.block + look.normal;
        let prev = self
            .replica
            .world
            .data()
            .chunk_block(place_pos.x, place_pos.y, place_pos.z);
        if prev != petramond_world::block::Block::Air.0 {
            return PlacePrediction::No;
        }
        let held = self.predicted_held().map(|s| s.item);
        let player_facing =
            petramond::rules::placement::facing_from_forward(self.local.player.forward());

        let inputs = petramond::world::placement::PlaceInputs {
            hit: look.block,
            normal: look.normal,
            spot: petramond::net::protocol::TargetRef::of_hit(&look).spot_fraction(),
            place_pos,
            replacing_in_place: false,
            player_facing,
            held_rotation: self.local.held_rotation.clone(),
            held,
        };
        let plan = self
            .replica
            .world
            .placement_plan(block, &inputs, &mut |cell, boxes| {
                self.placement_blocked_by_body(cell, boxes)
            });
        let Some(plan) = plan else {
            return PlacePrediction::No;
        };
        // Placements the accept convention denies by design (accept ⇔ landed
        // exactly at `target + normal`) are never ghosted: an anchor off the
        // build cell (a slab stack into the hit cell, an oriented model's
        // shifted base) — and oriented/multi-cell models even when the base
        // happens to coincide. Plausible: the jab fires (the click WILL
        // place), the request ships unpredicted.
        let oriented_model = match block.model_kind() {
            Some(kind) => {
                block.directional_view()
                    || petramond_world::block_model::instance(kind).cells.len() > 1
            }
            None => false,
        };
        if plan.anchor != place_pos || oriented_model {
            return PlacePrediction::Plausible;
        }

        if !self.prediction.can_predict() {
            return PlacePrediction::TrackOnly(self.prediction.begin_track_only());
        }
        let previous_cells: Vec<(IVec3, u16)> = plan
            .cells()
            .map(|c| (c, self.replica.world.data().chunk_block(c.x, c.y, c.z)))
            .collect();
        let snapshot = prediction::PredictionSnapshot::World {
            inventory: Some(self.replica.self_view.inventory.clone()),
            cells: previous_cells.clone(),
        };
        let id = self.prediction.begin(snapshot);
        let _ = self.replica.world.commit_placement(&plan, false);
        self.predict_restore_carry(block, place_pos, plan.anchor_part());
        self.replica.world.present_predicted_edit(&previous_cells);
        let hand = self.local.player.acting_hand;
        if !self.local.player.is_creative() {
            self.replica.self_view.inventory.decrement_held(hand);
        }
        self.hand.latch_place(block, hand);
        self.prediction.mark_presented(place_pos);
        self.present_predicted_world_event(WorldEvent::BlockPlaced {
            pos: place_pos,
            block,
        });
        PlacePrediction::Predicted(id)
    }

    /// Same steps the server uses in `try_place_custom_shape`: owner's plan,
    /// shared validation, replaceable gate, body-occupancy gate from the
    /// shape's sim bake. Refused is a silent `PlacePrediction::No`. Accepted
    /// ghosts the exact row, sibling-row orientation and all. `None` is no
    /// reachable owner; fall back to the ordinary ghost.
    fn try_predict_custom_place(
        &mut self,
        sneak: bool,
        block: petramond_world::block::Block,
        look: petramond::player::RaycastHit,
        place_pos: IVec3,
    ) -> Option<PlacePrediction> {
        let shape_key = block.shape_kind().key();
        let shape_kind = block.shape_kind().0;
        let view = mod_api::PlaceInputsView {
            hit: look.block.to_array(),
            normal: look.normal.to_array(),
            place_pos: place_pos.to_array(),
            player_facing: petramond::rules::placement::facing_from_forward(
                self.local.player.forward(),
            ) as u8,
        };
        let actor = self.client_actor_snapshot(sneak, Default::default());
        let result = self.client_mods.placement_plan(
            &self.replica.world,
            &actor,
            shape_key,
            shape_kind,
            block.id(),
            view,
        )?;
        if !result.accepted {
            return Some(PlacePrediction::No);
        }
        let Some((anchor, write_block)) = petramond::world::placement::validate_custom_plan(
            &result, block, shape_kind, place_pos,
        ) else {
            return Some(PlacePrediction::No);
        };
        let cur = petramond_world::block::Block::from_id(
            self.replica
                .world
                .data()
                .chunk_block(anchor.x, anchor.y, anchor.z),
        );
        if !cur.is_replaceable() || cur == write_block {
            return Some(PlacePrediction::No);
        }
        if !self
            .replica
            .world
            .data()
            .placement_support_ok(write_block, anchor)
        {
            return Some(PlacePrediction::No);
        }
        let (sim_boxes, render_boxes) = {
            let input = self
                .replica
                .world
                .data()
                .bake_cell_input(anchor, write_block);
            let Self {
                client_mods,
                replica,
                ..
            } = self;
            client_mods.bake_placement_geometry(&replica.world, shape_key, shape_kind, input)
        };
        let boxes: &[petramond_world::block::Aabb] = match &sim_boxes {
            Some(b) => b,
            None => self
                .replica
                .world
                .data()
                .custom_shape_boxes(anchor)
                .unwrap_or_else(|| write_block.collision_boxes()),
        };
        if self.placement_blocked_by_body(anchor, boxes) {
            return Some(PlacePrediction::No);
        }
        if anchor != place_pos {
            return Some(PlacePrediction::Plausible);
        }
        if !self.prediction.can_predict() {
            return Some(PlacePrediction::TrackOnly(
                self.prediction.begin_track_only(),
            ));
        }
        let plan = petramond::world::placement::PlacementPlan::single(
            anchor,
            write_block,
            petramond_world::block::ShapeState::NONE,
        );
        let previous_cells = vec![(anchor, cur.id())];
        let snapshot = prediction::PredictionSnapshot::World {
            inventory: Some(self.replica.self_view.inventory.clone()),
            cells: previous_cells.clone(),
        };
        let id = self.prediction.begin(snapshot);
        let _ = self.replica.world.commit_placement(&plan, false);
        self.predict_restore_carry(block, anchor, 0);
        if let Some(b) = &sim_boxes {
            self.replica.world.set_custom_bake(anchor, b);
        }
        if let Some(b) = render_boxes {
            self.replica.world.set_custom_render_bake(anchor, b);
        }
        self.replica.world.present_predicted_edit(&previous_cells);
        let hand = self.local.player.acting_hand;
        if !self.local.player.is_creative() {
            self.replica.self_view.inventory.decrement_held(hand);
        }
        self.hand.latch_place(write_block, hand);
        self.prediction.mark_presented(anchor);
        self.present_predicted_world_event(WorldEvent::BlockPlaced {
            pos: anchor,
            block: write_block,
        });
        Some(PlacePrediction::Predicted(id))
    }

    pub(super) fn placement_blocked_by_body(
        &self,
        cell: IVec3,
        boxes: &[petramond_world::block::Aabb],
    ) -> bool {
        use petramond::rules::placement::{placement_blocked_by_bodies, player_occupies, Occupant};
        let own = std::iter::once(Occupant::Player {
            feet: self.local.player.pos,
        });
        let mobs = self
            .replica
            .entities
            .mobs()
            .iter()
            .filter(|e| !e.curr.dead)
            .map(|e| Occupant::Mob {
                pos: e.curr.pos,
                yaw: e.curr.yaw,
                kind: petramond::mob::Mob(e.curr.kind_id),
            });
        let players = self
            .replica
            .entities
            .players()
            .iter()
            .filter(|p| player_occupies(false, p.curr.alive, !p.curr.visible))
            .map(|p| Occupant::Player {
                feet: p.curr.transform.pos,
            });
        placement_blocked_by_bodies(cell, boxes, own.chain(mobs).chain(players))
    }
}
