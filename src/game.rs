//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

use crate::block::Block;
use crate::camera::Camera;
use crate::crafting::{load_recipes, CraftGrid, Recipes};
use crate::entity::{DroppedItem, ParticleSystem};
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{lerp, IVec3, SelectionShape, Vec3};
use crate::mining::MiningState;
use crate::player::{self, Input, Player, PlayerMode, RaycastHit};
use crate::render::{BreakOverlayView, ItemEntityInstance, ParticleInstance};
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

/// Deep, murky blue the world fades to (fog + clear colour) when the camera eye
/// is underwater.
const UNDERWATER_FOG_COLOR: [f32; 3] = [0.04, 0.16, 0.30];

/// Require the camera eye to sit this far below an open water surface before the
/// underwater shader/fog kicks in. This keeps shallow flowing films from tinting
/// the view when the eye is only barely clipping their rendered surface.
const UNDERWATER_SURFACE_MARGIN: f32 = 0.03;

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

/// Fixed simulation timestep: 20 game ticks per second, independent of frame
/// rate. World simulation (block updates, scheduled ticks, water flow) advances
/// in whole steps of this size.
const TICK_DT: f32 = 0.05;

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time —
/// it just runs the late tick and reschedules from now (per the design).
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
    /// Edge state: secondary button pressed for placement.
    pub place_clicked: bool,
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct GameEvents {
    pub placed_block: bool,
    pub broke_block: bool,
    /// An item/stack left the hand for the world this frame — a Q drop or an
    /// inventory drag-out. Drives the same first-person place jab as a placement.
    pub threw_item: bool,
    /// The player right-clicked a placed crafting table this frame. The app shell
    /// reacts by opening the 3×3 crafting screen (the game can't own screens).
    pub open_crafting_table: bool,
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
    /// Block currently under the crosshair, refreshed each tick.
    look: Option<RaycastHit>,
    mining: MiningState,
    /// Tracks the world's lighting revision so item-entity skylight is only
    /// recomputed when a world light bake actually changed. The drops themselves
    /// live in `World` (with their chunks); this is just the change detector.
    dropped_light_revision: u64,
    particles: ParticleSystem,
    spawn_counter: u32,
    mining_dust_t: f32,
    item_entity_instances: Vec<ItemEntityInstance>,
    particle_instances: Vec<ParticleInstance>,
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    tick_accumulator: f32,
    /// Wall-clock seconds since the last background autosave.
    autosave_t: f32,
    /// Set when the hand expels an item into the world (Q drop or inventory
    /// drag-out) so the next [`tick`](Self::tick) reports it for the hand's place
    /// jab. Consumed (reset) each tick.
    threw_item: bool,
    /// Loaded crafting recipes (from `assets/recipes.json`), used to compute the
    /// crafting result preview.
    recipes: Recipes,
    /// The active crafting grid (2×2 in the inventory, 3×3 at a table) + its
    /// cached result. Empty whenever no crafting screen is open.
    craft: CraftGrid,
    /// Set when the player right-clicks a placed crafting table, so the next
    /// [`tick`](Self::tick) asks the app shell to open the 3×3 screen. One-shot.
    request_open_table: bool,
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
        let mut player = match &level {
            Some(l) => {
                let mut p = Player::new(l.player_pos);
                p.set_mode(l.player_mode);
                p.vel = l.player_vel; // assign after set_mode, which zeroes vel
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
        // Centre chunk streaming on the real player position from frame one.
        cam.pos = Vec3::new(player.pos.x, player.pos.y + player::EYE, player.pos.z);

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
            mining: MiningState::new(),
            dropped_light_revision: 0,
            particles: ParticleSystem::new(),
            spawn_counter: 0,
            mining_dust_t: 0.0,
            item_entity_instances: Vec::new(),
            particle_instances: Vec::new(),
            tick_accumulator: 0.0,
            autosave_t: 0.0,
            threw_item: false,
            recipes: load_recipes(),
            craft: CraftGrid::new(),
            request_open_table: false,
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
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.tick_world();

        self.look = Player::raycast(self.cam.pos, self.cam.forward(), &self.world);
        let (placed_block, broke_block) = self.handle_block_actions(dt, input);

        self.run_fixed_ticks(dt);

        self.tick_entities(dt);
        self.tick_mesh_budget();
        self.refresh_dropped_item_lights_after_world_light_update();

        self.maybe_autosave(dt);

        GameEvents {
            placed_block,
            broke_block,
            // Throws happen via input handlers before this tick; report and clear.
            threw_item: std::mem::take(&mut self.threw_item),
            // Set this tick by handle_block_actions when a table was right-clicked.
            open_crafting_table: std::mem::take(&mut self.request_open_table),
        }
    }

    /// Advance the world simulation in fixed 50 ms steps, decoupled from the
    /// frame rate: 0 steps on a fast frame, several to catch up on a slow one
    /// (capped — never two running at once, the late one just runs and the clock
    /// resyncs). Player movement, camera, and rendering stay per-frame above.
    fn run_fixed_ticks(&mut self, dt: f32) {
        // Ignore absurd deltas (first frame, tab regaining focus) so a long pause
        // doesn't dump a burst of ticks; clamp keeps at most one step pending.
        self.tick_accumulator += dt.clamp(0.0, 1.0);
        let mut ran = 0;
        while self.tick_accumulator >= TICK_DT && ran < MAX_TICKS_PER_FRAME {
            self.world.game_tick();
            // Item collection is paced by the simulation clock, not the frame
            // rate: "a game tick decides the player picks it up".
            self.item_pickup_tick();
            self.tick_accumulator -= TICK_DT;
            ran += 1;
        }
        if self.tick_accumulator > TICK_DT {
            self.tick_accumulator = TICK_DT;
        }
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

    pub fn click_inventory_slot(&mut self, slot: usize) {
        self.player.inventory.click_slot(slot);
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

    /// Right-click an inventory slot: split the slot's stack onto the cursor, or
    /// drip one held item into the slot. See [`Inventory::right_click_slot`].
    pub fn right_click_inventory_slot(&mut self, slot: usize) {
        self.player.inventory.right_click_slot(slot);
    }

    /// Shift-click an inventory slot: shuttle the stack between the hotbar and the
    /// main grid. See [`Inventory::shift_move_slot`].
    pub fn shift_click_inventory_slot(&mut self, slot: usize) {
        self.player.inventory.shift_move_slot(slot);
    }

    /// The active crafting grid (for the UI to read cells + result preview).
    #[inline]
    pub fn craft_grid(&self) -> &CraftGrid {
        &self.craft
    }

    /// Configure the crafting grid for a screen of `cols×cols` (2 = inventory,
    /// 3 = table) and clear it. Called when a crafting screen opens.
    pub fn open_crafting(&mut self, cols: usize) {
        self.craft.reset(cols);
        self.craft.recompute(&self.recipes);
    }

    /// Close the crafting grid: return every input item to the inventory (any
    /// overflow is thrown into the world), then clear the result.
    pub fn close_crafting(&mut self) {
        for i in 0..self.craft.capacity() {
            if let Some(stack) = self.craft.take_cell(i) {
                if let Some(leftover) = self.player.inventory.add(stack) {
                    self.throw_item(leftover);
                }
            }
        }
        self.craft.recompute(&self.recipes);
    }

    /// Left-click a crafting input cell (cursor pick/drop/merge/swap), then
    /// refresh the result preview.
    pub fn craft_click_slot(&mut self, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        self.player
            .inventory
            .click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(&self.recipes);
    }

    /// Right-click a crafting input cell (split / place-one), then refresh.
    pub fn craft_right_click_slot(&mut self, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        self.player
            .inventory
            .right_click_external_slot(self.craft.cell_mut(i));
        self.craft.recompute(&self.recipes);
    }

    /// Shift-click a crafting input cell: move its whole stack to the inventory
    /// (whatever doesn't fit stays in the cell), then refresh.
    pub fn craft_shift_slot(&mut self, i: usize) {
        if i >= self.craft.capacity() {
            return;
        }
        if let Some(stack) = self.craft.take_cell(i) {
            if let Some(leftover) = self.player.inventory.add(stack) {
                *self.craft.cell_mut(i) = Some(leftover);
            }
        }
        self.craft.recompute(&self.recipes);
    }

    /// Take one craft from the result slot onto the cursor: places the result on
    /// the cursor (stacking onto a matching held stack with room) and consumes one
    /// item from every occupied input cell. No-op if there's no result or the
    /// cursor can't accept the whole result.
    pub fn craft_take_result(&mut self) {
        let Some(result) = self.craft.result().copied() else {
            return;
        };
        if self.player.inventory.try_stack_onto_cursor(result) {
            self.craft.consume_one();
            self.craft.recompute(&self.recipes);
        }
    }

    /// Shift-click the result: craft as many times as possible straight into the
    /// inventory, stopping when an ingredient runs out or the next result won't
    /// fully fit. The hotbar/main grid both receive results (via `add`).
    pub fn craft_shift_result(&mut self) {
        // Bounded by the grid contents: each craft consumes ≥1 from every cell.
        for _ in 0..(64 * crate::crafting::MAX_CELLS) {
            let Some(result) = self.craft.result().copied() else {
                break;
            };
            if !self.player.inventory.can_add(result) {
                break;
            }
            self.player.inventory.add(result);
            self.craft.consume_one();
            self.craft.recompute(&self.recipes);
        }
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

    /// Spawn `stack` as a thrown item flying out along the camera's look
    /// direction, originating just in front of the eye so it clears the player.
    fn throw_item(&mut self, stack: ItemStack) {
        let dir = self.cam.forward();
        let origin = self.cam.pos + dir * 0.3;
        let mut drop = DroppedItem::thrown(origin, stack, dir);
        drop.skylight = sky6_at_pos(&self.world, origin);
        self.world.spawn_item(drop);
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
        self.mining
            .overlay()
            .map(|(block, stage)| BreakOverlayView { block, stage })
    }

    pub fn item_entity_instances(&mut self) -> &[ItemEntityInstance] {
        self.map_item_entities();
        &self.item_entity_instances
    }

    pub fn particle_instances(&mut self) -> &[ParticleInstance] {
        self.map_particles();
        &self.particle_instances
    }

    pub fn held_item_skylight(&self) -> u8 {
        sky6_at_pos(&self.world, self.cam.pos)
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
        self.cam.rotate(-dx * SENS, -dy * SENS);
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

    fn handle_block_actions(&mut self, dt: f32, input: &GameInput) -> (bool, bool) {
        // The held pickaxe tier (0 = hand) gates mining speed + whether drops fall.
        let tool_tier = self
            .player
            .inventory
            .selected()
            .map_or(0, |s| s.item.pickaxe_tier());

        if !input.gameplay_enabled {
            self.mining.update(
                dt,
                self.look.as_ref(),
                input.break_held,
                true,
                &self.world,
                tool_tier,
            );
            self.mining_dust_t = 0.0;
            return (false, false);
        }

        let mut broke_block = false;
        if let Some(event) = self.mining.update(
            dt,
            self.look.as_ref(),
            input.break_held,
            false,
            &self.world,
            tool_tier,
        ) {
            broke_block = true;
            let hit_normal = self
                .look
                .filter(|h| h.block == event.pos && h.normal != IVec3::ZERO)
                .map(|h| h.normal);
            let light = break_light(&self.world, event.pos, hit_normal);
            self.world
                .set_block_world(event.pos.x, event.pos.y, event.pos.z, Block::Air);
            self.particles
                .spawn_break_burst_lit(event.pos, event.block, light);
            if event.harvested {
                self.spawn_drops(event.pos, event.block, light);
            }
        }

        if self.mining.is_mining() {
            if let Some(h) = self.look {
                self.mining_dust_t += dt;
                if self.mining_dust_t >= MINING_DUST_INTERVAL {
                    self.mining_dust_t = 0.0;
                    let block =
                        Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z));
                    let light = sky6_at_block(&self.world, h.block + h.normal);
                    self.particles
                        .spawn_mining_lit(h.block, h.normal, block, light);
                }
            }
        } else {
            self.mining_dust_t = 0.0;
        }

        // Right-click a placed crafting table to open it (interact) rather than
        // placing into the cell — unless sneaking, which falls through so the
        // player can still build on top of a table.
        let open_table =
            input.place_clicked && !input.movement.sneak && self.targeting_crafting_table();
        if open_table {
            self.request_open_table = true;
        }
        let placed = !open_table && input.place_clicked && self.try_place();
        (placed, broke_block)
    }

    /// Whether the current look target is a placed crafting table.
    fn targeting_crafting_table(&self) -> bool {
        match self.look {
            Some(h) => {
                Block::from_id(self.world.chunk_block(h.block.x, h.block.y, h.block.z))
                    == Block::CraftingTable
            }
            None => false,
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

        let p = h.block + h.normal;
        let target = Block::from_id(self.world.chunk_block(p.x, p.y, p.z));
        if target.is_replaceable()
            && !self.player.intersects_block(p)
            && self.world.set_block_world(p.x, p.y, p.z, block)
        {
            self.player.inventory.decrement_selected();
            true
        } else {
            false
        }
    }

    fn spawn_drops(&mut self, pos: IVec3, block: Block, skylight: u8) {
        let centre = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5);
        for d in block.drop_spec().drops {
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

    /// Per-frame entity update: item-entity physics (gravity, collision, pickup
    /// magnet) then particles. The drops live in `World` and the magnet target is
    /// the player chest; the per-item pickup delay is enforced inside the world
    /// step. Lifetime/despawn and the actual pickup run on the fixed tick.
    fn tick_entities(&mut self, dt: f32) {
        let player_pos = self.player.body_center();
        self.world.tick_item_physics(dt, player_pos);
        self.particles.tick(dt, &self.world);
    }

    /// Per game-tick (20 TPS) item maintenance: advance every drop's lifetime
    /// timer (despawning those past their 5-minute limit) and pull any eligible
    /// drop within the player's pickup radius into the inventory. Driven from the
    /// fixed-tick loop, so both are paced by the simulation clock.
    fn item_pickup_tick(&mut self) {
        self.world.tick_item_lifetime();
        let player_pos = self.player.body_center();
        // Borrow-split: the world owns the drops, the player owns the inventory.
        let inventory = &mut self.player.inventory;
        self.world
            .collect_item_pickups(player_pos, |stack| inventory.add(stack));
    }

    fn refresh_dropped_item_lights_after_world_light_update(&mut self) {
        let revision = self.world.lighting_revision();
        if self.dropped_light_revision == revision {
            return;
        }
        self.world.refresh_item_lights();
        self.dropped_light_revision = revision;
    }

    fn map_item_entities(&mut self) {
        self.item_entity_instances.clear();
        self.item_entity_instances
            .extend(
                self.world
                    .item_entities()
                    .iter()
                    .map(|d| ItemEntityInstance {
                        pos: d.pos,
                        item: d.stack.item,
                        count: d.stack.count,
                        spin: d.spin,
                        skylight: d.skylight,
                    }),
            );
    }

    fn map_particles(&mut self) {
        self.particle_instances.clear();
        self.particle_instances
            .extend(self.particles.particles().iter().map(|p| {
                let (uv_min, uv_size) = p.atlas_uv();
                ParticleInstance {
                    pos: p.pos,
                    uv_min,
                    uv_size,
                    tint: p.tint,
                    alpha: p.alpha(),
                    size: p.render_size(),
                    skylight: p.skylight,
                }
            }));
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

fn sky6_at_pos(world: &World, pos: Vec3) -> u8 {
    sky6_at_block(world, voxel_at(pos))
}

fn sky6_at_block(world: &World, pos: IVec3) -> u8 {
    world.skylight6_at_world(pos.x, pos.y, pos.z)
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

fn break_light(world: &World, pos: IVec3, normal: Option<IVec3>) -> u8 {
    if let Some(n) = normal {
        return sky6_at_block(world, pos + n);
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
    .map(|n| sky6_at_block(world, pos + n))
    .max()
    .unwrap_or(63)
}

fn voxel_at(pos: Vec3) -> IVec3 {
    IVec3::new(
        pos.x.floor() as i32,
        pos.y.floor() as i32,
        pos.z.floor() as i32,
    )
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

    fn install_empty_chunk(game: &mut Game) {
        let pos = crate::chunk::ChunkPos::new(0, 0);
        game.world.chunks.clear();
        game.world.meshes.clear();
        game.world.pending.clear();
        game.world
            .chunks
            .insert(pos, crate::chunk::Chunk::new(0, 0));
    }

    fn set_test_water(game: &mut Game, pos: IVec3, meta: u8) {
        let chunk = game
            .world
            .chunks
            .get_mut(&crate::chunk::ChunkPos::new(pos.x >> 4, pos.z >> 4))
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
        game.tick_entities(1.0 / 60.0);
        if !game.world.item_entities().is_empty() {
            let d1 = (game.world.item_entities()[0].pos - chest).length();
            assert!(d1 < d0);
        }
        // Magnet flies it in per frame; the fixed tick absorbs it once in range.
        for _ in 0..60 {
            if game.world.item_entities().is_empty() {
                break;
            }
            game.tick_entities(1.0 / 60.0);
            game.item_pickup_tick();
        }
        assert!(game.world.item_entities().is_empty());
        assert_eq!(count_item(&game.player.inventory, item), before + 1);
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
            game.tick_entities(1.0 / 60.0);
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
        game.world.chunks.clear();
        game.world.meshes.clear();
        game.world.pending.clear();

        let pos = crate::chunk::ChunkPos::new(0, 0);
        game.world
            .chunks
            .insert(pos, crate::chunk::Chunk::new(0, 0));
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
    fn map_item_entities_one_instance_per_drop() {
        let mut game = game();
        game.world.spawn_item(DroppedItem::new(
            Vec3::new(1.0, 2.0, 3.0),
            ItemStack::new(crate::item::ItemType::Dirt, 1),
            1,
        ));
        game.world.spawn_item(DroppedItem::new(
            Vec3::new(4.0, 5.0, 6.0),
            ItemStack::new(crate::item::ItemType::Stone, 1),
            2,
        ));
        game.map_item_entities();
        assert_eq!(game.item_entity_instances.len(), 2);
        assert_eq!(
            game.item_entity_instances[0].pos,
            game.world.item_entities()[0].pos
        );
        assert_eq!(
            game.item_entity_instances[0].item,
            crate::item::ItemType::Dirt
        );
        assert_eq!(
            game.item_entity_instances[0].spin,
            game.world.item_entities()[0].spin
        );
        assert_eq!(
            game.item_entity_instances[1].item,
            crate::item::ItemType::Stone
        );
    }

    #[test]
    fn map_item_entities_reuses_the_vec_without_growth() {
        let mut game = game();
        for i in 0..8 {
            game.world.spawn_item(DroppedItem::new(
                Vec3::splat(i as f32),
                ItemStack::new(crate::item::ItemType::Dirt, 1),
                i,
            ));
        }
        game.map_item_entities();
        let cap = game.item_entity_instances.capacity();
        game.world.item_entities_mut().truncate(2);
        game.map_item_entities();
        assert_eq!(game.item_entity_instances.len(), 2);
        assert_eq!(game.item_entity_instances.capacity(), cap);
    }

    #[test]
    fn map_particles_one_instance_per_alive_particle() {
        let mut game = game();
        game.particles
            .spawn_break_burst(IVec3::new(0, 64, 0), Block::Dirt);
        let alive = game.particles.particles().len();
        assert!(alive > 0);
        game.map_particles();
        assert_eq!(game.particle_instances.len(), alive);
        let (uv_min, uv_size) = game.particles.particles()[0].atlas_uv();
        assert_eq!(game.particle_instances[0].uv_min, uv_min);
        assert_eq!(game.particle_instances[0].uv_size, uv_size);
        assert_eq!(
            game.particle_instances[0].size,
            game.particles.particles()[0].size
        );
    }

    fn count_item(inv: &Inventory, item: ItemType) -> u32 {
        (0..crate::inventory::TOTAL_SLOTS)
            .filter_map(|i| inv.slot(i))
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Put `stack` into the first craft cell by routing it through the cursor
    /// (inventory slot 0 → cursor → craft cell), as the UI clicks would.
    fn place_in_craft_cell(game: &mut Game, cell: usize, stack: ItemStack) {
        game.add_to_inventory(stack);
        game.click_inventory_slot(0); // pick the stack onto the cursor
        game.craft_click_slot(cell); // drop it into the craft cell
    }

    #[test]
    fn crafting_planks_from_log_via_result_slot() {
        let mut game = game();
        game.open_crafting(2);
        place_in_craft_cell(&mut game, 0, ItemStack::new(ItemType::OakLog, 1));
        assert_eq!(
            game.craft_grid().result().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        // Take the result: 4 planks onto the cursor, the log consumed, no result.
        game.craft_take_result();
        assert_eq!(
            game.inventory().cursor().map(|s| (s.item, s.count)),
            Some((ItemType::OakPlanks, 4))
        );
        assert!(game.craft_grid().result().is_none());
        assert!(game.craft_grid().is_empty());
    }

    #[test]
    fn shift_crafting_consumes_every_log_in_the_cell() {
        let mut game = game();
        game.open_crafting(2);
        // A cell holding 3 logs shift-crafts three times (one log per craft).
        place_in_craft_cell(&mut game, 0, ItemStack::new(ItemType::OakLog, 3));
        game.craft_shift_result();
        assert!(game.craft_grid().is_empty(), "all logs consumed");
        assert_eq!(count_item(&game.player.inventory, ItemType::OakPlanks), 12);
    }

    #[test]
    fn closing_crafting_returns_grid_items_to_inventory() {
        let mut game = game();
        game.open_crafting(3);
        place_in_craft_cell(&mut game, 4, ItemStack::new(ItemType::OakLog, 5));
        assert!(game.inventory().cursor().is_none());
        game.close_crafting();
        assert_eq!(count_item(&game.player.inventory, ItemType::OakLog), 5);
        assert!(game.craft_grid().cell(4).is_none());
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
}
