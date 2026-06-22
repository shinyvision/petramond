//! Voxel game simulation and scene state.
//!
//! `Game` owns the world, player, entities, mining, and camera. It does not own
//! platform input, app screens, or hand animation; those belong to the app shell
//! and render presentation layer.

use crate::block::Block;
use crate::camera::Camera;
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

/// Seconds a dropped item survives before despawning if never collected.
const ITEM_DESPAWN_SECS: f32 = 300.0;

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
    dropped: Vec<DroppedItem>,
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
}

impl Game {
    pub fn new(mut cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        // Open (or create) the on-disk world. A returning world supplies its own
        // seed, player and entities; a fresh one uses `new_seed` and a found spawn.
        let (save, level, saved_entities) = if world_name.is_empty() {
            // Empty name = in-memory only (used by tests; never touches disk).
            (None, None, Vec::new())
        } else {
            match crate::save::open(world_name) {
                Ok(o) => (Some(o.save), o.level, o.entities),
                Err(e) => {
                    log::warn!("save disabled: could not open world '{world_name}': {e}");
                    (None, None, Vec::new())
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
            dropped: saved_entities,
            dropped_light_revision: 0,
            particles: ParticleSystem::new(),
            spawn_counter: 0,
            mining_dust_t: 0.0,
            item_entity_instances: Vec::new(),
            particle_instances: Vec::new(),
            tick_accumulator: 0.0,
            autosave_t: 0.0,
        }
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + player + inventory) and `entities.dat` (dropped items).
    /// A no-op without an attached save.
    pub fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(self.world.seed, &self.player, 0));
            save.save_entities(crate::save::entities::encode(&self.dropped));
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
        if !input.gameplay_enabled {
            self.mining
                .update(dt, self.look.as_ref(), input.break_held, true, &self.world);
            self.mining_dust_t = 0.0;
            return (false, false);
        }

        let mut broke_block = false;
        if let Some(event) =
            self.mining
                .update(dt, self.look.as_ref(), input.break_held, false, &self.world)
        {
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

        (input.place_clicked && self.try_place(), broke_block)
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
        for &(item, chance) in block.drop_spec().drops {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let roll = crate::entity::hash01(self.spawn_counter as u64);
            if chance < 1.0 && roll >= chance {
                continue;
            }
            let stack = ItemStack::new(item, 1);
            let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = skylight;
            self.dropped.push(drop);
        }
    }

    fn tick_entities(&mut self, dt: f32) {
        let player_pos = self.player.body_center();
        let mut i = self.dropped.len();
        while i > 0 {
            i -= 1;
            // After loading a saved world, restored drops may sit over chunks
            // that haven't streamed back in yet. Freeze any such drop — no
            // physics, ageing, despawn, or pickup — until its column returns, so
            // it can't fall through not-yet-generated terrain. Mirrors the
            // player's `columns_loaded` gate. Gated on a save being attached: a
            // freshly generated world only ever spawns drops in loaded chunks, and
            // unit tests simulate drops in a deliberately empty (no-save) world.
            if self.world.save().is_some() {
                let dp = self.dropped[i].pos;
                if !self
                    .world
                    .chunk_loaded((dp.x.floor() as i32) >> 4, (dp.z.floor() as i32) >> 4)
                {
                    continue;
                }
            }
            let before_cell = voxel_at(self.dropped[i].pos);
            self.dropped[i].tick(dt, &self.world, Some(player_pos));
            let after_cell = voxel_at(self.dropped[i].pos);
            if before_cell != after_cell {
                self.dropped[i].skylight = sky6_at_block(&self.world, after_cell);
            }

            if self.dropped[i].age >= ITEM_DESPAWN_SECS {
                self.dropped.swap_remove(i);
                continue;
            }

            if self.dropped[i].within_pickup(player_pos) {
                let stack = self.dropped[i].stack;
                match self.player.inventory.add(stack) {
                    None => {
                        self.dropped.swap_remove(i);
                    }
                    Some(leftover) => {
                        self.dropped[i].stack = leftover;
                    }
                }
            }
        }

        self.particles.tick(dt, &self.world);
    }

    fn refresh_dropped_item_lights_after_world_light_update(&mut self) {
        let revision = self.world.lighting_revision();
        if self.dropped_light_revision == revision {
            return;
        }
        for drop in &mut self.dropped {
            drop.skylight = sky6_at_pos(&self.world, drop.pos);
        }
        self.dropped_light_revision = revision;
    }

    fn map_item_entities(&mut self) {
        self.item_entity_instances.clear();
        self.item_entity_instances
            .extend(self.dropped.iter().map(|d| ItemEntityInstance {
                pos: d.pos,
                item: d.stack.item,
                spin: d.spin,
                skylight: d.skylight,
            }));
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

    fn game() -> Game {
        Game::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
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
        assert!(game.dropped.is_empty());
        game.spawn_drops(IVec3::new(2, 3, 4), Block::Dirt, 17);
        assert_eq!(game.dropped.len(), 1);
        let d = &game.dropped[0];
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
        game.dropped
            .push(DroppedItem::new(centre, ItemStack::new(item, 1), 1));
        game.tick_entities(0.001);
        let after = count_item(&game.player.inventory, item);
        assert_eq!(after, before + 1);
        assert!(game.dropped.is_empty());
    }

    #[test]
    fn dropped_item_magnets_toward_player_then_absorbs() {
        let mut game = game();
        let item = crate::item::ItemType::Poppy;
        let before = count_item(&game.player.inventory, item);
        let chest = game.player.body_center();
        let start = chest + Vec3::new(0.0, crate::entity::ATTRACT_RADIUS - 0.1, 0.0);
        game.dropped
            .push(DroppedItem::new(start, ItemStack::new(item, 1), 1));
        let d0 = (game.dropped[0].pos - chest).length();
        game.tick_entities(1.0 / 60.0);
        if !game.dropped.is_empty() {
            let d1 = (game.dropped[0].pos - chest).length();
            assert!(d1 < d0);
        }
        for _ in 0..60 {
            if game.dropped.is_empty() {
                break;
            }
            game.tick_entities(1.0 / 60.0);
        }
        assert!(game.dropped.is_empty());
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
        game.dropped.push(drop);

        for _ in 0..60 {
            game.tick_entities(1.0 / 60.0);
        }

        assert_eq!(game.dropped.len(), 1);
        assert_eq!(count_item(&game.player.inventory, item), before);
    }

    #[test]
    fn distant_dropped_item_is_not_picked_up() {
        let mut game = game();
        let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
        game.dropped.push(DroppedItem::new(
            far,
            ItemStack::new(crate::item::ItemType::Dirt, 1),
            2,
        ));
        game.tick_entities(0.001);
        assert_eq!(game.dropped.len(), 1);
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
        game.dropped.push(drop);

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
        assert_eq!(game.dropped[0].skylight, 63);
    }

    #[test]
    fn stale_dropped_item_despawns() {
        let mut game = game();
        let far = game.player.eye() + Vec3::new(50.0, 0.0, 0.0);
        let mut item = DroppedItem::new(far, ItemStack::new(crate::item::ItemType::Dirt, 1), 3);
        item.age = ITEM_DESPAWN_SECS + 1.0;
        game.dropped.push(item);
        game.tick_entities(0.001);
        assert!(game.dropped.is_empty());
    }

    #[test]
    fn place_with_empty_hand_does_nothing() {
        let mut game = game();
        game.player.inventory = crate::inventory::Inventory::new();
        for _ in 0..64 {
            game.player.inventory.decrement_selected();
        }
        assert!(game.player.inventory.selected().is_none());
        game.look = Some(hit(IVec3::new(0, 40, 0), IVec3::Y));
        assert!(!game.try_place());
    }

    #[test]
    fn place_into_loaded_air_decrements_selected() {
        let mut game = game();
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
        game.dropped.clear();
        game.dropped.push(DroppedItem::new(
            Vec3::new(1.0, 2.0, 3.0),
            ItemStack::new(crate::item::ItemType::Dirt, 1),
            1,
        ));
        game.dropped.push(DroppedItem::new(
            Vec3::new(4.0, 5.0, 6.0),
            ItemStack::new(crate::item::ItemType::Stone, 1),
            2,
        ));
        game.map_item_entities();
        assert_eq!(game.item_entity_instances.len(), 2);
        assert_eq!(game.item_entity_instances[0].pos, game.dropped[0].pos);
        assert_eq!(
            game.item_entity_instances[0].item,
            crate::item::ItemType::Dirt
        );
        assert_eq!(game.item_entity_instances[0].spin, game.dropped[0].spin);
        assert_eq!(
            game.item_entity_instances[1].item,
            crate::item::ItemType::Stone
        );
    }

    #[test]
    fn map_item_entities_reuses_the_vec_without_growth() {
        let mut game = game();
        game.dropped.clear();
        for i in 0..8 {
            game.dropped.push(DroppedItem::new(
                Vec3::splat(i as f32),
                ItemStack::new(crate::item::ItemType::Dirt, 1),
                i,
            ));
        }
        game.map_item_entities();
        let cap = game.item_entity_instances.capacity();
        game.dropped.truncate(2);
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
}
