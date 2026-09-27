mod caches;
mod content;
mod design;
mod fx;
mod geometry;
mod golem;
mod host;
mod jobs;
mod keys;
mod node;
mod project;
mod supplies;
mod survey;
mod table;
#[cfg(test)]
mod testing;
mod ui;
mod worker;

use crate::host::prelude::*;

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
        register_ai_node(keys::WORKER_NODE, AI_WORKER);
        register_tick_system(Stage::Mobs, AttachSide::After, 0, TICK_JOBS);
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
                unlock_recipe(*player, keys::COPPER_BLOCK_RECIPE);
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
