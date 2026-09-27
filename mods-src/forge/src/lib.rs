//! Forge: metal is CAST, not assembled.
//!
//! Three pieces, only one has real code.
//!
//! clay.rs - worldgen feature, clay in riverbeds, banks, savanna patches. Data plus one
//! positional field. Only host call: batched biome probe for bank detection.
//!
//! ore.rs - petramond ore, socket-carving gem source, rare veins below cave floor. Purely
//! positional, no host calls.
//!
//! Pottery table - no code. A block row that opens `forge:pottery_table` plus recipe rows
//! naming that station gives you a full crafting bench, engine handles the rest.
//!
//! Recipe `petramond:unlock_on` rows decide when each station shows up.
//!
//! furnace.rs - the forging furnace, a machine you run by hand. Mould in, lever to pour,
//! cast comes out of the basin as an item.
//!
//! Data decides the cast, not code. Mould in the basin carries a `forge:mould` row naming
//! a recipe class; furnace looks up (class, input metal) in the machine-recipe table. No
//! mould means `forge:cast_plate`. Whole metal x mould matrix lives in recipes.json, so
//! another pack adds a mould just by shipping item and processing rows. No `cast_iron_axe`
//! anywhere.

mod anvil;
mod augments;
mod clay;
mod content;
mod furnace;
mod gold;
mod keys;
mod liquid;
mod ore;
mod schema;

use machine_core::Caches;
use mod_sdk::*;

use furnace::ForgingFurnace;

const TICK_SYSTEM: u32 = 1;
const ANVIL_TICK_SYSTEM: u32 = 2;
const ON_BLOCK_PLACED: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 2;
const ON_BLOCK_BREAK: u32 = 4;
const ON_MOB_DAMAGED: u32 = 5;
const GEN_CLAY: u32 = 1;
const GEN_ORE: u32 = 2;

#[derive(Default)]
struct Forge {
    furnace: ForgingFurnace,
    anvil: anvil::Anvil,
    caches: Caches,
    clay: clay::Deposits,
    ore: ore::Ore,
    gold: gold::Gold,
}

impl Mod for Forge {
    fn init(&mut self) {
        self.clay.init();
        register_worldgen_feature(WorldgenStage::Underground, GEN_CLAY, clay::GEN_FILTER);
        self.ore.init();
        register_worldgen_feature(WorldgenStage::Underground, GEN_ORE, ore::GEN_FILTER);

        if runtime_side() != RuntimeSide::Server {
            return;
        }
        self.gold = gold::Gold::resolve();

        let furnace_ok = self.furnace.init();
        if !furnace_ok {
            log("forge: the forging furnace rows are missing; casting stays idle");
        }
        let anvil_ok = self.anvil.init();
        if !anvil_ok {
            log("forge: the anvil rows are missing; augments stay idle");
        }
        if !self.gold.is_empty() || anvil_ok {
            register_event_handler(EventKind::BlockBreakPre, 0, ON_BLOCK_BREAK);
        }
        if anvil_ok {
            register_event_handler(EventKind::MobDamaged, 0, ON_MOB_DAMAGED);
        }
        if !furnace_ok && !anvil_ok {
            return;
        }
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        register_tick_system(Stage::WorldScheduled, AttachSide::After, 0, TICK_SYSTEM);
        register_tick_system(Stage::Menu, AttachSide::After, 0, ANVIL_TICK_SYSTEM);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &mut *payload) {
            (
                ON_BLOCK_BREAK,
                EventPayload::BlockBreakPre {
                    block,
                    harvested,
                    actor: EntityRef::Player(player),
                    drops,
                    ..
                },
            ) => {
                let proc = self.gold.on_block_break(*block, *harvested, *player, drops);
                self.anvil
                    .spec()
                    .wear_held(*player, anvil::WearOn::Break, proc.as_deref());
            }
            (
                ON_MOB_DAMAGED,
                EventPayload::MobDamaged {
                    source: DamageSource::PlayerAttack { id },
                    ..
                },
            ) => {
                self.anvil.spec().wear_held(*id, anvil::WearOn::Hit, None);
            }
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                self.furnace.on_placed(*pos, *block);
                self.anvil.on_placed(*pos, *block);
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { kind, at }) => {
                self.furnace.on_container_opened(kind, *at);
                self.anvil.on_container_opened(kind, *at);
            }
            _ => {}
        }
        Outcome::Continue
    }

    fn gui_click(&mut self, kind_key: &str, widget_id: &str, at: Option<ContainerAddress>) {
        let Some(ContainerAddress::Block(pos)) = at else {
            return;
        };
        if kind_key == keys::FURNACE_GUI && widget_id == keys::WIDGET_LEVER {
            self.furnace.spec().pull_lever(pos, &mut self.caches);
        }
        if kind_key == keys::FURNACE_GUI && widget_id == keys::WIDGET_FITTINGS {
            gui_open(keys::FITTINGS_GUI, Some(pos));
        }
        if kind_key == keys::FITTINGS_GUI {
            if widget_id == keys::WIDGET_BACK {
                gui_open(keys::FURNACE_GUI, Some(pos));
            }
            if let Some(index) = keys::fittings::TABLE
                .iter()
                .position(|fitting| fitting.widget == widget_id)
            {
                if self.furnace.is_present(pos) {
                    self.furnace.spec().fittings.buy(pos, index);
                }
            }
        }
        if kind_key == keys::ANVIL_GUI && widget_id == keys::WIDGET_AUGMENT {
            self.anvil.spec_mut().request_apply(pos);
        }
    }

    fn tick_system(&mut self, system_id: u32) {
        if system_id == TICK_SYSTEM {
            self.furnace.tick(&mut self.caches);
        }
        if system_id == ANVIL_TICK_SYSTEM {
            self.anvil.tick(&mut self.caches);
        }
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> GenOutput {
        (match feature_id {
            GEN_CLAY => self.clay.generate(ctx),
            GEN_ORE => self.ore.generate(ctx),
            _ => Vec::new(),
        })
        .into()
    }
}

register_mod!(Forge);
