//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

mod container;

use std::collections::HashMap;

use crate::block::{Block, RenderShape};
use crate::camera::Camera;
use crate::crafting::{load_recipes, Recipes};
use crate::entity::{DroppedItem, ParticleSystem};
use crate::furnace::Facing;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{lerp, voxel_at, IVec3, SelectionShape, Vec3};
use crate::mining::MiningState;
#[cfg(test)]
use crate::mob::Mob;
use crate::mob::{load_loot, DeathDrop, LootTables};
use crate::player::{self, Input, Player, PlayerMode, RaycastHit};
use crate::render::{BreakOverlayView, ChestView, FurnaceView};
use crate::torch::TorchPlacement;
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

pub use container::{ContainerMenu, ContainerTarget, MenuSlot};

/// Deep, murky blue the world fades to (fog + clear colour) when the camera eye
/// is underwater.
const UNDERWATER_FOG_COLOR: [f32; 3] = [0.04, 0.16, 0.30];

/// Require the camera eye to sit this far below an open water surface before the
/// underwater shader/fog kicks in. This keeps shallow flowing films from tinting
/// the view when the eye is only barely clipping their rendered surface.
const UNDERWATER_SURFACE_MARGIN: f32 = 0.03;

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

/// Minimum number of game ticks between two attack swings, so a player mashing the
/// attack button can't land hits every tick (which would, e.g., instakill an owl).
/// Counted in ticks now that attacks resolve on the fixed tick — 6 ticks ≈ 0.3 s.
const ATTACK_COOLDOWN_TICKS: u32 = 6;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
const TICK_DT: f32 = 0.05;

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time —
/// it just runs the late tick and reschedules from now (per the design).
const MAX_TICKS_PER_FRAME: u32 = 4;

/// Chest-lid open/close speed (fraction per second)
const CHEST_LID_SPEED: f32 = 3.5;

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
    /// Edge state: primary button *pressed* this frame — one attack swing (damages a
    /// targeted mob, or punches the air). Distinct from `break_held` (the held state
    /// that drives mining a block).
    pub attack_clicked: bool,
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GameEvents {
    pub placed_block: bool,
    pub broke_block: bool,
    /// The hand swung this frame for an attack — a mob hit or a punch at the air.
    /// Drives a one-shot first-person swing (like `broke_block`).
    pub swung_hand: bool,
    /// An item/stack left the hand for the world this frame — a Q drop or an
    /// inventory drag-out. Drives the same first-person place jab as a placement.
    pub threw_item: bool,
    /// The player right-clicked a placed crafting table this frame. The app shell
    /// reacts by opening the 3×3 crafting screen (the game can't own screens).
    pub open_crafting_table: bool,
    /// The player right-clicked a placed furnace this frame (its world position).
    /// The app shell reacts by opening the furnace screen.
    pub open_furnace: Option<IVec3>,
    /// The player right-clicked a placed chest this frame (its world position).
    /// The app shell reacts by opening the chest screen.
    pub open_chest: Option<IVec3>,
}

/// What the world-mutating actions did across the fixed tick(s) that ran this frame,
/// surfaced back to the per-frame [`GameEvents`] so the presentation layer (hand
/// animation, sounds) reacts. Bools because a frame runs at most a handful of ticks and
/// each action happens at most once — only whether it happened matters.
#[derive(Copy, Clone, Debug, Default)]
struct TickEvents {
    /// A block finished breaking on a tick.
    broke_block: bool,
    /// A block was placed on a tick.
    placed_block: bool,
    /// An attack swing connected on a tick (a mob hit or a punch at the air).
    swung_hand: bool,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct GameEnvironment {
    pub fog: [f32; 3],
    pub time: f32,
    pub underwater: bool,
}

pub struct Game {
    cam: Camera,
    world: World,
    fallback_world: CascadeWorld,
    player: Player,
    /// Block currently under the crosshair, refreshed each tick. Set to `None` when a
    /// mob is the closer target (so looking at a mob interrupts block selection/mining).
    look: Option<RaycastHit>,
    /// The mob under the crosshair (index into the world's live mob set) this frame,
    /// nearer than any block, if any. Recomputed every frame — never stored across a
    /// mob tick, so despawn-driven index shifts can't make it stale.
    targeted_mob: Option<usize>,
    mining: MiningState,
    /// Tracks the world's lighting revision so item-entity skylight is only
    /// recomputed when a world light bake actually changed. The drops themselves
    /// live in `World` (with their chunks); this is just the change detector.
    dropped_light_revision: u64,
    particles: ParticleSystem,
    spawn_counter: u32,
    mining_dust_t: f32,
    /// Game ticks remaining before the player may attack again — the
    /// [`ATTACK_COOLDOWN_TICKS`] gate, decremented once per tick. An attack is refused
    /// while it is positive, so mashing the button can't land hits faster than this.
    attack_cooldown: u32,
    /// --- Per-frame input intent, sampled in [`tick`](Self::tick) and consumed by the
    /// fixed-tick loop, so every world/entity mutation happens on the game tick while
    /// input is still read per-frame. ---
    /// Primary button held this frame (mine the looked-at block); re-sampled each frame.
    intent_break_held: bool,
    /// Sneaking this frame (gates right-click interact vs. place); re-sampled each frame.
    intent_sneak: bool,
    /// Gameplay input is live this frame (false while a screen owns focus); re-sampled.
    intent_gameplay: bool,
    /// A primary-button *press* is waiting to be resolved as an attack on the next tick
    /// (edge-latched; the tick consumes it). The visual swing is emitted when it resolves.
    pending_attack: bool,
    /// A secondary-button *press* is waiting to be resolved as a place/interact on the
    /// next tick (edge-latched; the tick consumes it).
    pending_place: bool,
    /// Items dropped this frame (Q-drop, inventory throws, crafting overflow), built
    /// against the current look but waiting to enter the world on the next tick — a drop
    /// materialises on the tick, like every other entity. Drained each tick.
    pending_drops: Vec<DroppedItem>,
    /// Container-menu clicks (slot, button, shift, the App's double-click `gather`
    /// verdict) latched this frame, applied in order on the next tick so chest / furnace /
    /// inventory edits mutate state on the tick. Real clicks are >1 tick apart, so each is
    /// applied before the next is decided. Drained each tick.
    pending_menu_clicks: Vec<(MenuSlot, crate::controls::PointerButton, bool, bool)>,
    /// Transient per-chest lid open angle (`0.0` closed .. `1.0` open), keyed by world
    /// position. Eased toward open for the chest whose screen is up and toward closed
    /// for the rest; client-side animation only, never persisted. The render-side
    /// scene adapter reads the angle (via [`Game::chest_lid_angle`]) to bake the lid;
    /// the easing in [`Game::advance_chest_lids`] is the owning sim/animation state.
    chest_lids: HashMap<IVec3, f32>,
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    tick_accumulator: f32,
    /// Wall-clock seconds since the last background autosave.
    autosave_t: f32,
    /// Set when the hand expels an item into the world (Q drop or inventory
    /// drag-out) so the next [`tick`](Self::tick) reports it for the hand's place
    /// jab. Consumed (reset) each tick.
    threw_item: bool,
    /// Loaded crafting recipes (from `assets/recipes.json`). Used both by the open
    /// [`ContainerMenu`]'s craft preview (borrowed in per call) and by the furnace
    /// *smelting* tick (`World::game_tick`), which is why they live here on `Game`
    /// rather than on the menu — the menu would otherwise need a self-referential
    /// borrow during the tick.
    recipes: Recipes,
    /// Mob loot tables (from `assets/loot_tables.json`), rolled when a mob dies to
    /// spawn its dropped items. Loaded once at world load, like [`recipes`](Self::recipes).
    loot: LootTables,
    /// The open container GUI's persistent *edit target*: the block-entity (or the
    /// inventory-side craft grid) the screen currently mutates, plus its slot
    /// behaviour. NOT the screen authority — `App::AppScreen` decides which screen
    /// is open; this only tracks what that screen is acting on.
    menu: ContainerMenu,
    /// Set when the player right-clicks a placed crafting table, so the next
    /// [`tick`](Self::tick) asks the app shell to open the 3×3 screen. One-shot
    /// open *request* (consumed via [`GameEvents`]), distinct from the menu's
    /// persistent edit target.
    request_open_table: bool,
    /// Set to a furnace's position when right-clicked, so the next
    /// [`tick`](Self::tick) asks the app shell to open the furnace screen. One-shot
    /// open request (consumed via [`GameEvents`]).
    request_open_furnace: Option<IVec3>,
    /// Set to a chest's position when right-clicked, so the next [`tick`](Self::tick)
    /// asks the app shell to open the chest screen. One-shot open request (consumed
    /// via [`GameEvents`]).
    request_open_chest: Option<IVec3>,
}

impl Game {
    pub fn new(mut cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        // Open (or create) the on-disk world. A returning world supplies its own
        // seed and player; a fresh one uses `new_seed` and a found spawn. Item
        // entities are no longer global — they load with their chunks.
        let (save, level) = if world_name.is_empty() {
            // Empty name = in-memory only (used by tests; never touches disk).
            (None, None)
        } else {
            match crate::save::open(world_name) {
                Ok(o) => (Some(o.save), o.level),
                Err(e) => {
                    log::warn!("save disabled: could not open world '{world_name}': {e}");
                    (None, None)
                }
            }
        };
        let seed = level.as_ref().map(|l| l.seed).unwrap_or(new_seed);

        let fallback_world = CascadeWorld::new(seed);

        // Restore the saved player, or spawn on the nearest exposed solid surface
        // to the origin (drops to the nearest coast if the origin is open ocean).
        let player = match &level {
            Some(l) => {
                let mut p = Player::new(l.player_pos);
                p.set_mode(l.player_mode);
                p.vel = l.player_vel; // assign after set_mode, which zeroes vel
                p.yaw = l.player_yaw;
                p.pitch = l.player_pitch;
                p.inventory = l.inventory.clone();
                p
            }
            None => {
                let surface = crate::worldgen::spawn::find_spawn(&fallback_world, seed);
                let feet = Vec3::new(
                    surface.x as f32 + 0.5,
                    (surface.y + 1) as f32,
                    surface.z as f32 + 0.5,
                );
                Player::new(feet)
            }
        };
        // Centre chunk streaming on the real player position from frame one, and
        // point the camera the way the player is looking (restored from the save,
        // or the default forward for a fresh spawn).
        cam.pos = player.eye();
        cam.yaw = player.yaw;
        cam.pitch = player.pitch;

        let mut world = World::new(seed, render_dist);
        if let Some(s) = save {
            world.attach_save(s);
        }

        Self {
            cam,
            world,
            fallback_world,
            player,
            look: None,
            targeted_mob: None,
            mining: MiningState::new(),
            dropped_light_revision: 0,
            particles: ParticleSystem::new(),
            spawn_counter: 0,
            mining_dust_t: 0.0,
            attack_cooldown: 0,
            intent_break_held: false,
            intent_sneak: false,
            intent_gameplay: false,
            pending_attack: false,
            pending_place: false,
            pending_drops: Vec::new(),
            pending_menu_clicks: Vec::new(),
            chest_lids: HashMap::new(),
            tick_accumulator: 0.0,
            autosave_t: 0.0,
            threw_item: false,
            recipes: load_recipes(),
            loot: load_loot(),
            menu: ContainerMenu::new(),
            request_open_table: false,
            request_open_furnace: None,
            request_open_chest: None,
        }
    }

