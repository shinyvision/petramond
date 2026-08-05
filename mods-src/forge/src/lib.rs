//! Forge: metal is CAST, not assembled.
//!
//! Three pieces, and only the third is code worth the name:
//!
//! - `clay.rs` — a worldgen feature that lays clay in riverbeds, on the banks
//!   beside them, and in patches across savanna. Pure data plus one positional
//!   field; the only host call in it is the batched biome probe that tells a
//!   BANK apart from ordinary ground.
//! - the pottery table — no code at all. A block row whose interaction opens
//!   `forge:pottery_table` plus recipe rows naming that station is a whole
//!   crafting bench; the engine runs the ordinary crafting session.
//! - `unlocks.rs` — when each station becomes visible, which the pack states
//!   itself rather than inheriting the engine's hold-every-ingredient default.
//! - `furnace.rs` — the forging furnace: a machine you OPERATE. Mould in by
//!   hand, lever to pour, and the cast pops out of the basin as an item.
//!
//! WHAT DECIDES A CAST IS DATA. The mould sitting in the basin carries a
//! `forge:mould` row-data entry naming a recipe CLASS; the furnace looks up
//! `(that class, the metal in the input slot)` in the ordinary machine-recipe
//! table. With no mould the class is `forge:cast_plate`. So the whole
//! metal x mould matrix lives in `recipes.json`, another pack adds a mould by
//! shipping an item row and its processing rows, and nothing here or in the
//! engine learns a new name. There is no `cast_iron_axe` anywhere.

mod anvil;
mod clay;
mod content;
mod furnace;
mod gold;
mod liquid;
mod unlocks;

use machine_core::Caches;
use mod_sdk::*;

use furnace::ForgingFurnace;

const TICK_SYSTEM: u32 = 1;
const ON_BLOCK_PLACED: u32 = 1;
const ON_CONTAINER_OPENED: u32 = 2;
const ON_ITEM_OBTAINED: u32 = 3;
const ON_BLOCK_BREAK: u32 = 4;
const GEN_CLAY: u32 = 1;

#[derive(Default)]
struct Forge {
    furnace: ForgingFurnace,
    anvil: anvil::Anvil,
    caches: Caches,
    clay: clay::Deposits,
    unlocks: unlocks::Unlocks,
    gold: gold::Gold,
}

impl Mod for Forge {
    fn init(&mut self) {
        // Worldgen instances run detached and re-run `init` per thread; the
        // clay feature resolves its own block there, so it must come first and
        // must not depend on anything sim-side.
        self.clay.init();
        register_worldgen_feature(WorldgenStage::Underground, GEN_CLAY);

        // Everything below reaches the SIMULATION — the anchor registry lives
        // in world KV. Worldgen instances run `mod_init` detached, with no
        // simulation to call into, and a sim call there is a hard error that
        // would take this thread's generator down with it.
        if runtime_side() != RuntimeSide::Server {
            return;
        }
        // Independent of the casting machinery: the ladder must still reveal
        // itself even if the furnace rows are broken.
        self.unlocks = unlocks::Unlocks::resolve();
        register_event_handler(EventKind::ItemObtained, 0, ON_ITEM_OBTAINED);

        // Gold's nondestructive mining: pure break-drops policy, independent
        // of the machines.
        self.gold = gold::Gold::resolve();
        if !self.gold.is_empty() {
            register_event_handler(EventKind::BlockBreakPre, 0, ON_BLOCK_BREAK);
        }

        let furnace_ok = self.furnace.init();
        if !furnace_ok {
            log("forge: the forging furnace rows are missing; casting stays idle");
        }
        let anvil_ok = self.anvil.init();
        if !anvil_ok {
            log("forge: the anvil rows are missing; augments stay idle");
        }
        if !furnace_ok && !anvil_ok {
            return;
        }
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        // The machines are HANDLED, not just opened: a claim here is what
        // makes the mould, the lever and the augment slots real interactions
        // instead of GUI widgets.
        register_event_handler(EventKind::ContainerOpened, 0, ON_CONTAINER_OPENED);
        // After WorldScheduled = beside the engine's own furnace step.
        register_tick_system(Stage::WorldScheduled, AttachSide::After, 0, TICK_SYSTEM);
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &mut *payload) {
            (
                ON_BLOCK_BREAK,
                EventPayload::BlockBreakPre {
                    block,
                    harvested,
                    player,
                    drops,
                    ..
                },
            ) => {
                self.gold.on_block_break(*block, *harvested, *player, drops);
            }
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                self.furnace.on_placed(*pos, *block);
                self.anvil.on_placed(*pos, *block);
            }
            (ON_CONTAINER_OPENED, EventPayload::ContainerOpened { kind, pos }) => {
                self.furnace.on_container_opened(kind, *pos);
                self.anvil.on_container_opened(kind, *pos);
            }
            (ON_ITEM_OBTAINED, EventPayload::ItemObtained { player, item }) => {
                self.unlocks.on_item_obtained(*player, *item);
            }
            _ => {}
        }
        Outcome::Continue
    }

    /// A widget in one of this pack's documents was clicked: the forging
    /// furnace's pour lever, or the anvil's Augment button.
    fn gui_click(&mut self, kind_key: &str, widget_id: &str, pos: Option<[i32; 3]>) {
        use machine_core::MachineSpec;
        let Some(pos) = pos else {
            return;
        };
        if kind_key == furnace::ForgingFurnaceSpec::KIND_KEY && widget_id == furnace::WIDGET_LEVER {
            self.furnace.spec().pull_lever(pos, &mut self.caches);
        }
        if kind_key == anvil::AnvilSpec::KIND_KEY && widget_id == anvil::WIDGET_AUGMENT {
            self.anvil.spec_mut().request_apply(pos);
        }
    }

    fn tick_system(&mut self, system_id: u32) {
        if system_id == TICK_SYSTEM {
            self.furnace.tick(&mut self.caches);
            self.anvil.tick(&mut self.caches);
        }
    }

    fn gen_feature(&mut self, feature_id: u32, ctx: &GenCtx) -> Vec<GenWrite> {
        match feature_id {
            GEN_CLAY => self.clay.generate(ctx),
            _ => Vec::new(),
        }
    }
}

register_mod!(Forge);
