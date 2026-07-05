//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

mod bed;
mod breaking;
mod client_presentation;
mod container;
mod daynight;
mod drops;
mod entities;
mod environment;
mod frame;
mod health;
mod item_use;
mod local_player;
mod menu;
mod mod_actions;
mod placement;
pub(crate) mod presentation;
mod session;
mod terrain_render;
mod tick;

use std::collections::HashMap;

use crate::block::RenderShape;
use crate::block_state::{HeldBlockState, LogAxis, SlabState, StairHalf, StairState};
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
pub(crate) use tick::TickEvents;
pub use tick::{
    GameEvents, GameInput, MobSoundEvent, ModSound, ModSpatialSoundCommand, MovementInput,
};

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

/// Minimum number of game ticks between two attack swings, so a player mashing the
/// attack button can't land hits every tick (which would, e.g., instakill an owl).
/// Counted in ticks now that attacks resolve on the fixed tick — 6 ticks ≈ 0.3 s.
const ATTACK_COOLDOWN_TICKS: u32 = 6;

pub struct Game {
    cam: Camera,
    /// Visual-only vertical lag after grounded auto-step movement. The player
    /// feet and collision state update immediately; only the camera eases upward.
    camera_step_y_offset: f32,
    last_player_eye_y: f32,
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
    /// Next deterministic session handle for mod-owned spatial sounds. The app
    /// owns playback; this counter only gives mods stable identities for stop calls.
    next_mod_sound_handle: u64,
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
    held_rotation_item: Option<ItemType>,
    held_block_rotation: u8,
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
    /// Set when a block's `open_gui` interaction (pos `Some`) or a mod's
    /// `GuiOpen` HostCall (pos `None`) asks for a mod GUI screen. One-shot
    /// open request (consumed via [`GameEvents`]).
    request_open_mod_gui: Option<(crate::gui::GuiKind, Option<IVec3>)>,
    /// Set by a mod's `GuiClose` HostCall; the app closes the mod GUI screen
    /// if one is up. One-shot (consumed via [`GameEvents`]).
    request_close_mod_gui: bool,
    /// Set when the player right-clicks a bed, so the next [`tick`](Self::tick)
    /// asks the app shell to open the sleep overlay. One-shot open request
    /// (consumed via [`GameEvents`]).
    request_open_sleep: bool,
    /// The in-flight sleep session (`None` = awake). Tick-owned; the overlay
    /// fade reads it through [`sleep_progress01`](Self::sleep_progress01).
    sleep: Option<bed::SleepState>,
    /// App-side wake request (ESC / "Leave bed"), latched to the next tick.
    wake_requested: bool,
    /// App-side respawn request (the death screen), latched to the next tick.
    respawn_requested: bool,
    /// Set when a door was toggled on a tick, so the per-frame [`GameEvents`] can
    /// flick the hand (the toggle itself already applied) and play the open/close
    /// sound. Carries the door's NEW open state. One-shot (consumed via `GameEvents`).
    toggled_door: Option<bool>,
    /// The modding event bus (Phase 1): pre events dispatch at their decision sites,
    /// post events queue and drain at tick-stage boundaries. The engine registers no
    /// handlers yet — the seams exist for mods. See WIKI/modding.md.
    bus: crate::events::EventBus,
    /// Systems attached between the fixed-tick stages (Phase 1 seam).
    systems: crate::events::TickSystems,
    /// The WASM mod instances (Phase 2b). Their registered closures (held by
    /// `bus`/`systems`) share ownership; the host keeps the canonical handles
    /// for GUI click dispatch (Phase 5) and diagnostics.
    mods: crate::modding::ModHost,
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

    /// Test injection: replace the mod host (e.g. with a WAT guest) so the GUI
    /// click dispatch plumbing can be driven without compiled mods.
    #[cfg(test)]
    pub(crate) fn set_mods_for_test(&mut self, mods: crate::modding::ModHost) {
        self.mods = mods;
    }

    #[cfg(test)]
    pub(crate) fn mods_for_test(&self) -> &crate::modding::ModHost {
        &self.mods
    }