    /// Persist everything: flush modified chunks (carrying any resting item
    /// entities, so their lifetime timers survive) to the save thread, then write
    /// `level.dat` (seed + player + inventory). A no-op without an attached save.
    pub fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(self.world.seed, &self.player, 0));
        }
    }

    fn maybe_autosave(&mut self, dt: f32) {
        const AUTOSAVE_SECS: f32 = 30.0;
        if self.world.save().is_none() {
            return;
        }
        self.autosave_t += dt;
        if self.autosave_t >= AUTOSAVE_SECS {
            self.autosave_t = 0.0;
            self.save_all();
        }
    }

    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        // --- Per-frame: input → look/camera, player movement, chunk streaming. These
        // read the world but don't simulate it; the local player moves per-frame so the
        // controls stay crisp (mouse-look + movement are presentation/input, not ticks). ---
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        // Mobs push the player out of an overlap per-frame (not on the tick) so the drift
        // is perfectly smooth — player movement integrates every frame, and a 20 Hz shove
        // would pulse. The mobs themselves are shoved on the tick (in `game_tick_step`).
        self.apply_mob_push(dt);
        self.tick_world();

        // Block hit (with its distance) and the closest mob in front. A mob nearer than
        // the block wins the crosshair and nulls the block look, so block selection /
        // mining / placement are interrupted while looking at a mob. Recomputed per frame
        // for the crosshair and read by the tick when it resolves an action.
        let block_hit = Player::raycast_with_dist(self.cam.pos, self.cam.forward(), &self.world);
        self.look = block_hit.map(|(h, _)| h);
        let block_dist = block_hit.map(|(_, d)| d).unwrap_or(player::REACH);
        self.targeted_mob = self.closest_mob(self.cam.pos, self.cam.forward(), block_dist);
        if self.targeted_mob.is_some() {
            self.look = None;
        }

        // Sample this frame's action intent for the fixed tick to consume — world/entity
        // mutation (mining, placing, attacking, entity physics) all happens on the tick.
        self.capture_intent(input);

        // --- Fixed tick(s): the authoritative simulation. Returns what its world-mutating
        // actions did, so the per-frame presentation layer can react. ---
        let events = self.run_fixed_ticks(dt);

        // --- Per-frame: presentation + infra (particles, chest-lid animation, meshing,
        // light handoff, autosave). None of this changes world/entity state. ---
        self.tick_entities(dt);
        self.advance_chest_lids(dt);
        self.tick_mesh_budget();
        self.refresh_dropped_item_lights_after_world_light_update();

        self.maybe_autosave(dt);

        GameEvents {
            placed_block: events.placed_block,
            broke_block: events.broke_block,
            swung_hand: events.swung_hand,
            // Throws happen via input handlers before this tick; report and clear.
            threw_item: std::mem::take(&mut self.threw_item),
            // Set on a tick by `tick_place` when a table was right-clicked.
            open_crafting_table: std::mem::take(&mut self.request_open_table),
            // Set when a furnace was right-clicked.
            open_furnace: std::mem::take(&mut self.request_open_furnace),
            // Set when a chest was right-clicked.
            open_chest: std::mem::take(&mut self.request_open_chest),
        }
    }

    /// Latch this frame's input into the action-intent fields the fixed tick consumes:
    /// held states (mine / sneak / gameplay-enabled) are re-sampled every frame, while
    /// button *presses* (attack / place) are edge-latched so a press is never lost on a
    /// frame that runs no tick, and is consumed by the next tick that does.
    fn capture_intent(&mut self, input: &GameInput) {
        self.intent_gameplay = input.gameplay_enabled;
        self.intent_sneak = input.movement.sneak;
        // While a screen (inventory, chest, …) owns input, gameplay actions are off:
        // don't mine, and drop any latched press so a buffered click can't fire on a
        // tick that runs behind the open menu.
        if !input.gameplay_enabled {
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

    /// Advance the simulation in fixed 50 ms steps, decoupled from the frame rate: 0
    /// steps on a fast frame, several to catch up on a slow one (capped, so a long stall
    /// runs the late tick and resyncs rather than spiralling). Player movement, camera,
    /// and rendering stay per-frame above. Returns the merged [`TickEvents`] of the
    /// world-mutating actions across the tick(s) that ran, for the per-frame presentation.
    fn run_fixed_ticks(&mut self, dt: f32) -> TickEvents {
        // Ignore absurd deltas (first frame, tab regaining focus) so a long pause
        // doesn't dump a burst of ticks; clamp keeps at most one step pending.
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

    /// One fixed game tick: every change to the world and the entities in it. Player
    /// actions (sampled per-frame into intent fields) resolve first so the world reacts to
    /// them the same tick, then the world simulation runs, then the entities (mobs, items).
    /// Merges what its actions did into `events` for the per-frame presentation layer.
    fn game_tick_step(&mut self, events: &mut TickEvents) {
        // Player → world actions, on the tick.
        self.tick_mining(events);
        self.tick_place(events);
        self.tick_attack(events);
        self.tick_drops();
        self.tick_menu();

        // The world owns its per-tick sequence (scheduled ticks, block updates, furnace
        // smelting). Recipes live in `Game`, so they're passed through. Then item lifetime
        // + collection (pickup needs `player.inventory`, hence the borrow split here).
        self.world.game_tick(&self.recipes);
        // Blocks the world simulation itself destroyed this tick (fragile blocks that
        // lost their support, or were washed away by water) get a player-style break:
        // the break-particle burst plus their rolled item drops.
        self.process_natural_breaks();
        self.item_pickup_tick();

        // Entities. Mobs and dropped items are owned by `World` (they persist with their
        // chunks); the world drives them via a borrow-split. Soft entity pushing shoves
        // the MOBS here (mob↔mob, and off the player's body) — the reverse push on the
        // player is applied per-frame (`apply_mob_push`), since it moves the player and
        // player movement integrates per-frame for smoothness. A noclip spectator has no
        // body, so mobs aren't shoved off it.
        let player_pos = self.player.body_center();
        let player_body = (!self.player.is_spectator())
            .then(|| crate::mob::Body::new(self.player.pos, player::HALF_W, player::HEIGHT));
        self.world.tick_mobs(TICK_DT, player_pos, player_body);
        self.world.tick_item_physics(TICK_DT, player_pos);
        // Natural spawning, one attempt per tick.
        self.world.spawn_mobs_tick(player_pos);
    }

    #[inline]
    pub fn camera(&self) -> &Camera {
        &self.cam
    }

    #[inline]
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    #[inline]
    pub fn inventory(&self) -> &Inventory {
        &self.player.inventory
    }

    /// Add a stack to the player inventory, returning any leftover that didn't
    /// fit (merging into matching stacks first, then empty slots). The world
    /// pickup path inlines this with a borrow split; exposed here for giving the
    /// player items directly.
    pub fn add_to_inventory(&mut self, stack: ItemStack) -> Option<ItemStack> {
        self.player.inventory.add(stack)
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        self.cam.aspect = aspect;
    }

    pub fn toggle_player_mode(&mut self) {
        self.player.toggle_mode();
    }

    #[inline]
    pub fn player_mode(&self) -> PlayerMode {
        self.player.mode()
    }

    pub fn set_active_hotbar(&mut self, slot: u8) {
        self.player.inventory.set_active(slot);
    }

    /// Whether the cursor currently holds a stack. Gates the double-click gather,
    /// which only fires while a stack is being dragged.
    pub fn cursor_has_stack(&self) -> bool {
        self.player.inventory.cursor().is_some()
    }

    /// Double-click gather: top up the cursor-held stack with every matching item
    /// in the inventory. See [`Inventory::collect_to_cursor`].
    pub fn collect_to_cursor(&mut self) {
        self.player.inventory.collect_to_cursor();
    }

    // --- Container menu (forwarders) --------------------------------------
    //
    // The open container GUI's edit target + slot behaviour live on `ContainerMenu`
    // (`game/container.rs`). These thin forwarders split `Game` into its disjoint
    // `menu` / `world` / `player.inventory` fields and hand them to the menu (recipes
    // borrowed from `Game` per call) — the App can't take those disjoint borrows
    // itself. Per-slot interaction routing funnels through the single
    // [`menu_click`](Self::menu_click) entry; open/close + view forwarders cover the
    // menu's lifecycle and what the UI reads.

    /// Read-only handle to the open container menu (its target + craft grid).
    #[inline]
    pub fn menu(&self) -> &ContainerMenu {
        &self.menu
    }

    /// Latch a hit-tested container click — resolved by the App to a [`MenuSlot`], a
    /// button, and Shift, with the App's double-click `gather` verdict — for the next game
    /// tick. Container edits mutate world state (chest / furnace contents) and the
    /// inventory, so they resolve on the tick like every other action — see
    /// [`tick_menu`](Self::tick_menu). The verdict is captured now, against the live
    /// cursor; since real clicks are more than a tick apart, each is applied before the
    /// next one is decided.
    pub fn menu_click(
        &mut self,
        slot: MenuSlot,
        button: crate::controls::PointerButton,
        shift: bool,
        gather: bool,
    ) {
        self.pending_menu_clicks.push((slot, button, shift, gather));
    }

    /// Apply the player actions latched this frame — container edits and item drops — at
    /// once, standing in for the game tick that resolves them in play. For App-level tests
    /// that drive the input routing and then assert the resulting inventory / world state
    /// (between two clicks a real tick interleaves, applying the first before the second is
    /// decided — call this there too).
    #[cfg(test)]
    pub(crate) fn apply_latched_actions_for_test(&mut self) {
        self.tick_menu();
        self.tick_drops();
    }

    /// Apply this frame's latched container-menu clicks, in order, on the tick. Splits the
    /// disjoint `menu` / `world` / `inventory` borrows the menu needs and lends the
    /// recipes; the menu decodes each interaction keyed on its target.
    fn tick_menu(&mut self) {
        for (slot, button, shift, gather) in std::mem::take(&mut self.pending_menu_clicks) {
            self.menu.click(
                &mut self.world,
                &mut self.player.inventory,
                &self.recipes,
                slot,
                button,
                shift,
                gather,
            );
        }
    }

    /// The active crafting grid (for the UI to read cells + result preview).
    #[inline]
    pub fn craft_grid(&self) -> &crate::crafting::CraftGrid {
        self.menu.craft_grid()
    }

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub fn open_crafting(&mut self, cols: usize) {
        self.menu.open_crafting(cols, &self.recipes);
    }

    /// Begin a furnace-screen session at `pos` (the GUI's edit target).
    pub fn open_furnace_screen(&mut self, pos: IVec3) {
        self.menu.open_furnace_screen(&mut self.world, pos);
    }

    /// End the furnace-screen session.
    pub fn close_furnace(&mut self) {
        self.menu.close_furnace();
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world), then clear the result. Overflow is
    /// gathered first, then thrown after the menu call so `throw_item`'s
    /// `world`/`cam` borrow doesn't alias the menu's borrow.
    pub fn close_crafting(&mut self) {
        let mut overflow = Vec::new();
        self.menu
            .close_crafting(&mut self.player.inventory, &self.recipes, |stack| {
                overflow.push(stack);
            });
        for stack in overflow {
            self.throw_item(stack);
        }
    }

    /// The view of the currently-open furnace for the UI, or `None` if no furnace
    /// screen is up or it has unloaded.
    pub fn open_furnace_view(&self) -> Option<FurnaceView> {
        self.menu.open_furnace_view(&self.world)
    }

    /// Begin a chest-screen session at `pos` (the GUI's edit target).
    pub fn open_chest_screen(&mut self, pos: IVec3) {
        self.menu.open_chest_screen(&mut self.world, pos);
    }

    /// End the chest-screen session.
    pub fn close_chest(&mut self) {
        self.menu.close_chest();
    }

    /// Close-time cleanup for a cursor-held GUI stack: merge it back into matching
    /// inventory stacks, then empty slots, and queue only any leftover to drop into
    /// the world on the next tick.
    pub fn close_cursor_stack(&mut self) {
        if let Some(stack) = self.player.inventory.stash_cursor_in_inventory() {
            self.throw_item(stack);
        }
    }

    /// The view of the currently-open chest for the UI, or `None` if no chest screen
    /// is up or it has unloaded.
    pub fn open_chest_view(&self) -> Option<ChestView> {
        self.menu.open_chest_view(&self.world)
    }

    /// Throw the whole cursor-held stack out into the world (inventory drag-out
    /// then click outside the panel). No-op when the cursor is empty.
    pub fn throw_cursor_stack(&mut self) {
        if let Some(stack) = self.player.inventory.take_cursor() {
            self.throw_item(stack);
        }
    }

    /// Throw a single item off the cursor-held stack (right-click outside the
    /// panel while dragging). No-op when the cursor is empty.
    pub fn throw_cursor_one(&mut self) {
        if let Some(stack) = self.player.inventory.take_cursor_one() {
            self.throw_item(stack);
        }
    }

    /// Drop the player's held (active hotbar) item into the world via the in-game
    /// drop key. With `all`, the whole stack is thrown (Ctrl+Q); otherwise a
    /// single item (Q). No-op with an empty hand.
    pub fn drop_selected_item(&mut self, all: bool) {
        let stack = if all {
            self.player.inventory.take_selected_all()
        } else {
            self.player.inventory.take_selected_one()
        };
        if let Some(stack) = stack {
            self.throw_item(stack);
        }
    }

    /// Queue `stack` to be thrown into the world as a dropped item, flying out along the
    /// camera's look direction and originating just in front of the eye so it clears the
    /// player. The drop is built now (per frame, against the current look) but enters the
    /// world on the next game tick — like every other entity, a drop materialises on the
    /// tick (see [`tick_drops`](Self::tick_drops)). The item already left the inventory.
    fn throw_item(&mut self, stack: ItemStack) {
        let dir = self.cam.forward();
        let origin = self.cam.pos + dir * 0.3;
        let mut drop = DroppedItem::thrown(origin, stack, dir);
        drop.skylight = light6_at_pos(&self.world, origin);
        self.pending_drops.push(drop);
        // Flick the hand forward (place jab) on the next rendered frame.
        self.threw_item = true;
    }

    #[inline]
    pub fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    #[inline]
    pub fn is_mining(&self) -> bool {
        self.mining.is_mining()
    }

    #[inline]
    pub fn selection(&self) -> Option<SelectionShape> {
        self.look.map(|h| h.outline)
    }

    #[inline]
    pub fn break_overlay_view(&self) -> Option<BreakOverlayView> {
        self.mining.overlay().map(|(block, stage)| {
            // A bbmodel block cracks over its targeted cell's actual cube surfaces (kind
            // + that cell's footprint offset); the chest over its inset box; everything
            // else over the full cell (`None`).
            let model = match Block::from_id(self.world.chunk_block(block.x, block.y, block.z))
                .render_shape()
            {
                crate::block::RenderShape::Model(kind) => Some((
                    kind,
                    self.world.model_offset_at(block.x, block.y, block.z),
                    self.world.model_facing_at(block.x, block.y, block.z),
                )),
                _ => None,
            };
            BreakOverlayView {
                block,
                visual_box: if model.is_some() {
                    None
                } else {
                    self.world.selection_box_at(block.x, block.y, block.z)
                },
                model,
                stage,
            }
        })
    }

    /// The active dropped item-entities, for the render-side scene adapter to bake
    /// into `ItemEntityInstance`s. Their cached skylight is kept fresh by the
    /// per-tick light refresh, so the adapter only reads here.
    #[inline]
    pub fn item_entities(&self) -> &[DroppedItem] {
        self.world.item_entities()
    }

    /// The placed chests' render data — world position, facing, sampled skylight —
    /// gathered from the loaded chunks into `out` (cleared first). The render-side
    /// scene adapter pairs each with its lid angle (via [`chest_lid_angle`]) to bake
    /// a `ChestInstance`. The lid animation itself stays here on `Game`.
    ///
    /// [`chest_lid_angle`]: Self::chest_lid_angle
    #[inline]
    pub fn collect_chest_render_data(&self, out: &mut Vec<(IVec3, Facing, u8)>) {
        self.world.collect_chests(out);
    }

    /// The transient open progress (`0.0` closed .. `1.0` open) of the chest at
    /// `pos`, or `0.0` if it isn't tracked. The render-side scene adapter reads this
    /// to bake the chest's lid hinge; the easing/animation lives in
    /// [`advance_chest_lids`](Self::advance_chest_lids).
    #[inline]
    pub fn chest_lid_angle(&self, pos: IVec3) -> f32 {
        self.chest_lids.get(&pos).copied().unwrap_or(0.0)
    }

    /// Advance the transient chest-lid animation by `dt`: the open chest's lid eases
    /// toward fully open, every other tracked lid toward closed, and lids that reach
    /// closed (and aren't the open chest) are dropped. The open/closed target is
    /// derived from the menu's edit target (the open chest's position), so the lid
    /// follows the GUI being open — purely client-side, never saved.
    fn advance_chest_lids(&mut self, dt: f32) {
        let step = (dt * CHEST_LID_SPEED).clamp(0.0, 1.0);
        let open = self.menu.target().open_chest();
        // Ensure the open chest is tracked so it animates from closed on the first frame.
        if let Some(pos) = open {
            self.chest_lids.entry(pos).or_insert(0.0);
        }
        self.chest_lids.retain(|&pos, lid| {
            let target = if Some(pos) == open { 1.0 } else { 0.0 };
            if *lid < target {
                *lid = (*lid + step).min(target);
            } else if *lid > target {
                *lid = (*lid - step).max(target);
            }
            // Keep while still animating, or while it is the open chest.
            *lid > f32::EPSILON || Some(pos) == open
        });
    }

    /// The live particle system, for the render-side scene adapter to bake into
    /// `ParticleInstance`s. Read-only; ticking happens in the sim.
    #[inline]
    pub fn particles(&self) -> &ParticleSystem {
        &self.particles
    }

    /// The live mobs, for the render-side scene adapter to interpolate + bake.
    #[inline]
    pub fn mobs(&self) -> &[crate::mob::Instance] {
        self.world.mobs().instances()
    }

    /// Fraction (`0..1`) into the next fixed tick — the blend factor the scene uses to
    /// interpolate each entity's render pose between its previous and current tick, so the
    /// mobs and dropped items (which simulate at 20 TPS) move smoothly at any frame rate.
    #[inline]
    pub fn tick_alpha(&self) -> f32 {
        (self.tick_accumulator / TICK_DT).clamp(0.0, 1.0)
    }

    /// Combined light + warm-tint amount at the player's eye, for lighting the
    /// first-person hand / held item — it brightens AND warms near torches/furnaces.
    pub fn held_item_light(&self) -> (u8, u8) {
        let c = voxel_at(self.cam.pos);
        self.world.dynamic_light_at_world(c.x, c.y, c.z)
    }

    pub fn environment(&self, now: f64) -> GameEnvironment {
        let eye = self.cam.pos;
        let underwater = camera_eye_underwater(&self.world, eye);

        let fog = if underwater {
            UNDERWATER_FOG_COLOR
        } else {
            self.blended_sky_fog_color(eye.x, eye.z)
        };

        GameEnvironment {
            fog,
            underwater,
            time: (now % 3600.0) as f32,
        }
    }

    fn apply_camera_input(&mut self, input: &GameInput) {
        if !input.gameplay_enabled {
            return;
        }
        let (dx, dy) = input.look_delta;
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        const SENS: f32 = 0.0025;
        self.player.rotate(-dx * SENS, -dy * SENS);
        // Mirror the player's look onto the camera now (before this tick's
        // movement + raycast read `cam.forward()`), the same way `tick_player`
        // mirrors `player.eye()` onto `cam.pos`.
        self.cam.yaw = self.player.yaw;
        self.cam.pitch = self.player.pitch;
    }

    fn apply_hotbar_input(&mut self, input: &GameInput) {
        if input.gameplay_enabled && input.hotbar_scroll != 0 {
            self.player.inventory.scroll_active(input.hotbar_scroll);
        }
    }

    fn tick_player(&mut self, dt: f32, input: &GameInput) {
        let spectator = self.player.is_spectator();
        let f = self.cam.forward();
        let fwd = if spectator {
            f
        } else {
            Vec3::new(f.x, 0.0, f.z).normalize_or_zero()
        };
        let right = self.cam.right();
        let mut wishdir = Vec3::ZERO;

        if input.gameplay_enabled {
            if input.movement.forward {
                wishdir += fwd;
            }
            if input.movement.backward {
                wishdir -= fwd;
            }
            if input.movement.right {
                wishdir += right;
            }
            if input.movement.left {
                wishdir -= right;
            }
            if spectator {
                if input.movement.jump {
                    wishdir += Vec3::Y;
                }
                if input.movement.sneak {
                    wishdir -= Vec3::Y;
                }
            }
        }

        let player_input = Input {
            wishdir: wishdir.normalize_or_zero(),
            jump: input.gameplay_enabled && input.movement.jump,
            sprint: input.gameplay_enabled && input.movement.sprint,
        };

        if spectator || self.player.columns_loaded(&self.world) {
            let mut remaining = dt.min(0.25);
            while remaining > 0.0 {
                let step = remaining.min(player::DT_MAX);
                self.player.update(step, &self.world, player_input);
                remaining -= step;
            }
        }

        self.cam.pos = self.player.eye();
    }

    /// Push the player out of any mob it overlaps, per frame. The mobs sit at their
    /// last-tick positions (fixed between ticks), so as the player moves each frame the
    /// overlap — and the push — track the player smoothly; applied as a small
    /// collision-resolved displacement (the push *velocity* over this frame's `dt`), it
    /// never accumulates or fights the movement controller. A noclip spectator has no body
    /// to jostle. The mobs' own half of the push runs on the tick (`game_tick_step`).
    fn apply_mob_push(&mut self, dt: f32) {
        if self.player.is_spectator() {
            return;
        }
        let body = crate::mob::Body::new(self.player.pos, player::HALF_W, player::HEIGHT);
        let push = self.world.mobs().push_on_player(body);
        if push != Vec3::ZERO {
            self.player.shove(push * dt, &self.world);
        }
        self.cam.pos = self.player.eye();
    }

    fn tick_world(&mut self) {
        let cam_cx = (self.cam.pos.x as i32) >> 4;
        let cam_cz = (self.cam.pos.z as i32) >> 4;
        self.world.update_load(cam_cx, cam_cz);
        let _ = self.world.poll();
    }

    fn tick_mesh_budget(&mut self) {
        const MESH_BUDGET: usize = 32;
        self.world.tick_mesh_budget(MESH_BUDGET);
    }

    /// Mining, on the tick: advance the break timer against the block under the crosshair
    /// (sampled per frame into `look`) by [`TICK_DT`], and when a block finishes breaking,
    /// clear it, scatter any block-entity contents + harvested drops, and spawn the break
    /// burst. Frame-rate independent. Gated off (progress reset) while a screen owns input
    /// (`intent_gameplay` false) — that's `inventory_open` to the mining controller.
    fn tick_mining(&mut self, events: &mut TickEvents) {
        // The held tool (None = bare hand) gates mining speed + whether drops fall.
        let tool = self.player.inventory.selected().and_then(|s| s.item.tool());
        if let Some(event) = self.mining.update(
            TICK_DT,
            self.look.as_ref(),
            self.intent_break_held,
            !self.intent_gameplay,
            &self.world,
            tool,
        ) {
            events.broke_block = true;
            let hit_normal = self
                .look
                .filter(|h| h.block == event.pos && h.normal != IVec3::ZERO)
                .map(|h| h.normal);
            let (light, warm) = break_light(&self.world, event.pos, hit_normal);
            // A bbmodel block breaks as a whole: removing any cell clears every footprint
            // cell (the 2×2×1 workbench vanishes as one object, drops one item below).
            if matches!(event.block.render_shape(), RenderShape::Model(_)) {
                self.world.remove_model_block(event.pos);
            } else {
                self.world
                    .set_block_world(event.pos.x, event.pos.y, event.pos.z, Block::Air);
            }
            // A broken furnace scatters whatever it held, regardless of tool (the
            // furnace ITEM still needs a pickaxe — handled by spawn_drops below).
            if event.block == Block::Furnace {
                if let Some(f) = self.world.take_furnace(event.pos) {
                    for stack in [f.input, f.fuel, f.output].into_iter().flatten() {
                        self.spawn_item_stack(event.pos, stack, light);
                    }
                }
            } else if event.block == Block::Chest {
                // A broken chest scatters its whole contents, regardless of tool.
                if let Some(chest) = self.world.take_chest(event.pos) {
                    for stack in chest.slots.into_iter().flatten() {
                        self.spawn_item_stack(event.pos, stack, light);
                    }
                }
            } else if event.block == Block::Torch {
                // A torch has no contents — just forget its recorded orientation so
                // the freed cell carries no stale block-entity state.
                self.world.take_torch(event.pos);
            }
            // A bbmodel block has no block-atlas tile, so its burst samples its own
            // texture (the model atlas); every other block uses its face tiles.
            match event.block.render_shape() {
                RenderShape::Model(kind) => self
                    .particles
                    .spawn_break_burst_model(event.pos, kind, light, warm),
                _ => self
                    .particles
                    .spawn_break_burst_lit(event.pos, event.block, light, warm),
            }
            if event.harvested {
                self.spawn_drops(event.pos, event.block, light);
            }
        }

        // A small dust fleck every MINING_DUST_INTERVAL while actively breaking.
        if self.mining.is_mining() {
            if let Some(h) = self.look {
                self.mining_dust_t += TICK_DT;
                if self.mining_dust_t >= MINING_DUST_INTERVAL {
                    self.mining_dust_t = 0.0;
                    let block =
                        Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
                    let cell = h.block + h.normal;
                    let (light, warm) = self.world.dynamic_light_at_world(cell.x, cell.y, cell.z);
                    match block.render_shape() {
                        RenderShape::Model(kind) => self
                            .particles
                            .spawn_mining_model(h.block, h.normal, kind, light, warm),
                        _ => self
                            .particles
                            .spawn_mining_lit(h.block, h.normal, block, light, warm),
                    }
                }
            }
        } else {
            self.mining_dust_t = 0.0;
        }
    }

    /// Placement / interaction, on the tick: consume a buffered secondary-button press
    /// once. Right-clicking a placed interactable block (crafting table, furnace, chest)
    /// opens its screen rather than placing into the cell — unless sneaking, which falls
    /// through so the player can still build against it.
    fn tick_place(&mut self, events: &mut TickEvents) {
        if !std::mem::take(&mut self.pending_place) {
            return;
        }
        let interacted = !self.intent_sneak && self.try_open_interactable();
        if !interacted && self.try_place() {
            events.placed_block = true;
        }
    }

    /// Spawn the items dropped this frame into the world — on the tick, so a drop enters
    /// the world deterministically like every other entity (the item already left the
    /// inventory per frame; it materialises here). Spawned before the tick's item physics
    /// so a fresh drop gets its first physics step this tick.
    fn tick_drops(&mut self) {
        for drop in std::mem::take(&mut self.pending_drops) {
            self.world.spawn_item(drop);
        }
    }

    /// If the look target is an interactable block, request its screen and return
    /// `true` (consuming the right-click). A crafting table opens the 3×3 grid; a
    /// furnace opens the furnace screen at that position.
    fn try_open_interactable(&mut self) -> bool {
        let Some(h) = self.look else { return false };
        match Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z)) {
            Block::CraftingTable => {
                self.request_open_table = true;
                true
            }
            Block::Furnace => {
                self.request_open_furnace = Some(h.block);
                true
            }
            Block::Chest => {
                self.request_open_chest = Some(h.block);
                true
            }
            _ => false,
        }
    }

    fn try_place(&mut self) -> bool {
        let Some(h) = self.look else { return false };
        if h.normal == IVec3::ZERO {
            return false;
        }

        let block = match self.player.inventory.selected() {
            Some(stack) => match stack.item.as_block() {
                Some(b) if b != Block::Air => b,
                _ => return false,
            },
            None => return false,
        };

        // Right-clicking a replaceable block (short grass, a fern…) while holding a block
        // places straight INTO its cell, overwriting it with no drop — the block just
        // disappears, as if the cell were empty. Otherwise the placement builds against
        // the clicked face. Air is replaceable too (a placement may overwrite it) but is
        // never itself a raycast hit, so exclude it. `p` then feeds the torch support
        // gate, the model footprint, and the final replaceable check uniformly.
        let looked_at = Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
        let replacing_in_place = looked_at.is_replaceable() && looked_at != Block::Air;
        let p = if replacing_in_place {
            h.block
        } else {
            h.block + h.normal
        };

        // A torch only mounts on a floor or wall (never a ceiling) and needs a full solid
        // face to attach to. Resolve that up front so an invalid spot is a no-op (the
        // click neither places nor consumes the torch) rather than leaving a floating one.
        // When REPLACING a plant the torch always drops to the FLOOR of that cell — so
        // right-clicking grass from any angle, even its side, stands a floor torch where
        // the grass was instead of failing on the side face's would-be wall mount.
        let torch_placement = if block == Block::Torch {
            let tp = if replacing_in_place {
                TorchPlacement::Floor
            } else {
                match TorchPlacement::from_place_normal(h.normal) {
                    Some(tp) => tp,
                    None => return false,
                }
            };
            let s = tp.support_cell(p);
            if !Block::from_id(self.world.chunk_block(s.x, s.y, s.z)).is_opaque() {
                return false;
            }
            Some(tp)
        } else {
            None
        };

        // A bbmodel block places its WHOLE footprint (the workbench is 2×2×1): every
        // occupied cell must be loaded + replaceable AND clear of the player/mobs, or the
        // placement fails as a unit (nothing placed, the held item kept). Multi-cell
        // models, and models marked directionalView, are oriented from the player's
        // facing; `p` is the front-left bottom anchor from the player's view.
        if let RenderShape::Model(kind) = block.render_shape() {
            let player_facing = facing_from_forward(self.cam.forward());
            let multi_cell = crate::block_model::instance(kind).cells.len() > 1;
            let facing = if block.directional_view() || multi_cell {
                player_facing
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            let base = if block.directional_view() || multi_cell {
                crate::block_model::base_from_front_left_anchor(p, kind, facing)
            } else {
                p
            };
            if !self.world.model_footprint_clear_facing(base, kind, facing) {
                return false;
            }
            let blocked = crate::block_model::oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .any(|(c, off)| {
                    self.player.intersects_block(c)
                        || self.world.mobs().any_overlapping_boxes(
                            c,
                            crate::block_model::collision_boxes_oriented(kind, off, facing),
                        )
                });
            if !blocked && self.world.place_model_block_facing(base, block, facing) {
                self.player.inventory.decrement_selected();
                return true;
            }
            return false;
        }

        // Substrate gate: a block that roots in a particular ground — a flower in soil, a
        // cactus in sand, a mushroom on soil or stone — places only when the cell directly
        // below is a ground it accepts (`can_root_on`). Blocks with no such rule (almost
        // all of them) accept anything; a torch is gated by its own opaque-face check
        // above. Staying put once placed is the separate job of the FRAGILE behaviour.
        let below = Block::from_id(self.world.chunk_block(p.x, p.y - 1, p.z));
        if !block.can_root_on(below) {
            return false;
        }

        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        // A block with no collision box (a torch, grass, a fern, …) traps nothing, so it
        // may be placed inside an entity; a block that WOULD collide can't be placed where
        // it overlaps the player or a mob — the placement simply fails (the click does
        // nothing and the held item isn't consumed).
        let collides = block.blocks_movement();
        let clear_of_player = !collides || !self.player.intersects_block(p);
        let clear_of_mobs = !collides || !self.world.mobs().any_overlapping_placement(p, block);
        if target.is_replaceable()
            && clear_of_player
            && clear_of_mobs
            && self.world.set_block_world(p.x, p.y, p.z, block)
        {
            // A placed furnace/chest gets an empty block-entity from the moment it
            // exists. Blocks marked directionalView have their front oriented to face
            // the player; a torch records how it is mounted (floor vs which wall) for
            // the mesher + outline.
            let placed_facing = if block.directional_view() {
                facing_from_forward(self.cam.forward())
            } else {
                crate::block_model::DEFAULT_MODEL_FACING
            };
            if block == Block::Furnace {
                self.world.insert_furnace(p, placed_facing);
            } else if block == Block::Chest {
                self.world.insert_chest(p, placed_facing);
            } else if let Some(tp) = torch_placement {
                self.world.insert_torch(p, tp);
            }
            self.player.inventory.decrement_selected();
            true
        } else {
            false
        }
    }

    /// Drain the blocks the world simulation destroyed this tick (fragile blocks that
    /// lost support or were washed away by water) and give each the same break a player
    /// would — the break-particle burst plus its rolled item drops. Particles are purely
    /// visual (Game-owned), so they're spawned here rather than inside the world tick; the
    /// drops materialise on this tick like every other entity. The block is already gone
    /// (the world cleared the cell), so light is sampled from the now-empty cell — which is
    /// what the burst should glow with.
    fn process_natural_breaks(&mut self) {
        for (pos, block) in self.world.take_natural_breaks() {
            let (light, warm) = self.world.dynamic_light_at_world(pos.x, pos.y, pos.z);
            match block.render_shape() {
                RenderShape::Model(kind) => {
                    self.particles.spawn_break_burst_model(pos, kind, light, warm)
                }
                _ => self.particles.spawn_break_burst_lit(pos, block, light, warm),
            }
            // Fragile blocks are all tier-0 hand-harvestable, so they drop exactly as a
            // hand-break would (short grass yields nothing, a flower/torch yields itself).
            self.spawn_drops(pos, block, light);
        }
    }

    fn spawn_drops(&mut self, pos: IVec3, block: Block, skylight: u8) {
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        for d in block.drop_spec().drops {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            // Probabilistic drops (chance < 1, e.g. a leaf's 10% sapling) roll first;
            // a guaranteed drop (chance 1.0) always passes. Reuses the same seeded
            // hash the count roll uses, so the roll stays deterministic on the tick.
            if d.chance < 1.0 && crate::entity::hash01(self.spawn_counter as u64) >= d.chance {
                continue;
            }
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            // Roll a count in [min, max] (a fixed amount when min == max, e.g. the
            // 2–4 raw copper from copper ore).
            let count = if d.min >= d.max {
                d.min
            } else {
                let r = crate::entity::hash01(self.spawn_counter as u64);
                let span = (d.max - d.min + 1) as f32;
                (d.min + (r * span) as u8).min(d.max)
            };
            if count == 0 {
                continue;
            }
            let stack = ItemStack::new(d.item, count);
            let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = skylight;
            self.world.spawn_item(drop);
        }
    }

    /// Spawn `stack` as a dropped item at the centre of block `pos` (e.g. a broken
    /// furnace scattering its contents). No-op for an empty stack.
    fn spawn_item_stack(&mut self, pos: IVec3, stack: ItemStack, skylight: u8) {
        if stack.is_empty() {
            return;
        }
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
        drop.skylight = skylight;
        self.world.spawn_item(drop);
    }

    /// The closest mob in front of the eye whose AABB the ray enters within `max_dist`
    /// (and within reach), skipping dead corpses. `max_dist` is the block hit distance,
    /// so a mob *behind* the block isn't targeted (the block occludes it).
    fn closest_mob(&self, eye: Vec3, dir: Vec3, max_dist: f32) -> Option<usize> {
        let limit = max_dist.min(player::REACH);
        let mut best: Option<(usize, f32)> = None;
        for (i, m) in self.world.mobs().instances().iter().enumerate() {
            if m.is_dead() {
                continue; // a corpse can't be targeted
            }
            let (min, max) = m.aabb();
            if let Some(t) = player::ray_vs_aabb(eye, dir, min, max) {
                if t <= limit && best.is_none_or(|(_, bt)| t < bt) {
                    best = Some((i, t));
                }
            }
        }
        best.map(|(i, _)| i)
    }

    /// Attack, on the tick: resolve a buffered primary-button press (consumed once, so a
    /// press never lands more than one hit). The damage lands the tick *after* the click —
    /// `pending_attack` is latched per frame and consumed here. Rate-limited by
    /// [`ATTACK_COOLDOWN_TICKS`]: the cooldown counts down one tick at a time and an attack
    /// is refused (no swing, no damage) while it's running, so mashing the button can't
    /// land a hit every tick — only one swing per cooldown connects, so an owl can't be
    /// spam-clicked to death. A swing that connects (a mob hit or a punch at the air) arms
    /// the cooldown and reports `swung_hand`; a click on a block (mining) does neither.
    fn tick_attack(&mut self, events: &mut TickEvents) {
        self.attack_cooldown = self.attack_cooldown.saturating_sub(1);
        // Consume the press whether or not it lands (no queuing past one tick); it only
        // resolves once the cooldown has elapsed.
        if !std::mem::take(&mut self.pending_attack) || self.attack_cooldown != 0 {
            return;
        }
        if self.resolve_attack() {
            self.attack_cooldown = ATTACK_COOLDOWN_TICKS;
            events.swung_hand = true;
        }
    }

    /// Apply one attack swing: damage the targeted mob (rolling the held weapon's damage
    /// and spawning loot if the hit kills it), or — looking at nothing — punch the air.
    /// Returns whether the hand swung (a mob hit or an air punch); a click on a block
    /// doesn't swing (mining is the held action). Reads the `targeted_mob` / `look`
    /// sampled this frame, before any mob tick has shifted indices.
    fn resolve_attack(&mut self) -> bool {
        if let Some(idx) = self.targeted_mob {
            let (lo, hi) = crate::item::attack_damage(self.selected_item());
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let damage = lo + crate::entity::hash01(self.spawn_counter as u64) * (hi - lo);
            let from = self.player.body_center();
            if let Some(death) = self.world.mobs_mut().hurt_mob(idx, damage, from) {
                self.spawn_mob_loot(death);
            }
            true
        } else {
            // No mob: a punch swing only when looking at nothing.
            self.look.is_none()
        }
    }

    /// Roll a dead mob's loot table and scatter the drops at its body. Called the
    /// instant a mob dies (from the attack that killed it), so loot appears "when
    /// killed" while the corpse ragdolls. No-op for a species with no table.
    fn spawn_mob_loot(&mut self, death: DeathDrop) {
        let Some(table) = self.loot.get(crate::mob::def(death.kind).key) else {
            return;
        };
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let stacks = table.roll(self.spawn_counter as u64);
        // Pop from roughly the mob's body centre so drops don't clip into the floor.
        let centre = death.pos + Vec3::new(0.0, 0.3, 0.0);
        for stack in stacks {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = death.skylight;
            self.world.spawn_item(drop);
        }
    }

    /// Per-frame presentation update: only particles, which are a purely visual effect
    /// (they don't touch the world). Everything that simulates the world or its entities —
    /// mob AI/physics AND dropped-item physics — runs on the fixed game tick (see
    /// [`game_tick_step`](Self::game_tick_step)); the renderer interpolates between ticks.
    fn tick_entities(&mut self, dt: f32) {
        self.particles.tick(dt, &self.world);
    }

    /// Per game-tick (20 TPS) item maintenance: advance every drop's lifetime
    /// timer (despawning those past their 5-minute limit) and pull any eligible
    /// drop within the player's pickup radius into the inventory. Driven from the
    /// fixed-tick loop, so both are paced by the simulation clock.
    fn item_pickup_tick(&mut self) {
        self.world.tick_item_lifetime();
        let player_pos = self.player.body_center();
        // Plan first against a cloned inventory, reserving capacity without
        // mutating the real slots. Only requested drops are allowed to magnet.
        let mut planned = self.player.inventory.clone();
        self.world
            .dropped_items_mut()
            .request_pickups(player_pos, |stack| {
                let count = planned.fits_count(stack);
                if count > 0 {
                    let leftover = planned.add(ItemStack::new(stack.item, count));
                    debug_assert!(
                        leftover.is_none(),
                        "fits_count overestimated pickup capacity"
                    );
                }
                count
            });

        // Borrow-split: `dropped_items_mut()` borrows the drops, `self.player`
        // owns the inventory — disjoint `Game` fields, so this type-checks without
        // aliasing. Actual inventory mutation only happens after a requested drop
        // reaches the absorb radius.
        let inventory = &mut self.player.inventory;
        self.world
            .dropped_items_mut()
            .collect_requested_pickups(player_pos, |stack| inventory.add(stack));
    }

    fn refresh_dropped_item_lights_after_world_light_update(&mut self) {
        let revision = self.world.lighting_revision();
        if self.dropped_light_revision == revision {
            return;
        }
        self.world.refresh_item_lights();
        self.dropped_light_revision = revision;
    }

    fn blended_sky_fog_color(&self, x: f32, z: f32) -> [f32; 3] {
        use crate::biome::{blended_fog_color, Biome};

        blended_fog_color(x, z, |wx, wz| {
            if let Some(id) = self.world.column_biome(wx, wz) {
                return Biome::from_id(id);
            }

            self.fallback_world.biome_at(wx, wz)
        })
    }
}

/// The furnace facing for a block placed while looking along `forward`: the front
/// (mouth) points back toward the player — opposite the camera's horizontal look
/// direction — snapped to the nearest cardinal.
fn facing_from_forward(forward: Vec3) -> Facing {
    let (fx, fz) = (-forward.x, -forward.z);
    if fx.abs() >= fz.abs() {
        if fx >= 0.0 {
            Facing::East
        } else {
            Facing::West
        }
    } else if fz >= 0.0 {
        Facing::South
    } else {
        Facing::North
    }
}

/// The 6-bit light level for dynamic geometry at a world position — the brighter of
/// skylight and torch block-light, so the held item, particles, and dropped items
/// are lit by torches just like the static blocks around them.
fn light6_at_pos(world: &World, pos: Vec3) -> u8 {
    light6_at_block(world, voxel_at(pos))
}

fn light6_at_block(world: &World, pos: IVec3) -> u8 {
    world.combined_light6_at_world(pos.x, pos.y, pos.z)
}

fn camera_eye_underwater(world: &World, eye: Vec3) -> bool {
    let cell = voxel_at(eye);
    if Block::from_id(world.chunk_block(cell.x, cell.y, cell.z)) != Block::Water {
        return false;
    }

    // Water above means this is an interior water volume, not the open surface.
    if Block::from_id(world.chunk_block(cell.x, cell.y + 1, cell.z)) == Block::Water {
        return true;
    }

    let surface_y = water_surface_y_at(world, cell, eye.x, eye.z);
    eye.y < surface_y - UNDERWATER_SURFACE_MARGIN
}

fn water_surface_y_at(world: &World, cell: IVec3, eye_x: f32, eye_z: f32) -> f32 {
    if water_fills_cell_at(world, cell.x, cell.y, cell.z) {
        return cell.y as f32 + 1.0;
    }

    let mut h = [[1.0f32; 2]; 2];

    // Match the water mesher's corner-height rule: each top vertex averages the
    // water cells meeting that corner, so flowing water forms one sloped sheet.
    for cx in 0..2i32 {
        for cz in 0..2i32 {
            let mut sum = 0.0;
            let mut cnt = 0;
            for ox in (cx - 1)..=cx {
                for oz in (cz - 1)..=cz {
                    if let Some(height) = fluid_height_at(world, cell.x + ox, cell.y, cell.z + oz) {
                        sum += height;
                        cnt += 1;
                    }
                }
            }
            h[cx as usize][cz as usize] = if cnt == 0 { 1.0 } else { sum / cnt as f32 };
        }
    }

    let fx = (eye_x - cell.x as f32).clamp(0.0, 1.0);
    let fz = (eye_z - cell.z as f32).clamp(0.0, 1.0);
    let z0 = lerp(h[0][0], h[1][0], fx);
    let z1 = lerp(h[0][1], h[1][1], fx);
    cell.y as f32 + lerp(z0, z1, fz)
}

fn fluid_height_at(world: &World, wx: i32, wy: i32, wz: i32) -> Option<f32> {
    if Block::from_id(world.chunk_block(wx, wy, wz)) != Block::Water {
        return None;
    }
    let water_above = Block::from_id(world.chunk_block(wx, wy + 1, wz)) == Block::Water;
    Some(crate::world::water::fluid_height(
        world.water_meta_world(wx, wy, wz),
        water_above,
    ))
}

fn water_fills_cell_at(world: &World, wx: i32, wy: i32, wz: i32) -> bool {
    if Block::from_id(world.chunk_block(wx, wy, wz)) != Block::Water {
        return false;
    }
    let water_above = Block::from_id(world.chunk_block(wx, wy + 1, wz)) == Block::Water;
    crate::world::water::fills_cell(world.water_meta_world(wx, wy, wz), water_above)
}

/// Combined light + warm at the lit face of a just-broken block, for its break
/// particles: the mined face's `(combined, warm)`, or the brightest neighbour when
/// the face is unknown.
fn break_light(world: &World, pos: IVec3, normal: Option<IVec3>) -> (u8, u8) {
    let at = |c: IVec3| world.dynamic_light_at_world(c.x, c.y, c.z);
    if let Some(n) = normal {
        return at(pos + n);
    }

    [
        IVec3::X,
        -IVec3::X,
        IVec3::Y,
        -IVec3::Y,
        IVec3::Z,
        -IVec3::Z,
    ]
    .into_iter()
    .map(|n| at(pos + n))
    .max_by_key(|(combined, _)| *combined)
    .unwrap_or((63, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{ITEM_LIFETIME_TICKS, ITEM_PICKUP_DELAY_TICKS};

    fn game() -> Game {
        Game::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
    }

    /// A hotbar slot filled with one full demo stack, for tests that need the
    /// player holding something (the real starting inventory is empty).
    fn filled_inventory() -> Inventory {
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Dirt, 64));
        inv
    }

    fn hit(pos: IVec3, normal: IVec3) -> RaycastHit {
        RaycastHit {
            block: pos,
            normal,
            outline: SelectionShape::full_block(pos),
        }
    }

    #[test]
    fn closest_mob_targets_in_front_within_reach_skips_block_occluded_and_corpses() {
        let mut game = game();
        install_empty_chunk(&mut game);
        game.cam.pos = Vec3::new(8.0, 66.0, 8.0);
        game.cam.pitch = 0.0; // level look, so the eye ray stays at constant y
        let dir = game.cam.forward();
        // An owl two metres ahead, feet dropped so the eye-level ray crosses its body.
        let mut feet = game.cam.pos + dir * 2.0;
        feet.y -= 0.35;
        assert!(game.world.mobs_mut().spawn(Mob::Owl, feet, 0.0));

        assert_eq!(
            game.closest_mob(game.cam.pos, dir, player::REACH),
            Some(0),
            "a mob in front within reach is targeted"
        );
        assert_eq!(
            game.closest_mob(game.cam.pos, dir, 1.0),
            None,
            "a nearer block (smaller max_dist) occludes the mob"
        );
        // A corpse can't be targeted.
        assert!(game
            .world
            .mobs_mut()
            .hurt_mob(0, 100.0, game.cam.pos)
            .is_some());
        assert_eq!(
            game.closest_mob(game.cam.pos, dir, player::REACH),
            None,
            "a dead mob isn't targeted"
        );
    }

    #[test]
    fn fist_takes_four_hits_to_kill_an_owl() {
        let mut game = game();
        let pos = Vec3::new(8.0, 64.0, 8.0);
        assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
        assert_eq!(crate::item::attack_damage(None), (1.0, 1.0));
        let from = pos + Vec3::X;
        for i in 0..3 {
            assert!(
                game.world.mobs_mut().hurt_mob(0, 1.0, from).is_none(),
                "fist hit {i} isn't lethal"
            );
        }
        assert!(
            game.world.mobs_mut().hurt_mob(0, 1.0, from).is_some(),
            "the 4th fist hit kills"
        );
    }

    #[test]
    fn attack_lands_next_tick_then_locks_out_for_the_cooldown() {
        let mut game = game();
        assert!(game
            .world
            .mobs_mut()
            .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
        game.targeted_mob = Some(0);
        let mut ev = TickEvents::default();

        // A click resolves on the tick (the tick after it was registered).
        game.pending_attack = true;
        game.tick_attack(&mut ev);
        assert!(ev.swung_hand, "the click lands on the tick");

        // For the rest of the cooldown, a fresh click each tick lands nothing — even
        // spamming can't beat the 6-tick gate.
        for _ in 0..ATTACK_COOLDOWN_TICKS - 1 {
            ev.swung_hand = false;
            game.pending_attack = true;
            game.tick_attack(&mut ev);
            assert!(!ev.swung_hand, "locked out during the cooldown");
        }

        // The cooldown has now elapsed, so a pending click connects again.
        ev.swung_hand = false;
        game.pending_attack = true;
        game.tick_attack(&mut ev);
        assert!(ev.swung_hand, "the cooldown elapsed, the next attack lands");

        // Only two fist hits (1 dmg each) landed across all those ticks, so the 4-health
        // owl is still alive: the gate makes a spam-click instakill impossible.
        assert!(
            !game.world.mobs().instances()[0].is_dead(),
            "rate-limited, so the owl survives the burst"
        );
    }

    #[test]
    fn opening_a_screen_drops_a_latched_action_so_it_cant_fire_behind_the_menu() {
        let mut game = game();
        assert!(game
            .world
            .mobs_mut()
            .spawn(Mob::Owl, Vec3::new(8.0, 64.0, 8.0), 0.0));
        game.targeted_mob = Some(0);

        // A click latches while playing...
        let click = GameInput {
            gameplay_enabled: true,
            attack_clicked: true,
            ..Default::default()
        };
        game.capture_intent(&click);
        assert!(game.pending_attack, "the click latched while playing");

        // ...then a screen takes input focus before any tick ran. The latched press is
        // dropped, so the tick that still runs behind the menu lands no attack.
        let menu = GameInput {
            gameplay_enabled: false,
            ..Default::default()
        };
        game.capture_intent(&menu);
        assert!(
            !game.pending_attack,
            "opening a screen drops the latched press"
        );
        let mut ev = TickEvents::default();
        game.tick_attack(&mut ev);
        assert!(!ev.swung_hand, "no attack fires behind the open menu");
    }

    #[test]
    fn a_killed_mob_ragdolls_then_despawns() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let pos = Vec3::new(8.0, 64.0, 8.0);
        assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
        assert!(game
            .world
            .mobs_mut()
            .hurt_mob(0, 100.0, pos + Vec3::X)
            .is_some());
        assert_eq!(
            game.world.mobs().len(),
            1,
            "the corpse is present while ragdolling"
        );
        let player_pos = game.player.body_center();
        let player_body = crate::mob::Body::new(game.player.pos, player::HALF_W, player::HEIGHT);
        // 1.5 s ragdoll lifetime at 20 TPS = 30 ticks; run extra for margin.
        for _ in 0..50 {
            game.world.tick_mobs(TICK_DT, player_pos, Some(player_body));
        }
        assert_eq!(
            game.world.mobs().len(),
            0,
            "the corpse despawns once the ragdoll finishes"
        );
    }

    #[test]
    fn killing_owls_drops_loot_into_the_world() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let pos = Vec3::new(8.0, 64.0, 8.0);
        // Over many kills the owl table (50% sticks / 25% coal) virtually always yields
        // something — this proves the death→loot path is wired, without pinning the
        // (freely-editable) table contents.
        for _ in 0..40 {
            assert!(game.world.mobs_mut().spawn(Mob::Owl, pos, 0.0));
            let idx = game.world.mobs().len() - 1;
            if let Some(death) = game.world.mobs_mut().hurt_mob(idx, 100.0, pos + Vec3::X) {
                game.spawn_mob_loot(death);
            }
        }
        assert!(
            !game.world.item_entities().is_empty(),
            "killing owls drops loot via the loot table"
        );
    }

    fn install_empty_chunk(game: &mut Game) {
        let pos = crate::chunk::ChunkPos::new(0, 0);
        game.world.clear_world();
        game.world
            .insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
    }

    fn set_test_water(game: &mut Game, pos: IVec3, meta: u8) {
        let chunk = game
            .world
            .chunk_mut_for_test(crate::chunk::ChunkPos::new(pos.x >> 4, pos.z >> 4))
            .expect("test chunk must be installed");
        chunk.set_water(
            (pos.x & 0x0F) as usize,
            pos.y as usize,
            (pos.z & 0x0F) as usize,
            Block::Water,
            meta,
        );
    }

    #[test]
    fn underwater_shader_uses_flowing_water_surface_height() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(4, 64, 4);
        set_test_water(&mut game, p, 1); // flowing edge: the thinnest water film

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + 0.5);
        assert!(!game.environment(0.0).underwater);

        let surface = p.y as f32 + crate::world::water::fluid_height(1, false);
        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN - 0.01,
            p.z as f32 + 0.5,
        );
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn underwater_shader_waits_until_confidently_below_source_surface() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(5, 64, 5);
        set_test_water(&mut game, p, 0);

        let surface = p.y as f32 + crate::world::water::fluid_height(0, false);
        game.cam.pos = Vec3::new(p.x as f32 + 0.5, surface + 0.01, p.z as f32 + 0.5);
        assert!(!game.environment(0.0).underwater);

        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN * 0.5,
            p.z as f32 + 0.5,
        );
        assert!(!game.environment(0.0).underwater);

        game.cam.pos = Vec3::new(
            p.x as f32 + 0.5,
            surface - UNDERWATER_SURFACE_MARGIN - 0.01,
            p.z as f32 + 0.5,
        );
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn capped_water_cell_is_underwater_even_near_its_top() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(6, 64, 6);
        set_test_water(&mut game, p, 0);
        set_test_water(&mut game, p + IVec3::Y, 0);

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.99, p.z as f32 + 0.5);
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn underwater_shader_treats_falling_water_as_full_height() {
        const FALLING_META: u8 = 0x80;

        let mut game = game();
        install_empty_chunk(&mut game);
        let p = IVec3::new(7, 64, 7);
        set_test_water(&mut game, p, FALLING_META);

        game.cam.pos = Vec3::new(p.x as f32 + 0.5, p.y as f32 + 0.5, p.z as f32 + 0.5);
        assert!(game.environment(0.0).underwater);
    }

    #[test]
    fn spawn_drops_dirt_yields_one_drop() {
        let mut game = game();
        assert!(game.world.item_entities().is_empty());
        game.spawn_drops(IVec3::new(2, 3, 4), Block::Dirt, 17);
        assert_eq!(game.world.item_entities().len(), 1);
        let d = &game.world.item_entities()[0];
        assert_eq!(d.stack.item, crate::item::ItemType::Dirt);
        assert_eq!(d.stack.count, 1);
        assert_eq!(d.skylight, 17);
        assert!((d.pos.x - 2.5).abs() < 1e-5);
        assert!((d.pos.y - 3.5).abs() < 1e-5);
        assert!((d.pos.z - 4.5).abs() < 1e-5);
    }

    #[test]
    fn dropped_item_is_picked_up_near_player() {
        let mut game = game();
        let item = crate::item::ItemType::Poppy;
        let before = count_item(&game.player.inventory, item);
        let centre = game.player.body_center();
        let mut drop = DroppedItem::new(centre, ItemStack::new(item, 1), 1);
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // past the pickup delay
        game.world.spawn_item(drop);
        game.item_pickup_tick();
        let after = count_item(&game.player.inventory, item);
        assert_eq!(after, before + 1);
        assert!(game.world.item_entities().is_empty());
    }

    #[test]
    fn partial_pickup_takes_what_fits_and_leaves_the_rest() {
        let mut game = game();
        // Room for exactly one more dirt: 63 dirt in one slot, every other slot
        // full of a different item.
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Dirt, 63));
        for _ in 0..(crate::inventory::TOTAL_SLOTS - 1) {
            inv.add(ItemStack::new(ItemType::Stone, 64));
        }
        game.player.inventory = inv;

        let centre = game.player.body_center();
        let mut drop = DroppedItem::new(centre, ItemStack::new(ItemType::Dirt, 5), 1);
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
        game.world.spawn_item(drop);

        // One tick plans the partial pickup and absorbs the requested split because
        // the stack is already inside the pickup radius.
        game.item_pickup_tick();

        assert_eq!(
            count_item(&game.player.inventory, ItemType::Dirt),
            64,
            "took exactly the one dirt that fit"
        );
        let loose: u32 = game
            .world
            .item_entities()
            .iter()
            .filter(|d| d.stack.item == ItemType::Dirt)
            .map(|d| d.stack.count as u32)
            .sum();
        assert_eq!(
            loose, 4,
            "the four that didn't fit stay in the world, not discarded"
        );
    }

    #[test]
    fn pickup_planning_reserves_capacity_before_magnetizing() {
        let mut game = game();
        // Room for exactly one dirt, but two dirt drops are inside the attract
        // radius. Planning should request only one of them.
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(ItemType::Dirt, 63));
        for _ in 0..(crate::inventory::TOTAL_SLOTS - 1) {
            inv.add(ItemStack::new(ItemType::Stone, 64));
        }
        game.player.inventory = inv;

        let chest = game.player.body_center();
        for (seed, offset) in [
            (1, Vec3::new(crate::entity::ATTRACT_RADIUS - 0.1, 0.0, 0.0)),
            (2, Vec3::new(0.0, 0.0, crate::entity::ATTRACT_RADIUS - 0.1)),
        ] {
            let mut drop =
                DroppedItem::new(chest + offset, ItemStack::new(ItemType::Dirt, 1), seed);
            drop.vel = Vec3::ZERO;
            drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS;
            game.world.spawn_item(drop);
        }

        game.item_pickup_tick();

        assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 63);
        let requested: u32 = game
            .world
            .item_entities()
            .iter()
            .filter(|d| d.pickup_requested)
            .map(|d| d.stack.count as u32)
            .sum();
        assert_eq!(requested, 1, "only the item that fits is requested");
    }

    #[test]
    fn fresh_dropped_item_waits_out_pickup_delay() {
        let mut game = game();
        let item = crate::item::ItemType::Poppy;
        let centre = game.player.body_center();
        // ticks_lived 0: sitting right on the player but still inside the delay.
        game.world
            .spawn_item(DroppedItem::new(centre, ItemStack::new(item, 1), 1));
        game.item_pickup_tick();
        assert_eq!(
            game.world.item_entities().len(),
            1,
            "delay blocks immediate pickup"
        );
        // Each tick ages it by one; once past the delay it is collected.
        for _ in 0..ITEM_PICKUP_DELAY_TICKS {
            game.item_pickup_tick();
        }
        assert!(game.world.item_entities().is_empty());
    }

    #[test]
    fn dropped_item_magnets_toward_player_then_absorbs() {
        let mut game = game();
        let item = crate::item::ItemType::Poppy;
        let before = count_item(&game.player.inventory, item);
        let chest = game.player.body_center();
        let start = chest + Vec3::new(0.0, crate::entity::ATTRACT_RADIUS - 0.1, 0.0);
        let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // skip the delay so the magnet engages now
        game.world.spawn_item(drop);
        let d0 = (game.world.item_entities()[0].pos - chest).length();
        game.item_pickup_tick();
        assert!(game.world.item_entities()[0].pickup_requested);
        let pp = game.player.body_center();
        game.world.tick_item_physics(TICK_DT, pp);
        if !game.world.item_entities().is_empty() {
            let d1 = (game.world.item_entities()[0].pos - chest).length();
            assert!(d1 < d0);
        }
        // Item physics + pickup both run on the fixed tick now: the magnet flies it in,
        // and the pickup absorbs it once it's in range.
        for _ in 0..60 {
            if game.world.item_entities().is_empty() {
                break;
            }
            game.item_pickup_tick();
            let pp = game.player.body_center();
            game.world.tick_item_physics(TICK_DT, pp);
        }
        assert!(game.world.item_entities().is_empty());
        assert_eq!(count_item(&game.player.inventory, item), before + 1);
    }

    #[test]
    fn a_dropped_item_enters_the_world_on_the_tick_not_the_frame() {
        let mut game = game();
        game.player.inventory = filled_inventory(); // a stack of Dirt
        game.player.inventory.set_active(0);
        let before = count_item(&game.player.inventory, ItemType::Dirt);

        // Q-drop: the item leaves the inventory now, but isn't a world entity yet.
        game.drop_selected_item(false);
        assert_eq!(
            count_item(&game.player.inventory, ItemType::Dirt),
            before - 1,
            "left the inventory this frame"
        );
        assert!(
            game.world.item_entities().is_empty(),
            "the drop hasn't entered the world until a tick runs"
        );

        // The tick materialises the queued drop as a world entity.
        game.tick_drops();
        assert_eq!(
            game.world.item_entities().len(),
            1,
            "the drop spawns on the tick"
        );
    }

    #[test]
    fn container_edits_apply_on_the_tick_not_the_frame() {
        let mut game = game();
        game.player.inventory = filled_inventory(); // a stack of Dirt in hotbar slot 0

        // Left-click that slot: it should pick the stack onto the cursor — but that's a
        // container edit, so it's latched, not applied this frame.
        game.menu_click(
            MenuSlot::Inventory(0),
            crate::controls::PointerButton::Primary,
            false,
            false,
        );
        assert!(
            game.player.inventory.cursor().is_none(),
            "the click hasn't applied yet — no cursor pickup this frame"
        );

        // The tick applies it, moving the stack onto the cursor.
        game.tick_menu();
        assert!(
            game.player.inventory.cursor().is_some(),
            "the tick applies the container edit (the stack is now on the cursor)"
        );
    }

    #[test]
    fn dropped_item_beyond_one_block_is_not_magnet_picked_up() {
        let mut game = game();
        let item = crate::item::ItemType::Poppy;
        let before = count_item(&game.player.inventory, item);
        let chest = game.player.body_center();
        let start = chest + Vec3::new(crate::entity::ATTRACT_RADIUS + 0.05, 0.0, 0.0);
        let mut drop = DroppedItem::new(start, ItemStack::new(item, 1), 1);
        drop.vel = Vec3::ZERO;
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible for pickup, so only range gates it
        game.world.spawn_item(drop);

        for _ in 0..60 {
            let pp = game.player.body_center();
            game.world.tick_item_physics(TICK_DT, pp);
            game.item_pickup_tick();
        }

        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(count_item(&game.player.inventory, item), before);
    }

    #[test]
    fn distant_dropped_item_is_not_picked_up() {
        let mut game = game();
        let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
        let mut drop = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 2);
        drop.ticks_lived = ITEM_PICKUP_DELAY_TICKS; // eligible, but far out of range
        game.world.spawn_item(drop);
        game.item_pickup_tick();
        assert_eq!(game.world.item_entities().len(), 1);
    }

    #[test]
    fn stationary_dropped_item_resamples_after_chunk_light_bake_installs() {
        let mut game = game();
        game.world.clear_world();

        let pos = crate::chunk::ChunkPos::new(0, 0);
        game.world
            .insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
        game.dropped_light_revision = game.world.lighting_revision();

        let mut drop = DroppedItem::new(
            Vec3::new(1.5, 5.5, 1.5),
            ItemStack::new(crate::item::ItemType::Dirt, 1),
            4,
        );
        drop.vel = Vec3::ZERO;
        drop.skylight = 0;
        game.world.spawn_item(drop);

        let before = game.world.lighting_revision();
        for _ in 0..200 {
            game.world.tick_mesh_budget(1);
            game.refresh_dropped_item_lights_after_world_light_update();
            if game.world.lighting_revision() != before {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        assert_ne!(game.world.lighting_revision(), before);
        assert_eq!(game.world.item_entities()[0].skylight, 63);
    }

    #[test]
    fn stale_dropped_item_despawns_on_the_lifetime_tick() {
        let mut game = game();
        let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
        let mut item = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 3);
        // One tick short of the lifetime limit: the next fixed tick ages it out.
        item.ticks_lived = ITEM_LIFETIME_TICKS - 1;
        game.world.spawn_item(item);
        game.item_pickup_tick();
        assert!(game.world.item_entities().is_empty());
    }

    #[test]
    fn throwing_cursor_stack_spawns_a_dropped_item() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        // Drag a stack onto the cursor first.
        game.player.inventory.click_slot(0);
        let held = game
            .player
            .inventory
            .cursor()
            .expect("cursor holds a stack")
            .count;
        assert!(game.world.item_entities().is_empty());
        game.throw_cursor_stack();
        assert!(game.player.inventory.cursor().is_none(), "cursor emptied");
        game.tick_drops(); // the queued drop materialises in the world on the tick
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, held);
        assert_eq!(
            game.world.item_entities()[0].ticks_lived,
            0,
            "thrown item starts the pickup delay"
        );
    }

    #[test]
    fn throwing_one_from_cursor_drops_a_single_item() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.click_slot(0);
        let held = game.player.inventory.cursor().unwrap().count;
        game.throw_cursor_one();
        game.tick_drops(); // the queued drop materialises in the world on the tick
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, 1);
        assert_eq!(game.player.inventory.cursor().unwrap().count, held - 1);
    }

    #[test]
    fn cursor_has_stack_tracks_the_held_stack() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        assert!(!game.cursor_has_stack(), "nothing held initially");
        game.player.inventory.click_slot(0); // pick up hotbar slot 0
        assert!(game.cursor_has_stack(), "holding a stack after pickup");
    }

    #[test]
    fn closing_cursor_stack_uses_empty_inventory_slot_after_matching_stacks() {
        let mut game = game();
        let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
        slots[4] = None;
        game.player.inventory =
            Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

        game.close_cursor_stack();

        assert!(game.player.inventory.cursor().is_none());
        assert_eq!(
            game.player.inventory.slot(4),
            Some(&ItemStack::new(ItemType::Dirt, 12))
        );
        game.tick_drops();
        assert!(
            game.world.item_entities().is_empty(),
            "stashed cursor stack should not drop"
        );
    }

    #[test]
    fn closing_cursor_stack_queues_a_drop_when_inventory_is_full() {
        let mut game = game();
        let slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
        game.player.inventory =
            Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

        game.close_cursor_stack();

        assert!(game.player.inventory.cursor().is_none());
        assert!(
            game.world.item_entities().is_empty(),
            "drop waits for the next tick"
        );
        game.tick_drops();
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(
            game.world.item_entities()[0].stack,
            ItemStack::new(ItemType::Dirt, 12)
        );
    }

    #[test]
    fn closing_cursor_stack_fills_matching_partials_then_drops_leftover() {
        let mut game = game();
        let mut slots = [Some(ItemStack::new(ItemType::Stone, 64)); crate::inventory::TOTAL_SLOTS];
        slots[2] = Some(ItemStack::new(ItemType::Dirt, 60));
        slots[10] = Some(ItemStack::new(ItemType::Dirt, 63));
        game.player.inventory =
            Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 12)), 0);

        game.close_cursor_stack();

        assert!(game.player.inventory.cursor().is_none());
        assert_eq!(
            game.player.inventory.slot(2),
            Some(&ItemStack::new(ItemType::Dirt, 64))
        );
        assert_eq!(
            game.player.inventory.slot(10),
            Some(&ItemStack::new(ItemType::Dirt, 64))
        );
        assert!(
            game.world.item_entities().is_empty(),
            "leftover drop waits for the next tick"
        );
        game.tick_drops();
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(
            game.world.item_entities()[0].stack,
            ItemStack::new(ItemType::Dirt, 7)
        );
    }

    #[test]
    fn collect_to_cursor_tops_up_from_hotbar_and_grid() {
        use crate::inventory::{Inventory, TOTAL_SLOTS};
        let mut game = game();
        // Cursor holds a partial Dirt stack; matching partials sit in the hotbar
        // and the main grid, with an unrelated stack that must be left alone.
        let mut slots = [None; TOTAL_SLOTS];
        slots[2] = Some(ItemStack::new(ItemType::Dirt, 20)); // hotbar
        slots[crate::inventory::HOTBAR_LEN] = Some(ItemStack::new(ItemType::Dirt, 30)); // main grid
        slots[5] = Some(ItemStack::new(ItemType::Stone, 64)); // untouched
        game.player.inventory =
            Inventory::from_parts(slots, Some(ItemStack::new(ItemType::Dirt, 5)), 0);

        game.collect_to_cursor();

        // 5 + 20 + 30 = 55 onto the cursor, both dirt sources emptied.
        assert_eq!(game.inventory().cursor().unwrap().count, 55);
        assert!(game.inventory().slot(2).is_none());
        assert!(game
            .inventory()
            .slot(crate::inventory::HOTBAR_LEN)
            .is_none());
        assert_eq!(game.inventory().slot(5).unwrap().item, ItemType::Stone);
    }

    #[test]
    fn throwing_with_empty_cursor_is_a_noop() {
        let mut game = game();
        game.player.inventory = crate::inventory::Inventory::new();
        assert!(game.player.inventory.cursor().is_none());
        game.throw_cursor_stack();
        game.throw_cursor_one();
        assert!(game.world.item_entities().is_empty());
    }

    #[test]
    fn drop_selected_one_throws_a_single_held_item() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        let before = game.player.inventory.selected().unwrap().count;
        game.drop_selected_item(false);
        game.tick_drops(); // the queued drop materialises in the world on the tick
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, 1);
        assert_eq!(
            game.world.item_entities()[0].ticks_lived,
            0,
            "dropped item starts the pickup delay"
        );
        assert_eq!(game.player.inventory.selected().unwrap().count, before - 1);
    }

    #[test]
    fn drop_selected_all_throws_the_whole_held_stack() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        let before = game.player.inventory.selected().unwrap().count;
        game.drop_selected_item(true);
        game.tick_drops(); // the queued drop materialises in the world on the tick
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, before);
        assert!(
            game.player.inventory.selected().is_none(),
            "held slot emptied"
        );
    }

    #[test]
    fn drop_with_empty_hand_is_a_noop() {
        let mut game = game();
        game.player.inventory = crate::inventory::Inventory::new();
        game.player.inventory.set_active(0);
        assert!(game.player.inventory.selected().is_none());
        game.drop_selected_item(false);
        game.drop_selected_item(true);
        assert!(game.world.item_entities().is_empty());
    }

    #[test]
    fn throwing_an_item_arms_the_hand_place_jab() {
        // The Q drop throws from the active hotbar slot.
        {
            let mut game = game();
            game.player.inventory = filled_inventory();
            game.player.inventory.set_active(0);
            assert!(!game.threw_item);
            game.drop_selected_item(false);
            assert!(game.threw_item, "Q drop should flick the hand forward");
        }
        // Both inventory drag-outs throw from the cursor-held stack.
        for throw in [
            Game::throw_cursor_stack as fn(&mut Game),
            Game::throw_cursor_one,
        ] {
            let mut game = game();
            game.player.inventory = filled_inventory();
            game.player.inventory.click_slot(0); // pick the stack onto the cursor
            assert!(!game.threw_item);
            throw(&mut game);
            assert!(
                game.threw_item,
                "inventory drag-out should flick the hand forward"
            );
        }
    }

    #[test]
    fn a_noop_throw_does_not_arm_the_place_jab() {
        let mut game = game();
        game.player.inventory = crate::inventory::Inventory::new();
        // Nothing in hand or on the cursor: every throw path is a no-op.
        for _ in 0..64 {
            game.player.inventory.decrement_selected();
        }
        game.drop_selected_item(false);
        game.throw_cursor_stack();
        game.throw_cursor_one();
        assert!(!game.threw_item, "an empty throw must not animate the hand");
    }

    #[test]
    fn tick_reports_then_clears_the_throw_event() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        game.drop_selected_item(false);

        let events = game.tick(1.0 / 60.0, &GameInput::default());
        assert!(events.threw_item, "the frame after a throw reports it");
        let next = game.tick(1.0 / 60.0, &GameInput::default());
        assert!(!next.threw_item, "the throw event is one-shot");
    }

    #[test]
    fn place_with_empty_hand_does_nothing() {
        let mut game = game();
        // The starting inventory is already empty.
        assert!(game.player.inventory.selected().is_none());
        game.look = Some(hit(IVec3::new(0, 40, 0), IVec3::Y));
        assert!(!game.try_place());
    }

    #[test]
    fn place_into_loaded_air_decrements_selected() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.world.update_load(0, 0);
        let mut loaded = false;
        for _ in 0..500 {
            game.world.poll();
            if game.world.chunk_loaded(0, 0) {
                loaded = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(loaded);

        let p = IVec3::new(0, 200, 0);
        assert!(Block::from_id(game.world.chunk_block(p.x, p.y, p.z)).is_replaceable());
        game.player.inventory.set_active(0);
        let item = game.player.inventory.selected().unwrap().item;
        let block = item.as_block().unwrap();
        let before = game.player.inventory.selected().unwrap().count;

        game.look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y));
        assert!(game.try_place());

        assert_eq!(Block::from_id(game.world.chunk_block(p.x, p.y, p.z)), block);
        assert_eq!(game.player.inventory.selected().unwrap().count, before - 1);
    }

    #[test]
    fn placing_into_replaceable_grass_overwrites_it_with_no_drop() {
        // Right-clicking short grass (a replaceable plant) while holding a block places
        // the block straight INTO the grass cell, overwriting it with no drop — not on
        // top of it.
        let mut game = game();
        install_empty_chunk(&mut game);
        game.player.inventory = filled_inventory(); // a stack of Dirt
        game.player.inventory.set_active(0);
        game.player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of the cell

        let g = IVec3::new(8, 100, 8);
        game.world.set_block_world(g.x, g.y, g.z, Block::ShortGrass);
        let before = game.player.inventory.selected().unwrap().count;

        // Look straight at the grass and place into it.
        game.look = Some(hit(g, IVec3::Y));
        assert!(game.try_place(), "placing into replaceable grass succeeds");

        assert_eq!(
            Block::from_id(game.world.chunk_block(g.x, g.y, g.z)),
            Block::Dirt,
            "the block replaced the grass in its own cell, not the cell above"
        );
        assert_eq!(
            game.player.inventory.selected().unwrap().count,
            before - 1,
            "one block was consumed"
        );
        assert!(
            game.item_entities().is_empty(),
            "the overwritten grass dropped nothing"
        );
    }

    #[test]
    fn rooted_plants_place_only_on_their_required_ground() {
        // The data-driven substrate gate: a flower roots in soil (grass/dirt), a cactus
        // in sand (sand/red sand). Building onto the wrong ground is a no-op; the right
        // ground accepts it. Each case uses its own column so they don't interfere.
        fn place_on(game: &mut Game, ground: Block, item: ItemType, col: i32) -> bool {
            let g = IVec3::new(col, 100, col);
            game.world.set_block_world(g.x, g.y, g.z, ground);
            let mut inv = Inventory::new();
            inv.add(ItemStack::new(item, 1));
            game.player.inventory = inv;
            game.player.inventory.set_active(0);
            game.look = Some(hit(g, IVec3::Y)); // build on TOP of the ground block
            let placed = game.try_place();
            // The return must agree with whether the block actually landed above.
            let above = Block::from_id(game.world.chunk_block(g.x, g.y + 1, g.z));
            assert_eq!(
                placed,
                above == item.as_block().unwrap(),
                "try_place() return must match whether the block landed"
            );
            placed
        }

        let mut game = game();
        install_empty_chunk(&mut game);
        game.player.pos = Vec3::new(100.0, 64.0, 100.0); // park clear of every cell

        // A flower (Dandelion) roots in soil only.
        assert!(!place_on(&mut game, Block::Stone, ItemType::Dandelion, 2), "no flower on stone");
        assert!(place_on(&mut game, Block::Grass, ItemType::Dandelion, 4), "flower on grass");
        assert!(place_on(&mut game, Block::Dirt, ItemType::Dandelion, 6), "flower on dirt");
        assert!(!place_on(&mut game, Block::Sand, ItemType::Dandelion, 8), "no flower on sand");

        // A cactus roots in sand only.
        assert!(!place_on(&mut game, Block::Grass, ItemType::Cactus, 10), "no cactus on grass");
        assert!(place_on(&mut game, Block::Sand, ItemType::Cactus, 12), "cactus on sand");
        assert!(place_on(&mut game, Block::RedSand, ItemType::Cactus, 14), "cactus on red sand");

        // A mushroom roots in soil OR any stone (its two RootsIn* tags combine).
        assert!(place_on(&mut game, Block::Grass, ItemType::BrownMushroom, 1), "mushroom on grass");
        assert!(place_on(&mut game, Block::Stone, ItemType::BrownMushroom, 3), "mushroom on stone");
        assert!(place_on(&mut game, Block::Cobblestone, ItemType::BrownMushroom, 5), "mushroom on cobblestone");
        assert!(!place_on(&mut game, Block::Sand, ItemType::BrownMushroom, 7), "no mushroom on sand");
        assert!(!place_on(&mut game, Block::OakPlanks, ItemType::BrownMushroom, 9), "no mushroom on wood");
    }

    #[test]
    fn a_mob_pushes_the_player_per_frame() {
        // The player is shoved out of an overlapping mob every frame (not on the tick),
        // so the drift is smooth. An owl just east of the player pushes it west.
        let mut game = game();
        game.player.pos = Vec3::new(8.0, 64.0, 8.0);
        assert!(game
            .world
            .mobs_mut()
            .spawn(Mob::Owl, Vec3::new(8.2, 64.0, 8.0), 0.0));
        let x0 = game.player.pos.x;
        for _ in 0..30 {
            game.apply_mob_push(1.0 / 60.0);
        }
        assert!(
            game.player.pos.x < x0 - 0.05,
            "the owl pushed the player -X, away from it: {x0} -> {}",
            game.player.pos.x
        );
    }

    #[test]
    fn cannot_place_a_solid_block_inside_a_mob() {
        let mut game = game();
        install_empty_chunk(&mut game);
        game.player.inventory = filled_inventory(); // a stack of Dirt
        game.player.inventory.set_active(0);
        // Park the player far off so only the mob can block placement here.
        game.player.pos = Vec3::new(100.0, 64.0, 100.0);

        // An owl standing in cell (8, 200, 8), high up and clear of the player.
        assert!(game
            .world
            .mobs_mut()
            .spawn(Mob::Owl, Vec3::new(8.5, 200.0, 8.5), 0.0));

        // Aiming a Dirt block into the owl's cell does nothing: no block lands and the
        // held stack isn't consumed.
        let before = game.player.inventory.selected().unwrap().count;
        game.look = Some(hit(IVec3::new(8, 199, 8), IVec3::Y)); // p = (8, 200, 8)
        assert!(
            !game.try_place(),
            "a solid block can't be placed inside the owl"
        );
        assert_eq!(
            Block::from_id(game.world.chunk_block(8, 200, 8)),
            Block::Air,
            "nothing was placed"
        );
        assert_eq!(
            game.player.inventory.selected().unwrap().count,
            before,
            "the held item wasn't consumed"
        );

        // A cell clear of the owl (and the player) places as usual.
        game.look = Some(hit(IVec3::new(0, 199, 0), IVec3::Y)); // p = (0, 200, 0)
        assert!(game.try_place(), "an empty cell places normally");
        assert_eq!(
            Block::from_id(game.world.chunk_block(0, 200, 0)),
            Block::Dirt
        );
    }

    fn count_item(inv: &Inventory, item: ItemType) -> u32 {
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| inv.slot(i))
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    #[test]
    fn stone_pickaxe_harvests_iron_as_raw_iron() {
        // Mining only spawns drops when harvested; the drop item comes from the
        // block's drop spec. Iron ore yields raw iron (here via spawn_drops, which
        // the mining path calls on a harvested break).
        let mut game = game();
        game.spawn_drops(IVec3::new(0, 64, 0), Block::IronOre, 15);
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.item, ItemType::RawIron);
    }

    #[test]
    fn copper_ore_drops_two_to_four_raw_copper() {
        let mut game = game();
        game.spawn_drops(IVec3::new(1, 64, 1), Block::CopperOre, 15);
        let drops = game.world.item_entities();
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].stack.item, ItemType::RawCopper);
        assert!((2..=4).contains(&drops[0].stack.count), "2–4 raw copper");
    }

    #[test]
    fn furnace_front_faces_the_player_on_placement() {
        // The front points opposite the look direction (back toward the player).
        assert_eq!(facing_from_forward(Vec3::new(0.0, 0.0, 1.0)), Facing::North);
        assert_eq!(
            facing_from_forward(Vec3::new(0.0, 0.0, -1.0)),
            Facing::South
        );
        assert_eq!(facing_from_forward(Vec3::new(1.0, 0.0, 0.0)), Facing::West);
        assert_eq!(facing_from_forward(Vec3::new(-1.0, 0.0, 0.0)), Facing::East);
        // A pitched, mostly-horizontal look snaps to the dominant horizontal axis.
        assert_eq!(
            facing_from_forward(Vec3::new(0.2, -0.9, 0.95)),
            Facing::North
        );
    }
}
