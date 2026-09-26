//! Farming mod: wild crop foraging, iron-hoe cultivation on irrigated
//! farmland, four-stage crop growth, and flour/dough/bread processing.
//!
//! Content is pack data (blocks/items/effects/recipes/sounds/bursts); this
//! crate is only the deterministic gameplay logic, split by subsystem:
//!
//! - [`content`] — the pack's registry names resolved to session ids once.
//! - [`attract`] — a planted stand drawing its pest out of the wild (the
//!   carrot patch that brings rabbits), rolled on the crop's own random tick.
//! - [`rest`] — the hour an area sits out attraction after it has produced
//!   a visitor, kept in world KV per 16×16 column.
//! - [`worldgen`] — wild wheat/carrot/potato patches after the Trees stage.
//! - [`claims`] — the click claim chains (item use, interact) and their
//!   order, written once: the server executes the gated plans, the client
//!   instance predicts from the same gates against the replica.
//! - [`tilling`] — the iron hoe turning grass/dirt into farmland.
//! - [`farmland`] — the shared hydration probe (ground water OR overhead
//!   rain via the weather field's `weather:field` channel) + farmland's dry/wet visual
//!   reconciliation (random ticks + neighbor re-arming).
//! - [`crops`] — planting validation, scheduled four-stage growth with dry
//!   pause, right-click harvesting, and supporting-soil invalidation.
//! - [`compost`] — the compost barrel (fill + collect).
//! - [`fertilize`] — the fertilizer target table (fertile farmland,
//!   fertilized grass, the sapling boost) behind one apply sequence.
//! - [`spread`] — fertilized grass spreading its rooted vegetation.
//! - [`forage`] — the rare wheat-seed forage from broken ground cover.
//! - [`follow`] — the wheat lure (a scripted AI node composed onto the
//!   engine sheep through the pack's `brain_extensions` row).
//! - [`husbandry`] — grazing saturation, drinking, love mode, courtship, and
//!   offspring for the breedable species (a sim sweep owning the state
//!   machine plus a steering/posing AI node, both driven by [`content`]
//!   husbandry rows).
//! - [`growth`] — juveniles (the lamb) growing into their adult species when
//!   their `farming:baby` tag is removed (the `mob_tag_removed` hook).
//! - [`wellfed`] — the Well Fed marker effect's damage consequence.
//!
//! Everything mutating runs on the deterministic tick through events, block
//! hooks, and scheduled ticks — no per-tick world sweeps and no whole-world
//! crop list. World reads treat `None` (unloaded / streaming) as "retry
//! later", never as state to act on.

mod attract;
mod claims;
mod compost;
mod content;
mod crops;
mod farmland;
mod fertilize;
mod follow;
mod forage;
mod growth;
mod hemp;
mod hop;
mod husbandry;
mod keys;
mod kv_counter;
mod rest;
mod spread;
mod tilling;
mod trough;
mod wellfed;
mod worldgen;

use mod_sdk::*;
use weather_core::feed::FieldFeed;

use content::Content;
use crops::Growth;

// Event handler ids (stable registration keys, mod-local).
const ON_ITEM_USE_PRE: u32 = 1;
const ON_BLOCK_PLACE_PRE: u32 = 2;
const ON_BLOCK_PLACED: u32 = 3;
const ON_INTERACT_ATTEMPT: u32 = 4;
const ON_PLAYER_DAMAGE_PRE: u32 = 5;
const ON_BLOCK_BROKEN: u32 = 6;
const ON_MOB_TAG_REMOVED: u32 = 7;
/// Mod events: the weather field channel (rain hydrates farmland).
const ON_MOD_EVENT: u32 = 9;

// Block-behavior callback ids.
const HOOK_CROP: u32 = 1;
const HOOK_FARMLAND: u32 = 2;
const HOOK_SPREAD: u32 = 3;

// Worldgen feature id.
const GEN_WILD_PATCHES: u32 = 1;

