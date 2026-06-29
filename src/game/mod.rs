//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

mod breaking;
mod client_presentation;
mod container;
mod drops;
mod entities;
mod environment;
mod frame;
mod local_player;
mod menu;
mod placement;
pub(crate) mod presentation;
mod session;
mod terrain_render;
mod tick;

use std::collections::HashMap;

use crate::camera::Camera;
use crate::crafting::Recipes;
use crate::entity::ParticleSystem;
#[cfg(test)]
use crate::inventory::Inventory;
#[cfg(test)]
use crate::item::ItemStack;
use crate::item::ItemType;
use crate::mathh::IVec3;
use crate::mining::MiningState;
use crate::mob::LootTables;
#[cfg(test)]
use crate::player::PlayerMode;
use crate::player::{Player, RaycastHit};
use crate::world::World;
use crate::worldgen::density::surface::SurfaceDensitySystem;

pub use crate::gui::MenuSlot;
use container::ContainerMenu;
use drops::DropQueue;
pub(crate) use environment::GameEnvironment;
pub(crate) use frame::CameraPose;
pub use tick::{GameEvents, GameInput, MovementInput};

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

/// Minimum number of game ticks between two attack swings, so a player mashing the
/// attack button can't land hits every tick (which would, e.g., instakill an owl).
/// Counted in ticks now that attacks resolve on the fixed tick — 6 ticks ≈ 0.3 s.
const ATTACK_COOLDOWN_TICKS: u32 = 6;

pub struct Game {
    cam: Camera,
    world: World,
    fallback_world: SurfaceDensitySystem,
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
    /// Item-drop intents latched by input/menu cleanup and applied on the next fixed
    /// tick. Gameplay-visible inventory/cursor mutation and entity spawning happen
    /// together in [`tick_drops`](Self::tick_drops).
    drop_queue: DropQueue,
    /// Container-menu clicks (slot, button, shift, the App's double-click `gather`
    /// verdict) latched this frame, applied in order on the next tick so chest / furnace /
    /// inventory edits mutate state on the tick. Real clicks are >1 tick apart, so each is
    /// applied before the next is decided. Drained each tick.
    pending_menu_clicks: Vec<(MenuSlot, crate::controls::PointerButton, bool, bool)>,
    /// Transient per-chest lid open angle (`0.0` closed .. `1.0` open), keyed by world
    /// position. Eased toward open for the chest whose screen is up and toward closed
    /// for the rest; client-side animation only, never persisted. The render-side
    /// presentation snapshot reads the angle (via [`Game::chest_lid_angle`]) to bake the lid;
    /// the easing in [`Game::advance_chest_lids`] is the owning sim/animation state.
    chest_lids: HashMap<IVec3, f32>,
    /// Transient per-door swing angle (`0.0` closed .. `1.0` open), keyed by the door's
    /// LOWER cell. A door enters the map when right-click toggles it and is eased toward
    /// its (now flipped) logical open state by [`Game::advance_door_swings`]; once it
    /// reaches the target it is dropped (the renderer then reads the resting angle
    /// straight from the door state). Client-side animation only, never persisted — the
    /// authoritative open/closed bit lives in the chunk door map. See [`crate::door`].
    door_swings: HashMap<IVec3, f32>,
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    tick_accumulator: f32,
    /// Wall-clock seconds since the last background autosave.
    autosave_t: f32,
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
    /// Set to a furniture workbench's position when right-clicked, so the next
    /// [`tick`](Self::tick) asks the app shell to open the workbench screen. One-shot
    /// open request (consumed via [`GameEvents`]).
    request_open_workbench: Option<IVec3>,
    /// Set when a door was toggled on a tick, so the per-frame [`GameEvents`] can
    /// flick the hand (the toggle itself already applied) and play the open/close
    /// sound. Carries the door's NEW open state. One-shot (consumed via `GameEvents`).
    toggled_door: Option<bool>,
}

impl Game {
    #[cfg(test)]
    #[inline]
    pub(crate) fn inventory(&self) -> &Inventory {
        &self.player.inventory
    }

    #[cfg(test)]
    pub(crate) fn add_to_inventory(&mut self, stack: ItemStack) -> Option<ItemStack> {
        self.player.inventory.add(stack)
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        self.cam.aspect = aspect;
    }

    pub fn toggle_player_mode(&mut self) {
        self.player.toggle_mode();
    }

    #[cfg(test)]
    #[inline]
    pub(crate) fn player_mode(&self) -> PlayerMode {
        self.player.mode()
    }

    pub fn set_active_hotbar(&mut self, slot: u8) {
        self.player.inventory.set_active(slot);
    }

    #[inline]
    fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }
}

#[cfg(test)]
mod tests;
