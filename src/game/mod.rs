//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

mod breaking;
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

#[cfg(test)]
use crate::block::Block;
use crate::camera::{Camera, Frustum};
use crate::crafting::Recipes;
#[cfg(test)]
use crate::entity::DroppedItem;
use crate::entity::ParticleSystem;
#[cfg(test)]
use crate::furnace::Facing;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
#[cfg(test)]
use crate::mathh::SelectionShape;
use crate::mathh::{voxel_at, IVec3, Vec3};
use crate::mining::MiningState;
use crate::mob::LootTables;
#[cfg(test)]
use crate::mob::Mob;
#[cfg(test)]
use crate::player;
use crate::player::{Player, PlayerMode, RaycastHit};
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

pub use crate::gui::MenuSlot;
pub use container::{ContainerMenu, ContainerTarget};
use drops::DropQueue;
pub(crate) use environment::GameEnvironment;
pub(crate) use frame::CameraPose;
pub use menu::MenuReadModel;
#[cfg(test)]
use placement::facing_from_forward;
pub use tick::{GameEvents, GameInput, MovementInput};

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

/// Minimum number of game ticks between two attack swings, so a player mashing the
/// attack button can't land hits every tick (which would, e.g., instakill an owl).
/// Counted in ticks now that attacks resolve on the fixed tick — 6 ticks ≈ 0.3 s.
const ATTACK_COOLDOWN_TICKS: u32 = 6;

/// Chest-lid open/close speed (fraction per second)
const CHEST_LID_SPEED: f32 = 3.5;

/// Door swing open/close speed (fraction per second). A touch slower than the chest
/// lid so the 90° swing reads as a deliberate door, not a snap.
const DOOR_SWING_SPEED: f32 = 4.5;

/// How near (blocks) a mob/dropped item must be — and it must also be in the camera
/// frustum — for its animation to force a redraw. Past this it's too small on screen to
/// read, so it keeps simulating but doesn't hold the frame rate up while the player idles.
const ENTITY_ACTIVITY_RANGE: f32 = 50.0;

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

    #[inline]
    fn selected_item(&self) -> Option<ItemType> {
        self.player.inventory.selected().map(|s| s.item)
    }

    /// The transient open progress (`0.0` closed .. `1.0` open) of the chest at
    /// `pos`, or `0.0` if it isn't tracked. The presentation snapshot reads this
    /// to bake the chest's lid hinge; the easing/animation lives in
    /// [`advance_chest_lids`](Self::advance_chest_lids).
    #[inline]
    fn chest_lid_angle(&self, pos: IVec3) -> f32 {
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

    /// The transient swing angle (`0.0` closed .. `1.0` open) of the door whose LOWER
    /// cell is `lower`. While a door is mid-swing the eased value is read from
    /// [`door_swings`](Self::door_swings); once it settles the entry is dropped and the
    /// door rests at its logical open state (read straight from the door map). The
    /// presentation snapshot calls this per visible door to bake its hinge.
    #[inline]
    fn door_swing_angle(&self, lower: IVec3) -> f32 {
        if let Some(&a) = self.door_swings.get(&lower) {
            return a;
        }
        // Not animating: rest at the door's logical state.
        match self.world.door_state_at(lower.x, lower.y, lower.z) {
            Some(s) if s.open => 1.0,
            _ => 0.0,
        }
    }

    /// Advance the transient door-swing animation by `dt`: each tracked door eases
    /// toward its current logical open state (flipped on the tick by [`World::toggle_door`]),
    /// and a door that reaches its target is dropped (it then rests at that state). Purely
    /// client-side, never saved — like [`advance_chest_lids`](Self::advance_chest_lids).
    fn advance_door_swings(&mut self, dt: f32) {
        let step = (dt * DOOR_SWING_SPEED).clamp(0.0, 1.0);
        self.door_swings.retain(|&lower, angle| {
            let target = match self.world.door_state_at(lower.x, lower.y, lower.z) {
                Some(s) if s.open => 1.0,
                Some(_) => 0.0,
                // The door was removed while swinging — stop tracking it.
                None => return false,
            };
            if *angle < target {
                *angle = (*angle + step).min(target);
            } else if *angle > target {
                *angle = (*angle - step).max(target);
            }
            // Keep only while still travelling toward the target.
            (*angle - target).abs() > f32::EPSILON
        });
    }

    /// Fraction (`0..1`) into the next fixed tick — the blend factor the scene uses to
    /// interpolate each entity's render pose between its previous and current tick, so the
    /// mobs and dropped items (which simulate at 20 TPS) move smoothly at any frame rate.
    #[inline]
    fn tick_alpha(&self) -> f32 {
        (self.tick_accumulator / tick::TICK_DT).clamp(0.0, 1.0)
    }

    /// Whether anything on screen is currently moving or pending, so the app shell knows
    /// this frame would differ from the last and must be drawn. Covers a mob or dropped
    /// item that is both close and in view (see [`entity_animating_in_view`]), live
    /// particles, an in-progress mining crack, chest lids mid-swing, and chunks still
    /// awaiting a (re)mesh. Camera motion, raw input, and open-menu interaction are tracked
    /// by the shell; slow sky/fog drift by the host's keep-alive redraw. A mob behind the
    /// player or far off keeps simulating but does NOT hold the frame at full rate.
    ///
    /// [`entity_animating_in_view`]: Self::entity_animating_in_view
    fn is_visually_active(&self) -> bool {
        !self.particles.is_empty()
            || self.mining.is_mining()
            || self.world.has_dirty_meshes()
            || self
                .chest_lids
                .values()
                .any(|&lid| lid > f32::EPSILON && lid < 1.0)
            || !self.door_swings.is_empty()
            || self.entity_animating_in_view()
    }

    /// Is a mob or dropped item both within [`ENTITY_ACTIVITY_RANGE`] and inside the
    /// camera frustum? Only then does its per-frame animation/interpolation actually
    /// change the rendered image and warrant holding the frame rate up. Off-screen or
    /// distant entities still simulate on the tick — they just don't force redraws,
    /// which is what lets a stationary player idle in a populated overworld.
    fn entity_animating_in_view(&self) -> bool {
        let eye = self.cam.pos;
        let r2 = ENTITY_ACTIVITY_RANGE * ENTITY_ACTIVITY_RANGE;
        let frustum = Frustum::from_view_proj(self.cam.view_proj());
        // A coarse upright box around the entity's feet — generous enough that an entity
        // near a frustum edge isn't missed for the frame it slips into view.
        let visible = |p: Vec3| {
            (p - eye).length_squared() <= r2
                && frustum.aabb_visible(p - Vec3::new(0.5, 0.0, 0.5), p + Vec3::new(0.5, 2.0, 0.5))
        };
        self.world.mobs().instances().iter().any(|m| visible(m.pos))
            || self.world.item_entities().iter().any(|d| visible(d.pos))
    }

    /// Combined light + warm-tint amount at the player's eye, for lighting the
    /// first-person hand / held item — it brightens AND warms near torches/furnaces.
    fn held_item_light(&self) -> (u8, u8) {
        let c = voxel_at(self.cam.pos);
        self.world.dynamic_light_at_world(c.x, c.y, c.z)
    }

    fn tick_mesh_budget(&mut self) {
        const MESH_BUDGET: usize = 32;
        self.world.tick_mesh_budget(MESH_BUDGET);
    }
}

