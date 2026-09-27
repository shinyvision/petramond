//! Farming mod: foraging, hoe + irrigated farmland, four-stage crops, flour/dough/bread.
//!
//! Pack has all the content data; this crate is just gameplay logic. One module per piece:
//! [`content`], [`attract`], [`rest`], [`worldgen`], [`claims`], [`tilling`], [`farmland`],
//! [`crops`], [`compost`], [`fertilize`], [`spread`], [`forage`], [`follow`], [`husbandry`],
//! [`growth`], [`wellfed`].
//!
//! Mutation only ever happens on tick, from events, block hooks, or scheduled ticks. No per-tick
//! sweeps, no whole-world crop list. A `None` world read just means retry later, not real state.

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

const ON_ITEM_USE_PRE: u32 = 1;
const ON_BLOCK_PLACE_PRE: u32 = 2;
const ON_BLOCK_PLACED: u32 = 3;
const ON_INTERACT_ATTEMPT: u32 = 4;
const ON_PLAYER_DAMAGE_PRE: u32 = 5;
const ON_BLOCK_BROKEN: u32 = 6;
const ON_MOB_TAG_REMOVED: u32 = 7;
const ON_MOD_EVENT: u32 = 9;

const HOOK_CROP: u32 = 1;
const HOOK_FARMLAND: u32 = 2;
const HOOK_SPREAD: u32 = 3;

const GEN_WILD_PATCHES: u32 = 1;

const AI_FOLLOW_WHEAT: u32 = 1;
const AI_HUSBANDRY_GOAL: u32 = 2;

const TICK_HUSBANDRY: u32 = 1;
const TICK_HOP: u32 = 2;
const TICK_WEATHER_FEED: u32 = 3;

#[derive(Default)]
struct Farming {
    content: Option<Content>,
    client: bool,
    growth: Growth,
    rests: rest::Rests,
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
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_HUSBANDRY);
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_HOP);
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
