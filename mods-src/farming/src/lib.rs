//! Farming mod: wild crop foraging, iron-hoe cultivation on irrigated
//! farmland, four-stage crop growth, and flour/dough/bread processing.
//!
//! Content is pack data (blocks/items/effects/recipes/sounds/bursts); this
//! crate is only the deterministic gameplay logic, split by subsystem:
//!
//! - [`content`] — the pack's registry names resolved to session ids once.
//! - [`worldgen`] — wild wheat/carrot/potato patches after the Trees stage.
//! - [`tilling`] — the iron hoe turning grass/dirt into farmland.
//! - [`farmland`] — the shared hydration probe (ground water OR overhead
//!   rain via the `weather:field` interop row) + farmland's dry/wet visual
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

mod compost;
mod content;
mod crops;
mod farmland;
mod fertilize;
mod follow;
mod forage;
mod growth;
mod husbandry;
mod kv_counter;
mod spread;
mod tilling;
mod trough;
mod wellfed;
mod worldgen;

use mod_sdk::*;

use content::Content;
use crops::Growth;

/// First-Cancel-wins handler composition: run `next` only while the event is
/// still live. Every multi-link dispatch below chains through this.
fn chain(first: Outcome, next: impl FnOnce() -> Outcome) -> Outcome {
    match first {
        Outcome::Cancel => Outcome::Cancel,
        Outcome::Continue => next(),
    }
}

// Event handler ids (stable registration keys, mod-local).
const ON_ITEM_USE_PRE: u32 = 1;
const ON_BLOCK_PLACE_PRE: u32 = 2;
const ON_BLOCK_PLACED: u32 = 3;
const ON_BLOCK_INTERACT: u32 = 4;
const ON_PLAYER_DAMAGE_PRE: u32 = 5;
const ON_BLOCK_BROKEN: u32 = 6;
const ON_MOB_TAG_REMOVED: u32 = 7;

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

#[derive(Default)]
struct Farming {
    /// Resolved session ids for everything the logic touches. `None` only if
    /// resolution failed (a broken install) — the mod then stays idle instead
    /// of trapping.
    content: Option<Content>,
    /// Armed growth attempts (crop cell → due tick). Session-scoped by
    /// design: lost scheduling re-arms from random ticks (see [`crops`]).
    growth: Growth,
}

impl Mod for Farming {
    fn init(&mut self) {
        let Some(content) = Content::resolve() else {
            log("farming: pack content failed to resolve; the mod stays idle");
            return;
        };
        self.content = Some(content);

        register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE_PRE);
        register_event_handler(EventKind::BlockPlacePre, 0, ON_BLOCK_PLACE_PRE);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::BlockInteract, 0, ON_BLOCK_INTERACT);
        register_event_handler(EventKind::PlayerDamagePre, 0, ON_PLAYER_DAMAGE_PRE);
        register_event_handler(EventKind::BlockBroken, 0, ON_BLOCK_BROKEN);
        register_event_handler(EventKind::MobTagRemoved, 0, ON_MOB_TAG_REMOVED);
        register_block_behavior("farming:crop", HOOK_CROP);
        register_block_behavior("farming:farmland", HOOK_FARMLAND);
        register_block_behavior("farming:grass_fertilized", HOOK_SPREAD);
        register_worldgen_feature(WorldgenStage::Trees, GEN_WILD_PATCHES);
        register_ai_node("farming:follow_wheat", AI_FOLLOW_WHEAT);
        register_ai_node("farming:husbandry_goal", AI_HUSBANDRY_GOAL);
        // Right after the mobs move, so the sweep measures this tick's
        // positions and its steering tags are in place for the next.
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_HUSBANDRY);
    }

    fn tick_system(&mut self, system_id: u32) {
        let Some(content) = &self.content else {
            return;
        };
        if system_id == TICK_HUSBANDRY {
            husbandry::on_tick(content);
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        let Some(content) = &self.content else {
            return Outcome::Continue;
        };
        match (handler_id, &mut *payload) {
            (ON_ITEM_USE_PRE, EventPayload::ItemUsePre { item, target }) => {
                // The hoe first (it consumes eligible clicks), then the
                // fertilizer targets, then the compostable barrel fill, then
                // the water trough bucket swap — each falls through quietly
                // when the held item is not its business.
                let first = chain(tilling::on_item_use(content, *item, *target), || {
                    fertilize::on_item_use(content, *item, *target)
                });
                let second = chain(first, || compost::on_item_use(content, *item, *target));
                chain(second, || trough::on_item_use(content, *item, *target))
            }
            (ON_BLOCK_PLACE_PRE, EventPayload::BlockPlacePre { pos, block, .. }) => {
                crops::on_place_pre(content, *pos, *block)
            }
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                crops::on_placed(content, &mut self.growth, *pos, *block);
                farmland::on_block_placed_above(content, *pos, *block);
                Outcome::Continue
            }
            (ON_BLOCK_INTERACT, EventPayload::BlockInteract { pos, block, item }) => chain(
                crops::on_interact(content, &mut self.growth, *pos, *block, *item),
                || compost::on_interact(content, *pos, *block),
            ),
            (ON_PLAYER_DAMAGE_PRE, EventPayload::PlayerDamagePre { amount, .. }) => {
                wellfed::on_player_damage(amount);
                Outcome::Continue
            }
            (
                ON_BLOCK_BROKEN,
                EventPayload::BlockBroken {
                    pos,
                    block,
                    natural,
                    ..
                },
            ) => {
                forage::on_block_broken(content, *pos, *block, *natural);
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
        match callback_id {
            HOOK_CROP => crops::on_hook(content, &mut self.growth, kind, pos),
            HOOK_FARMLAND => farmland::on_hook(content, kind, pos),
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

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> Vec<GenWrite> {
        let Some(content) = &self.content else {
            return Vec::new();
        };
        match feature_id {
            GEN_WILD_PATCHES => worldgen::wild_patches(content, ctx),
            _ => Vec::new(),
        }
    }
}

register_mod!(Farming);
