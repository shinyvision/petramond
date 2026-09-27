//! Optimistic client prediction for world edits: the P1 place ghost (the
//! SHARED per-shape placement rule `World::placement_plan` evaluated against
//! the replica), the full local break, the P0 use-click verdict, and the
//! body-occupancy gate. Tick orchestration (when a click ships, what rides
//! the wire) stays in [`tick`](super::tick); this module owns the prediction
//! logic itself.
//!
//! The use-click verdict is the SHARED consumer walk
//! (`petramond::rules::use_click`) — the server's own ladder and claim order
//! — with each consumer's turn answered by the SHARED gate
//! (`petramond::rules::item_use`, `builtin_claims_at`) evaluated against the
//! replica and the predicted body. Nothing here restates a server rule.

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

/// The predicted acting body as the shared item-use rules read it: the
/// ACTING hand's stack from the replicated self view (the inventory the
/// server confirms), the predicted spectator state, and the camera's ray.
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

/// The client's consumer registry as the shared walk sees it: every offer
/// PREDICTS the kind's consumer against the replica. Placements claim with
/// their [`PlacePrediction`]; a ghost opens its ledger entry only at the
/// place rung's own turn, so at most one pass ever opens one.
struct ClientConsumers<'a> {
    game: &'a mut Game,
    sneak: bool,
    use_mob: Option<u64>,
}

