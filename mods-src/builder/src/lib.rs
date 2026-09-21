//! Builder: survival schematics.
//!
//! A blueprint in a schematic table names a project: a saved schematic, the
//! ghost a player anchored for it, and the job that builds it. Start admits
//! the job once the chests chained to the table hold every item the site
//! still needs; a Mason Golem then digs itself out beside the table, fetches
//! blocks and tools, clears what is in the way, builds, and burrows home.
//!
//! Everything here composes generic host capabilities: world-held schematics
//! read as construction records, record statuses, container transfers, actor
//! digs and placements, route probes, anchored ghosts and mob tags. Modules:
//!
//! - [`project`] — the persistent project record and its world-KV store.
//! - [`design`] — a schematic compiled into construction units in build order.
//! - [`survey`] — a rolling measurement of those units against the world.
//! - [`supplies`] — the chests chained to a table, and what they hold.
//! - [`jobs`] — per-project session state, the tick, Start/Resume admission,
//!   holds, ghosts.
//! - [`table`] — the table and materials panels and their buttons.
//! - [`worker`] — the golem's work, a step at a time.
//! - [`node`] — the golem's steering brain node.

mod caches;
mod content;
mod design;
mod fx;
mod geometry;
mod golem;
mod jobs;
mod node;
mod project;
mod supplies;
mod survey;
mod table;
mod worker;

use mod_sdk::*;

use jobs::Builder;

const ON_SCHEMATIC_CHOSEN: u32 = 1;
const ON_SCHEMATIC_POSITIONED: u32 = 2;
const ON_ACTOR_ACTED: u32 = 3;
const ON_MOB_DIED: u32 = 4;
const ON_ITEM_USE: u32 = 5;
const ON_INTERACT: u32 = 6;
const ON_ITEM_OBTAINED: u32 = 7;
const ON_BLOCK_PLACED: u32 = 8;
const ON_CONTAINER_OPENED: u32 = 9;

const TICK_JOBS: u32 = 1;
const TICK_PANELS: u32 = 2;

const AI_WORKER: u32 = 1;

#[derive(Default)]
struct BuilderMod {
    state: Option<Builder>,
}

impl Mod for BuilderMod {
    fn init(&mut self) {
        if runtime_side() == RuntimeSide::Client {
            return;
        }
        let Some(content) = content::Content::resolve() else {
            log("builder: pack content failed to resolve; the mod stays idle");
            return;
        };
        self.state = Some(Builder::new(content));
        register_event_handler(EventKind::SchematicChosen, 0, ON_SCHEMATIC_CHOSEN);
        register_event_handler(EventKind::SchematicPositioned, 0, ON_SCHEMATIC_POSITIONED);
        register_event_handler(EventKind::ActorActed, 0, ON_ACTOR_ACTED);
        register_event_handler(EventKind::MobDied, 0, ON_MOB_DIED);
        register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE);
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT);
        register_event_handler(EventKind::ItemObtained, 0, ON_ITEM_OBTAINED);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_ai_node("builder:worker", AI_WORKER);
        // After the mobs move, so the golem is read where it stands this tick.
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_JOBS);
        // After the menu stage, where panels open: a fresh one is filled
        // before anyone sees it.
        register_tick_system(Stage::Menu, AttachSide::After, 0, TICK_PANELS);
    }

    fn tick_system(&mut self, system_id: u32) {
        match (system_id, self.state.as_mut()) {
            (TICK_JOBS, Some(state)) => state.tick(),
            (TICK_PANELS, Some(state)) => state.publish_panels(),
            _ => {}
        }
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        let Some(state) = self.state.as_mut() else {
            return Outcome::Continue;
        };
        match (handler_id, &*payload) {
            (ON_SCHEMATIC_CHOSEN, EventPayload::SchematicChosen { player, tag, asset }) => {
                table::chosen(state, *player, tag, *asset);
            }
            (
                ON_SCHEMATIC_POSITIONED,
                EventPayload::SchematicPositioned {
                    player,
                    tag,
                    asset,
                    origin,
                    turns,
                },
            ) => table::positioned(state, *player, tag, *asset, *origin, *turns),
            (
                ON_ACTOR_ACTED,
                EventPayload::ActorActed {
                    actor: EntityRef::Mob(mob),
                    pos,
                    action,
                    refusal,
                },
            ) => state.acted(*mob, *pos, *action, *refusal),
            (ON_MOB_DIED, EventPayload::MobDied { id, .. }) => state.died(*id),
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block })
                if *block == state.content.table =>
            {
                table::placed(state, *pos);
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { at, .. }) => {
                state.panels.opened(*at);
            }
            (ON_ITEM_OBTAINED, EventPayload::ItemObtained { player, item })
                if Some(*item) == state.content.raw_copper =>
            {
                unlock_recipe(*player, content::COPPER_BLOCK_RECIPE);
            }
            (ON_INTERACT, EventPayload::InteractAttempt { mob: Some(mob), .. }) => {
                return golem::used(state, *mob);
            }
            (ON_ITEM_USE, EventPayload::ItemUsePre { item, .. })
                if *item == state.content.blueprint =>
            {
                return table::use_blueprint(state);
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn gui_click(&mut self, kind_key: &str, widget_id: &str, at: Option<ContainerAddress>) {
        if let Some(state) = self.state.as_mut() {
            table::click(state, kind_key, widget_id, at);
        }
    }

    fn ai_node(&mut self, callback_id: u32, ctx: &AiNodeCtx) -> Option<AiNodeDecision> {
        match callback_id {
            AI_WORKER => node::decide(ctx),
            _ => None,
        }
    }
}

register_mod!(BuilderMod);
