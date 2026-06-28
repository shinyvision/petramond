use super::Game;
use crate::block::Block;
use crate::mathh::IVec3;
use crate::player;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
pub(super) const TICK_DT: f32 = 0.05;

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time.
const MAX_TICKS_PER_FRAME: u32 = 4;

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct MovementInput {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct GameInput {
    /// False while an app screen such as inventory owns input focus.
    pub gameplay_enabled: bool,
    pub movement: MovementInput,
    pub look_delta: (f32, f32),
    /// Whole wheel notches scrolled this frame (signed): negative selects
    /// previous slots, positive selects next, 0 for none. Wraps within the hotbar.
    pub hotbar_scroll: i32,
    /// Level state: primary button held for mining.
    pub break_held: bool,
    /// Edge state: primary button *pressed* this frame.
    pub attack_clicked: bool,
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GameEvents {
    /// The block placed this frame, if any.
    pub placed_block: Option<Block>,
    /// The block broken (player-mined) this frame, if any.
    pub broke_block: Option<Block>,
    /// The hand swung this frame for an attack.
    pub swung_hand: bool,
    /// An item/stack left the hand for the world this frame.
    pub threw_item: bool,
    /// At least one dropped item was collected into the inventory this frame.
    pub picked_up_item: bool,
    /// The player right-clicked a placed crafting table this frame.
    pub open_crafting_table: bool,
    /// The player right-clicked a placed furnace this frame.
    pub open_furnace: Option<IVec3>,
    /// The player right-clicked a placed chest this frame.
    pub open_chest: Option<IVec3>,
    /// The player right-clicked a placed furniture workbench this frame.
    pub open_furniture_workbench: Option<IVec3>,
    /// The player right-clicked a door this frame.
    pub toggled_door: bool,
}

/// What the world-mutating actions did across the fixed tick(s) that ran this frame.
#[derive(Copy, Clone, Debug, Default)]
pub(super) struct TickEvents {
    pub(super) broke_block: Option<Block>,
    pub(super) placed_block: Option<Block>,
    pub(super) swung_hand: bool,
    pub(super) picked_up_item: bool,
    pub(super) threw_item: bool,
}

impl Game {
    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        // Per-frame exceptions kept for local feel: look, hotbar, local player, mob push.
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.apply_mob_push(dt);
        self.tick_world();
        self.refresh_target();

        self.capture_intent(input);
        let events = self.run_fixed_ticks(dt);

        // Presentation/infra after fixed simulation; no gameplay mutation here.
        self.tick_entities(dt);
        self.advance_chest_lids(dt);
        self.advance_door_swings(dt);
        self.tick_mesh_budget();
        self.refresh_dropped_item_lights_after_world_light_update();

        self.maybe_autosave(dt);

        GameEvents {
            placed_block: events.placed_block,
            broke_block: events.broke_block,
            swung_hand: events.swung_hand,
            picked_up_item: events.picked_up_item,
            threw_item: events.threw_item,
            open_crafting_table: std::mem::take(&mut self.request_open_table),
            open_furnace: std::mem::take(&mut self.request_open_furnace),
            open_chest: std::mem::take(&mut self.request_open_chest),
            open_furniture_workbench: std::mem::take(&mut self.request_open_workbench),
            toggled_door: std::mem::take(&mut self.toggled_door),
        }
    }

    /// Latch this frame's input into the action-intent fields the fixed tick consumes.
    pub(super) fn capture_intent(&mut self, input: &GameInput) {
        self.intent_gameplay = input.gameplay_enabled;
        self.intent_sneak = input.movement.sneak;
        if !input.gameplay_enabled {
            // Menu focus drops queued action edges so clicks cannot fire behind screens.
            self.intent_break_held = false;
            self.pending_attack = false;
            self.pending_place = false;
            return;
        }
        self.intent_break_held = input.break_held;
        if input.attack_clicked {
            self.pending_attack = true;
        }
        if input.place_clicked {
            self.pending_place = true;
        }
    }

    fn run_fixed_ticks(&mut self, dt: f32) -> TickEvents {
        // Clamp long stalls and cap catch-up so fixed ticks never spiral.
        self.tick_accumulator += dt.clamp(0.0, 1.0);
        let mut ran = 0;
        let mut events = TickEvents::default();
        while self.tick_accumulator >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.game_tick_step(&mut events);
            self.tick_accumulator -= TICK_DT;
            ran += 1;
        }
        if self.tick_accumulator > TICK_DT {
            self.tick_accumulator = TICK_DT;
        }
        events
    }

    /// One fixed game tick: world and entity mutation only.
    fn game_tick_step(&mut self, events: &mut TickEvents) {
        // Keep action intent before world/entity simulation so inputs resolve on the tick.
        self.tick_mining(events);
        self.tick_place(events);
        self.tick_attack(events);
        self.tick_drops(events);
        self.tick_menu();

        self.world.game_tick(&self.recipes);
        self.process_natural_breaks();
        if self.item_pickup_tick() {
            events.picked_up_item = true;
        }

        let player_pos = self.player.body_center();
        let player_body = (!self.player.is_spectator())
            .then(|| crate::mob::Body::new(self.player.pos, player::HALF_W, player::HEIGHT));
        self.world.tick_mobs(TICK_DT, player_pos, player_body);
        self.world.tick_item_physics(TICK_DT, player_pos);
        self.world.spawn_mobs_tick(player_pos);
    }
}