    pub fn set_aspect(&mut self, aspect: f32) {
        self.cam.aspect = aspect;
    }

    /// The player's ear (eye) position, for the app layer's distance
    /// attenuation of positional mod sounds.
    #[inline]
    pub fn listener_position(&self) -> crate::mathh::Vec3 {
        self.player.eye()
    }

    /// Current fixed-tick number, exposed for client-side presentation systems
    /// that schedule effects against game tick time without mutating the sim.
    #[inline]
    pub fn current_tick(&self) -> u64 {
        self.world.current_tick()
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
        self.clear_held_block_rotation();
    }

    #[inline]
    fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    pub fn toggle_held_block_rotation(&mut self) {
        let Some(item) = self.selected_item() else {
            self.clear_held_block_rotation();
            return;
        };
        if !item.as_block().is_some_and(rotatable_block) {
            self.clear_held_block_rotation();
            return;
        }
        if self.held_rotation_item == Some(item) {
            let count = item.as_block().map_or(1, rotation_count).max(1);
            self.held_block_rotation = (self.held_block_rotation + 1) % count;
        } else {
            self.held_rotation_item = Some(item);
            self.held_block_rotation = 1 % item.as_block().map_or(1, rotation_count).max(1);
        }
    }

    #[inline]
    fn clear_held_block_rotation(&mut self) {
        self.held_rotation_item = None;
        self.held_block_rotation = 0;
    }

    #[inline]
    fn held_rotation_active(&self) -> bool {
        let Some(item) = self.selected_item() else {
            return false;
        };
        self.held_rotation_item == Some(item)
            && self.held_block_rotation != 0
            && item.as_block().is_some_and(rotatable_block)
    }

    #[inline]
    pub(crate) fn held_block_state(&self) -> HeldBlockState {
        let Some(block) = self.selected_item().and_then(ItemType::as_block) else {
            return HeldBlockState::None;
        };
        if block.render_shape() == RenderShape::Stair {
            return HeldBlockState::Stair(StairState::new(
                crate::block_model::DEFAULT_MODEL_FACING,
                if self.held_rotation_active() {
                    StairHalf::Top
                } else {
                    StairHalf::Bottom
                },
            ));
        }
        if block.render_shape() == RenderShape::Slab {
            let slot = crate::slab::slot_for_rotation(
                self.held_slab_rotation(),
                IVec3::ZERO,
                crate::furnace::Facing::South,
            );
            return HeldBlockState::Slab(SlabState::single(slot.split, slot.index, block));
        }
        if block.is_log() {
            return HeldBlockState::Log(if self.held_rotation_active() {
                LogAxis::X
            } else {
                LogAxis::Y
            });
        }
        HeldBlockState::None
    }

    #[inline]
    pub(crate) fn held_stair_half(&self) -> StairHalf {
        if self.held_rotation_active() {
            StairHalf::Top
        } else {
            StairHalf::Bottom
        }
    }

    #[inline]
    pub(crate) fn held_slab_rotation(&self) -> crate::slab::SlabRotation {
        if self.held_rotation_active() {
            crate::slab::SlabRotation::from_index(self.held_block_rotation)
        } else {
            crate::slab::SlabRotation::Bottom
        }
    }

    #[inline]
    pub(crate) fn held_log_axis_for_facing(&self, facing: crate::furnace::Facing) -> LogAxis {
        if !self.held_rotation_active() {
            return LogAxis::Y;
        }
        match facing {
            crate::furnace::Facing::East | crate::furnace::Facing::West => LogAxis::X,
            crate::furnace::Facing::North | crate::furnace::Facing::South => LogAxis::Z,
        }
    }
}

fn rotatable_block(block: crate::block::Block) -> bool {
    matches!(block.render_shape(), RenderShape::Stair | RenderShape::Slab) || block.is_log()
}

fn rotation_count(block: crate::block::Block) -> u8 {
    if block.render_shape() == RenderShape::Slab {
        3
    } else {
        2
    }
}

#[cfg(test)]
mod tests;