// AI node callback ids.
const AI_FOLLOW_WHEAT: u32 = 1;
const AI_HUSBANDRY_GOAL: u32 = 2;

// Tick system id.
const TICK_HUSBANDRY: u32 = 1;
const TICK_HOP: u32 = 2;
const TICK_WEATHER_FEED: u32 = 3;

#[derive(Default)]
struct Farming {
    /// Resolved session ids for everything the logic touches. `None` only if
    /// resolution failed (a broken install) — the mod then stays idle instead
    /// of trapping.
    content: Option<Content>,
    /// Running as the CLIENT instance: this side only PREDICTS (the same pre
    /// events, answered against the replica — see [`predict`]).
    client: bool,
    /// Armed growth attempts (crop cell → due tick). Session-scoped by
    /// design: lost scheduling re-arms from random ticks (see [`crops`]).
    growth: Growth,
    /// Areas sitting out crop attraction (see [`rest`]).
    rests: rest::Rests,
    /// The latest weather field heard on its channel. Hydration reads it as
    /// published: every reader runs before core day/night moves the clock,
    /// so the published row IS the sky at the clock those readers see.
    weather: FieldFeed,
}

impl Mod for Farming {
    fn init(&mut self) {
        let Some(content) = Content::resolve() else {
            log("farming: pack content failed to resolve; the mod stays idle");
            return;
        };
        self.content = Some(content);

        if runtime_side() == RuntimeSide::Client {
            // The client instance registers the same pre kinds as PREDICTORS
            // (dispatched speculatively by the engine's jab/ghost prediction);
            // no tick systems, no worldgen, no AI on this side.
            self.client = true;
            register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT_ATTEMPT);
            register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE_PRE);
            register_event_handler(EventKind::BlockPlacePre, 0, ON_BLOCK_PLACE_PRE);
            return;
        }

        register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE_PRE);
        register_event_handler(EventKind::BlockPlacePre, 0, ON_BLOCK_PLACE_PRE);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT_ATTEMPT);
        register_event_handler(EventKind::PlayerDamagePre, 0, ON_PLAYER_DAMAGE_PRE);
        register_event_handler(EventKind::BlockBroken, 0, ON_BLOCK_BROKEN);
        register_event_handler(EventKind::MobTagRemoved, 0, ON_MOB_TAG_REMOVED);
        weather_core::feed::subscribe(ON_MOD_EVENT);
        register_block_behavior(keys::CROP_HOOK, HOOK_CROP);
        register_block_behavior(keys::FARMLAND_HOOK, HOOK_FARMLAND);
        register_block_behavior(keys::SPREAD_HOOK, HOOK_SPREAD);
        register_worldgen_feature(WorldgenStage::Trees, GEN_WILD_PATCHES, worldgen::GEN_FILTER);
        register_ai_node(keys::FOLLOW_WHEAT_NODE, AI_FOLLOW_WHEAT);
        register_ai_node(keys::HUSBANDRY_GOAL_NODE, AI_HUSBANDRY_GOAL);
        // Right after the mobs move, so the sweep measures this tick's
        // positions and its steering tags are in place for the next.
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_HUSBANDRY);
        // The rabbit's hop gait: pack policy over the generic vertical-drive
        // seam, decided from each tick's fresh landings (see `hop`).
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_HOP);
        // Ages the weather feed once per tick, so a weather mod that stops
        // publishing reads as clear sky instead of a frozen storm.
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_WEATHER_FEED);
    }

    fn tick_system(&mut self, system_id: u32) {
        let Some(content) = &self.content else {
            return;
        };
        if system_id == TICK_HUSBANDRY {
            husbandry::on_tick(content);
        }
        if system_id == TICK_HOP {
            hop::on_tick(content);
        }
        if system_id == TICK_WEATHER_FEED {
            self.weather.tick();
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        let Some(content) = &self.content else {
            return Outcome::Continue;
        };
        if self.client {
            // Prediction dispatch: the same events through the SAME claim
            // chains and gates (`claims`), against the replica — a gated
            // plan is a predicted claim, never executed.
            let replica = SideWorld::Replica;
            return match (handler_id, &*payload) {
                (
                    ON_INTERACT_ATTEMPT,
                    EventPayload::InteractAttempt {
                        block: Some(pos), ..
                    },
                ) => claims::interact(content, &replica, *pos, |_| Outcome::Cancel),
                (ON_ITEM_USE_PRE, EventPayload::ItemUsePre { item, target }) => {
                    claims::item_use(content, &replica, *item, *target, |_, _| Outcome::Cancel)
                }
                (ON_BLOCK_PLACE_PRE, EventPayload::BlockPlacePre { pos, block, .. }) => {
                    crops::predict_place_pre(content, *pos, *block)
                }
                _ => Outcome::Continue,
            };
        }
        if handler_id == ON_MOD_EVENT {
            self.weather.observe(payload);
            return Outcome::Continue;
        }
        let sky = self.weather.params();
        match (handler_id, &mut *payload) {
            (ON_ITEM_USE_PRE, EventPayload::ItemUsePre { item, target, .. }) => {
                let item = *item;
                claims::item_use(content, &SideWorld::Server, item, *target, |pos, plan| {
                    plan.execute(content, sky.as_ref(), item, pos)
                })
            }
            (ON_BLOCK_PLACE_PRE, EventPayload::BlockPlacePre { pos, block, .. }) => {
                crops::on_place_pre(content, *pos, *block)
            }
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                crops::on_placed(content, &mut self.growth, *pos, *block);
                farmland::on_block_placed_above(content, *pos, *block);
                Outcome::Continue
            }
            (
                ON_INTERACT_ATTEMPT,
                EventPayload::InteractAttempt {
                    block: Some(pos), ..
                },
            ) => {
                let pos = *pos;
                let growth = &mut self.growth;
                claims::interact(content, &SideWorld::Server, pos, |plan| {
                    plan.execute(content, growth, pos)
                })
            }
            (ON_PLAYER_DAMAGE_PRE, EventPayload::PlayerDamagePre { amount, .. }) => {
                wellfed::on_player_damage(amount);
                Outcome::Continue
            }
            (
                ON_BLOCK_BROKEN,
                EventPayload::BlockBroken {
                    pos,
                    block,
                    harvested,
                    natural,
                },
            ) => {
                crops::on_block_broken(content, *pos, *block, *harvested);
                forage::on_block_broken(content, *pos, *block, *natural);
                hemp::on_block_broken(content, *pos, *block, *natural);
                Outcome::Continue
            }
            (
                ON_MOB_TAG_REMOVED,
                EventPayload::MobTagRemoved {
                    mob_id, kind, key, ..
                },
            ) => {
                growth::on_tag_removed(content, *mob_id, *kind, key);
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn block_hook(&mut self, callback_id: u32, kind: BlockHookKind, pos: [i32; 3]) {
        let Some(content) = &self.content else {
            return;
        };
        let sky = self.weather.params();
        match callback_id {
            HOOK_CROP => crops::on_hook(
                content,
                &mut self.growth,
                &mut self.rests,
                sky.as_ref(),
                kind,
                pos,
            ),
            HOOK_FARMLAND => farmland::on_hook(content, sky.as_ref(), kind, pos),
            HOOK_SPREAD => spread::on_hook(content, kind, pos),
            _ => {}
        }
    }

    fn ai_node(&mut self, callback_id: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        let content = self.content.as_ref()?;
        match callback_id {
            AI_FOLLOW_WHEAT => follow::decide(content, ctx),
            AI_HUSBANDRY_GOAL => husbandry::decide(ctx),
            _ => None,
        }
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        let Some(content) = &self.content else {
            return GenOutput::default();
        };
        match feature_id {
            GEN_WILD_PATCHES => worldgen::wild_patches(content, ctx).into(),
            _ => GenOutput::default(),
        }
    }
}

register_mod!(Farming);