#[cfg(test)]
mod tests {
    use super::tick::{TickEvents, TICK_DT};
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

    fn apply_drop_actions(game: &mut Game) -> TickEvents {
        let mut events = TickEvents::default();
        game.tick_drops(&mut events);
        events
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

        // Q-drop queues intent only; inventory and world stay unchanged until the tick.
        game.drop_selected_item(false);
        assert_eq!(
            count_item(&game.player.inventory, ItemType::Dirt),
            before,
            "inventory mutation waits for the tick"
        );
        assert!(
            game.world.item_entities().is_empty(),
            "the drop hasn't entered the world until a tick runs"
        );

        // The tick removes the item and materialises the drop as a world entity.
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert_eq!(
            count_item(&game.player.inventory, ItemType::Dirt),
            before - 1
        );
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
        assert!(
            game.player.inventory.cursor().is_some(),
            "cursor is not mutated until the tick"
        );
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert!(game.player.inventory.cursor().is_none(), "cursor emptied");
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, held);
        assert_eq!(
            game.world.item_entities()[0].ticks_lived,
            0,
            "thrown item starts the pickup delay"
        );
    }

    #[test]
    fn queued_cursor_stack_throw_survives_menu_close_before_tick() {
        let mut game = game();
        game.player.inventory = Inventory::from_parts(
            [None; crate::inventory::TOTAL_SLOTS],
            Some(ItemStack::new(ItemType::Dirt, 12)),
            0,
        );

        game.throw_cursor_stack();
        assert!(
            game.player.inventory.cursor().is_some(),
            "throwing does not mutate the cursor until another tick/close action"
        );
        game.close_cursor_stack();

        assert!(
            game.player.inventory.cursor().is_none(),
            "the committed cursor throw is not stashed on close"
        );
        assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 0);
        assert!(
            game.world.item_entities().is_empty(),
            "entity spawn still waits for the tick"
        );

        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(
            game.world.item_entities()[0].stack,
            ItemStack::new(ItemType::Dirt, 12)
        );
    }

    #[test]
    fn throwing_one_from_cursor_drops_a_single_item() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.click_slot(0);
        let held = game.player.inventory.cursor().unwrap().count;
        game.throw_cursor_one();
        assert_eq!(
            game.player.inventory.cursor().unwrap().count,
            held,
            "cursor count is unchanged until the tick"
        );
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(game.world.item_entities()[0].stack.count, 1);
        assert_eq!(game.player.inventory.cursor().unwrap().count, held - 1);
    }

    #[test]
    fn queued_cursor_one_throw_stashes_only_remainder_on_menu_close() {
        let mut game = game();
        game.player.inventory = Inventory::from_parts(
            [None; crate::inventory::TOTAL_SLOTS],
            Some(ItemStack::new(ItemType::Dirt, 12)),
            0,
        );

        game.throw_cursor_one();
        assert_eq!(
            game.player.inventory.cursor().unwrap().count,
            12,
            "throwing one does not mutate the cursor immediately"
        );
        game.close_cursor_stack();

        assert!(game.player.inventory.cursor().is_none());
        assert_eq!(
            count_item(&game.player.inventory, ItemType::Dirt),
            11,
            "close stashes only the part not committed to the throw"
        );
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(
            game.world.item_entities()[0].stack,
            ItemStack::new(ItemType::Dirt, 1)
        );
        assert_eq!(count_item(&game.player.inventory, ItemType::Dirt), 11);
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
        apply_drop_actions(&mut game);
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
        apply_drop_actions(&mut game);
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
        apply_drop_actions(&mut game);
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
        assert_eq!(
            game.player.inventory.selected().unwrap().count,
            before,
            "selected stack is not mutated until the tick"
        );
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
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
    fn queued_q_drop_uses_the_action_time_hotbar_slot() {
        let mut game = game();
        let mut slots = [None; crate::inventory::TOTAL_SLOTS];
        slots[0] = Some(ItemStack::new(ItemType::Dirt, 5));
        slots[1] = Some(ItemStack::new(ItemType::Stone, 7));
        game.player.inventory = Inventory::from_parts(slots, None, 0);

        game.drop_selected_item(false);
        game.player.inventory.set_active(1);

        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
        assert_eq!(
            game.player.inventory.slot(0),
            Some(&ItemStack::new(ItemType::Dirt, 4))
        );
        assert_eq!(
            game.player.inventory.slot(1),
            Some(&ItemStack::new(ItemType::Stone, 7)),
            "changing selection before the tick must not redirect the drop"
        );
        assert_eq!(game.world.item_entities().len(), 1);
        assert_eq!(
            game.world.item_entities()[0].stack,
            ItemStack::new(ItemType::Dirt, 1)
        );
    }

    #[test]
    fn drop_selected_all_throws_the_whole_held_stack() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        let before = game.player.inventory.selected().unwrap().count;
        game.drop_selected_item(true);
        assert_eq!(
            game.player.inventory.selected().unwrap().count,
            before,
            "selected stack is not mutated until the tick"
        );
        let events = apply_drop_actions(&mut game);
        assert!(events.threw_item);
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
    fn applying_a_real_throw_arms_the_hand_place_jab() {
        // The Q drop throws from the active hotbar slot.
        {
            let mut game = game();
            game.player.inventory = filled_inventory();
            game.player.inventory.set_active(0);
            game.drop_selected_item(false);
            let events = apply_drop_actions(&mut game);
            assert!(events.threw_item, "Q drop should flick the hand forward");
        }
        // Both inventory drag-outs throw from the cursor-held stack.
        for throw in [
            Game::throw_cursor_stack as fn(&mut Game),
            Game::throw_cursor_one,
        ] {
            let mut game = game();
            game.player.inventory = filled_inventory();
            game.player.inventory.click_slot(0); // pick the stack onto the cursor
            throw(&mut game);
            let events = apply_drop_actions(&mut game);
            assert!(
                events.threw_item,
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
        let events = apply_drop_actions(&mut game);
        assert!(
            !events.threw_item,
            "an empty throw must not animate the hand"
        );
    }

    #[test]
    fn tick_reports_throw_event_only_when_the_drop_is_applied() {
        let mut game = game();
        game.player.inventory = filled_inventory();
        game.player.inventory.set_active(0);
        game.drop_selected_item(false);

        let events = game.tick(1.0 / 60.0, &GameInput::default());
        assert!(
            !events.threw_item,
            "a frame with no fixed tick must not report a queued throw"
        );
        assert!(
            game.player.inventory.selected().is_some(),
            "a frame with no fixed tick must not mutate the inventory"
        );

        let applied = game.tick(TICK_DT, &GameInput::default());
        assert!(applied.threw_item, "the applying tick reports the throw");
        let next = game.tick(TICK_DT, &GameInput::default());
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
    fn right_clicking_interactable_blocks_requests_their_screen() {
        enum ExpectedOpen {
            CraftingTable,
            Furnace,
            Chest,
            FurnitureWorkbench,
        }

        for (block, expected) in [
            (Block::CraftingTable, ExpectedOpen::CraftingTable),
            (Block::Furnace, ExpectedOpen::Furnace),
            (Block::Chest, ExpectedOpen::Chest),
            (Block::FurnitureWorkbench, ExpectedOpen::FurnitureWorkbench),
        ] {
            let mut game = game();
            install_empty_chunk(&mut game);
            let pos = IVec3::new(4, 64, 4);
            game.world.set_block_world(pos.x, pos.y, pos.z, block);
            game.look = Some(hit(pos, IVec3::Y));
            game.pending_place = true;

            let mut events = TickEvents::default();
            game.tick_place(&mut events);

            assert!(
                events.placed_block.is_none(),
                "{block:?} should interact, not place"
            );
            match expected {
                ExpectedOpen::CraftingTable => {
                    assert!(game.request_open_table, "{block:?} should open crafting");
                }
                ExpectedOpen::Furnace => {
                    assert_eq!(game.request_open_furnace, Some(pos), "{block:?}");
                }
                ExpectedOpen::Chest => {
                    assert_eq!(game.request_open_chest, Some(pos), "{block:?}");
                }
                ExpectedOpen::FurnitureWorkbench => {
                    assert_eq!(game.request_open_workbench, Some(pos), "{block:?}");
                }
            }
        }
    }

    #[test]
    fn right_clicking_a_door_toggles_it_through_block_interaction() {
        let mut game = game();
        install_empty_chunk(&mut game);
        let floor = IVec3::new(5, 63, 5);
        let lower = floor + IVec3::Y;
        game.world
            .set_block_world(floor.x, floor.y, floor.z, Block::Stone);
        assert!(game.world.place_door(lower, Block::OakDoor, Facing::South));
        assert!(
            !game
                .world
                .door_state_at(lower.x, lower.y, lower.z)
                .unwrap()
                .open
        );

        game.look = Some(hit(lower, IVec3::Y));
        game.pending_place = true;
        let mut events = TickEvents::default();
        game.tick_place(&mut events);

        assert!(events.placed_block.is_none(), "door click should not place");
        assert!(game.toggled_door.is_some(), "door click should report a toggle event");
        assert!(
            game.world
                .door_state_at(lower.x, lower.y, lower.z)
                .unwrap()
                .open
        );
        let upper = lower + IVec3::Y;
        assert!(
            game.world
                .door_state_at(upper.x, upper.y, upper.z)
                .unwrap()
                .open
        );
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
            game.world.item_entities().is_empty(),
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
        assert!(
            !place_on(&mut game, Block::Stone, ItemType::Dandelion, 2),
            "no flower on stone"
        );
        assert!(
            place_on(&mut game, Block::Grass, ItemType::Dandelion, 4),
            "flower on grass"
        );
        assert!(
            place_on(&mut game, Block::Dirt, ItemType::Dandelion, 6),
            "flower on dirt"
        );
        assert!(
            !place_on(&mut game, Block::Sand, ItemType::Dandelion, 8),
            "no flower on sand"
        );

        // A cactus roots in sand only.
        assert!(
            !place_on(&mut game, Block::Grass, ItemType::Cactus, 10),
            "no cactus on grass"
        );
        assert!(
            place_on(&mut game, Block::Sand, ItemType::Cactus, 12),
            "cactus on sand"
        );
        assert!(
            place_on(&mut game, Block::RedSand, ItemType::Cactus, 14),
            "cactus on red sand"
        );

        // A mushroom roots in soil OR any stone (its two RootsIn* tags combine).
        assert!(
            place_on(&mut game, Block::Grass, ItemType::BrownMushroom, 1),
            "mushroom on grass"
        );
        assert!(
            place_on(&mut game, Block::Stone, ItemType::BrownMushroom, 3),
            "mushroom on stone"
        );
        assert!(
            place_on(&mut game, Block::Cobblestone, ItemType::BrownMushroom, 5),
            "mushroom on cobblestone"
        );
        assert!(
            !place_on(&mut game, Block::Sand, ItemType::BrownMushroom, 7),
            "no mushroom on sand"
        );
        assert!(
            !place_on(&mut game, Block::OakPlanks, ItemType::BrownMushroom, 9),
            "no mushroom on wood"
        );
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