/// A place rung's verdict: anything but a known refusal claims the click
/// (a `Plausible` placement still jabs).
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
            // The mod `interact_attempt` predictors, dispatched in the
            // registry position the server dispatches the real event.
            ConsumerKind::Registered => game.predict_interact_claim(sneak, use_mob),
            ConsumerKind::Shear => game.predicts_shear(use_mob),
            // Shears aimed at a chest still open it; food opens a door.
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
            // A dual-natured item's placement already had its turn above.
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
    /// The ACTING hand's stack from the replicated self view — the read every
    /// use-click prediction resolves through (`player.acting_hand` is the
    /// dispatch context the two-pass verdict sets; outside a click it is
    /// `Main`, so this is the selected hotbar stack).
    pub(super) fn predicted_held(&self) -> Option<&petramond_world::item::ItemStack> {
        self.replica
            .self_view
            .inventory
            .held_in(self.local.player.acting_hand)
    }

    /// The acting player's snapshot for a client-mod PREDICTION dispatch —
    /// the same `PlayerSnapshot` vocabulary a server handler queries, built
    /// from the client's predicted local player + replicated self view.
    /// `swing` is what this frame's hand triggers say (`Default` = idle; the
    /// frame hook passes the real edges, prediction dispatches need only the
    /// identity).
    pub(super) fn client_actor_snapshot(
        &self,
        sneak: bool,
        swing: mod_api::HandSwing,
    ) -> mod_api::PlayerSnapshot {
        mod_api::PlayerSnapshot {
            // A client has exactly one addressable body, and naming it is
            // what lets a mod use the SAME player-addressed calls its server
            // half does.
            id: Some(mod_api::PlayerId(self.replica.entities.self_id().0)),
            pos: self.local.player.pos.to_array(),
            vel: self.local.player.vel.to_array(),
            yaw: self.local.player.yaw,
            pitch: self.local.player.pitch,
            health: self.replica.self_view.health,
            on_ground: self.local.player.on_ground,
            spectator: self.local.player.is_spectator(),
            sneak,
            // The ACTING hand's stack — the client twin of the server's
            // `Player::held()`: during the off-hand prediction pass the mod
            // predictors see the off-hand item as `held`, exactly like their
            // authoritative halves will.
            held: self
                .predicted_held()
                .map(|st| mod_api::ItemId(st.item.id())),
            held_count: self.predicted_held().map_or(0, |st| st.count),
            // The literal off-hand slot (the client twin of the server's
            // snapshot field), so a predictor sees both hands at once.
            off_held: self
                .replica
                .self_view
                .inventory
                .off_hand()
                .map(|st| mod_api::ItemId(st.item.id())),
            use_held: self.local.intent_use_held,
            // Patched per mod at the dispatch (each sees its own answer).
            holds_use: false,
            pose_anchor: self.replica.entities.own_mount().and_then(|m| match m {
                petramond::net::protocol::PlayerMount::Anchor { pos, .. } => Some(pos.to_array()),
                petramond::net::protocol::PlayerMount::Mob { .. } => None,
            }),
            swing,
            half_width: petramond::player::HALF_W,
            height: petramond::player::HEIGHT,
            eye_height: petramond::player::EYE,
            // The client predicts its own escape from geometry, so this is
            // the predicted body's own answer, like `on_ground` above.
            entombed: self.local.player.entombed(),
            // Condition timers are server-only; replication carries stages alone.
            conditions: Vec::new(),
        }
    }

    /// Ask the client mod instances whether any PREDICTOR claims this
    /// predicted pre event (see `ClientModRuntime::predict_claim`) — the mod
    /// half of prediction parity: a mod consumer is exactly as predictable
    /// as an engine one, through the same event vocabulary the server
    /// dispatches, evaluated against the replica.
    pub(super) fn predict_mod_claim(
        &mut self,
        sneak: bool,
        payload: mod_api::EventPayload,
    ) -> bool {
        let actor = self.client_actor_snapshot(sneak, Default::default());
        self.client_mods.predict_claim(
            &self.replica.world,
            &actor,
            &self.replica.self_view.inventory,
            &payload,
        )
    }

    /// The predicted acting body for the shared item-use rules.
    pub(super) fn predicted_actor(&self) -> PredictedActor<'_> {
        PredictedActor {
            held: self.predicted_held(),
            spectator: self.local.player.is_spectator(),
            eye: self.local.cam.pos,
            dir: self.local.cam.forward(),
        }
    }

    /// Whether the acting hand holds a dual-natured item (food AND
    /// placeable) — the shared split between the contextual-place and
    /// ordinary-place rungs.
    fn holds_contextual_placeable(&self) -> bool {
        self.predicted_held()
            .is_some_and(|st| item_rules::is_contextual_placeable(st.item))
    }

    /// The ONE per-click dispatch of the predicted `interact_attempt` to the
    /// client mod predictors (the registered rung). `true` = a mod consumer
    /// is predicted to claim this click: the jab plays and NO ghost may
    /// appear.
    fn predict_interact_claim(&mut self, sneak: bool, use_mob: Option<u64>) -> bool {
        // A targeted mob BLANKS the block look, so a mob click has no cell —
        // and it is still an attempt (the shared offer rule).
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

    /// Whether the ENGINE's shear consumer will take this mob click: the
    /// shared gate (shears in the acting hand, a coat the species can lose,
    /// still on a live mob) over the replicated row, which carries all of it
    /// (`kind_id`, `dead`, `shorn`).
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

    /// The item-use rung: the mod item-use consumers first (tilling, the
    /// trough/compost fills — the predicted `item_use_pre`, in the registry
    /// position the server runs it), then the engine's own use through the
    /// SHARED rule (`rules::item_use`: bucket fill/pour targets) against the
    /// replica. Mod `block_place_pre` cancels of a pour stay invisible to the
    /// replica — the same over-optimism policy as the engine-block ghost.
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

    /// The whole use click, predicted through the SHARED walk
    /// (`rules::use_click::run_use_click`): the server's registry order and
    /// main/off-hand ladder, each rung answered against the replica. Claims
    /// only the server can see (a mod-cancelled `item_use_pre` or
    /// `interact_attempt` the predictors cannot foresee) arrive through the
    /// `used_unpredicted` echo instead.
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

    /// One MAIN-hand pass of the shared walk, answering only what it placed
    /// (`No` when another rung claimed the click or nothing did) — the place
    /// prediction a production click runs, without the off-hand fallback.
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

    /// Full local break prediction at `pos`: clear the replica footprint, latch
    /// hand + world event, open a ledger entry, and queue `BreakFinished`.
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
                    // Initial prediction blocks on the complete exact light ->
                    // mesh footprint so the click exposes no stale shading.
                    self.replica.world.present_predicted_edit(&cells);
                    self.present_predicted_world_event(WorldEvent::BlockBroken {
                        pos,
                        block,
                        normal,
                        tint: broken_tint,
                    });
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
            return PlacePrediction::No; // eye inside the cell — the server never places
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
        // A click the block's built-in consumer claims never reaches this
        // rung: the shared walk offers it to the built-in rung first, exactly
        // as the server registry does — so a chest/table/furnace click cancels
        // a mod-block ghost exactly like an engine-block ghost.
        let target = petramond_world::block::Block::from_id(self.replica.world.data().chunk_block(
            look.block.x,
            look.block.y,
            look.block.z,
        ));
        // A dual-natured item (both food and placeable — contextual placeable
        // food, e.g. a plantable carrot) resolves place-vs-eat server-side
        // through mod placement rules the replica cannot evaluate. Never
        // ghost it: jab only, and a real placement arrives unpredicted.
        if self
            .predicted_held()
            .is_some_and(|s| s.item.food().is_some())
        {
            return PlacePrediction::Plausible;
        }
        // A MOD-registered block's placement can be governed by mod law
        // (`block_place_pre` — a crop plants only on farmland), so ask the
        // mod's own CLIENT PREDICTOR first: the predicted pre event dispatched
        // against the replica, the same vocabulary the server dispatches. A
        // predicted cancel is a KNOWN refusal (no jab, no ghost). Everything
        // AFTER that gate is deterministic on both sides: a CUSTOM
        // shape runs the server's whole plan+bake pipeline and ghosts the
        // exact write; every other mod block falls through to the shared
        // placement ladder below like an engine block — a pack model block
        // (the furniture chair) gets the same footprint/body-gate refusal
        // prediction a bed or workbench does.
        if !block.is_engine() {
            let looked_at = petramond_world::block::Block::from_id(
                self.replica
                    .world
                    .data()
                    .chunk_block(look.block.x, look.block.y, look.block.z),
            );
            // The server's pre-event position rule (minus slab stacking,
            // which no mod row participates in): the shared build-position
            // rule, so this cannot drift from the authority.
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
                // No reachable owner: the server falls through to the
                // ordinary placement ladder — the shared ghost below does too.
            }
        }
        // Replace-in-place (clicking short grass, a fern…): the server
        // overwrites the CLICKED cell, which can never match the ghost
        // convention (`target + normal`), so the request denies by design —
        // plausible (jab), never ghosted. Replacing a block with ITSELF is a
        // KNOWN refusal (the shared ladder rejects the no-op rewrite), so the
        // click stays silent.
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

        // The SHARED per-shape placement ladder (`World::placement_plan`, the
        // same rule the server evaluates against its world), run against the
        // replica: same validity checks, same state write. Only the
        // body-occupancy answer is client-side (the predicted own body plus
        // the replicated rows).
        let inputs = petramond::world::placement::PlaceInputs {
            hit: look.block,
            normal: look.normal,
            // Through the wire's QUANTIZER, so the ghost resolves from the
            // exact spot the server will read back off the click.
            spot: petramond::net::protocol::TargetRef::of_hit(&look).spot_fraction(),
            // Replace-in-place classified Plausible above, so the build cell
            // is always `hit + normal` here — the ghost convention's cell.
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
        // `cells` lists every replica cell the write touches, with its
        // previous id — the deny-rollback footprint.
        let previous_cells: Vec<(IVec3, u16)> = plan
            .cells()
            .map(|c| (c, self.replica.world.data().chunk_block(c.x, c.y, c.z)))
            .collect();
        let snapshot = prediction::PredictionSnapshot::World {
            inventory: Some(self.replica.self_view.inventory.clone()),
            cells: previous_cells.clone(),
        };
        let id = self.prediction.begin(snapshot);
        // The same World write the server commits (facing only for the engine
        // containers — machine state is server-owned). Deny-rollback restores
        // the previous block ids, which wipes each cell's sparse state, so a
        // stale predicted state cannot leak.
        let _ = self.replica.world.commit_placement(&plan, false);
        self.predict_restore_carry(block, place_pos, plan.anchor_part());
        // Same synchronous prediction presentation as breaking: exact local
        // light and geometry are installed before the ghost is exposed.
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

    /// Full client prediction of a custom shape's placement — the SAME
    /// pipeline the server runs (`try_place_custom_shape`), evaluated against
    /// the replica: the owning mod's placement plan (its client instance — the
    /// plan is deterministic), the SHARED plan validation, the replaceable
    /// gate, and the body-occupancy gate fed by the shape's own sim bake of
    /// the hypothetical cell. A refusal the server will also reach predicts
    /// silent ([`PlacePrediction::No`]); an accepted plan ghosts the exact row
    /// the authoritative delta will confirm — sibling-row orientation
    /// override included. `None` = no reachable owner: the caller falls
    /// through to the ordinary ghost, the server's fall-through twin.
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
        // The replaceable gate (the server's `block_if_loaded` twin): an
        // unread replica cell reads as air — optimistic, and a stale read
        // rolls back like any engine ghost.
        let cur = petramond_world::block::Block::from_id(
            self.replica
                .world
                .data()
                .chunk_block(anchor.x, anchor.y, anchor.z),
        );
        if !cur.is_replaceable() || cur == write_block {
            return Some(PlacePrediction::No);
        }
        // The written row's SUPPORT gate — the server's twin, run against the
        // replica so the ghost refuses exactly what the server will refuse.
        if !self
            .replica
            .world
            .data()
            .placement_support_ok(write_block, anchor)
        {
            return Some(PlacePrediction::No);
        }
        // The body-occupancy gate, fed by the shape's own bake of the
        // hypothetical cell (collision AND render, installed eagerly so the
        // ghost collides and draws exactly from frame 0), falling back to the
        // replica's cached bake, then the row's static collision.
        let (sim_boxes, render_boxes) = {
            // The hypothetical cell's bake input, from the SAME builder the
            // server gate and both pumps use. The neighbours already carry
            // their declared state; the anchor's own is whatever the placement
            // will write (absent before commit, so the ghost bakes from
            // neighbours + the default, then re-bakes on the authoritative
            // state delta).
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
        // The ghost convention: only a plan landing exactly on the build cell
        // ghosts — a shifted anchor arrives unpredicted (jab only).
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
        // A custom shape's placement is single-cell and whole: part 0.
        self.predict_restore_carry(block, anchor, 0);
        // Install the eagerly baked geometry before presentation: the mesher
        // and the local physics read the same boxes the delta will re-bake.
        if let Some(b) = &sim_boxes {
            self.replica.world.set_custom_bake(anchor, b);
        }
        if let Some(b) = render_boxes {
            self.replica.world.set_custom_render_bake(anchor, b);
        }
        // Same synchronous prediction presentation as every ghost: exact
        // local light and geometry are installed before the ghost is exposed.
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

    /// The SHARED body gate (`rules::placement::placement_blocked_by_bodies`,
    /// the server's own) fed the client's bodies: the predicted own body
    /// (the placer always counts), every live replicated mob, and every
    /// remote player in play.
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
        // `visible` is false exactly for spectators and the dead.
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
