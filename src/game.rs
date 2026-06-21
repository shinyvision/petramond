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
use crate::mathh::{IVec3, SelectionShape, Vec3};
use crate::mining::MiningState;
use crate::player::{self, Input, Player, PlayerMode, RaycastHit};
use crate::render::{BreakOverlayView, ItemEntityInstance, ParticleInstance};
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

/// Deep, murky blue the world fades to (fog + clear colour) when the camera eye
/// is underwater.
const UNDERWATER_FOG_COLOR: [f32; 3] = [0.04, 0.16, 0.30];

/// Seconds a dropped item survives before despawning if never collected.
const ITEM_DESPAWN_SECS: f32 = 300.0;

/// Mining-dust emission interval, seconds.
const MINING_DUST_INTERVAL: f32 = 0.1;

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
}

impl Game {
    pub fn new(mut cam: Camera, seed: u32, render_dist: i32) -> Self {
        // Spawn on the nearest exposed solid surface to the origin (drops to the
        // nearest coast if the origin is open ocean). The camera's incoming
        // position is a placeholder; override it so chunk streaming centres on the
        // real spawn from frame one. Stand the feet on top of the surface block.
        let fallback_world = CascadeWorld::new(seed);
        let surface = crate::worldgen::spawn::find_spawn(&fallback_world, seed);
        let feet = Vec3::new(
            surface.x as f32 + 0.5,
            (surface.y + 1) as f32,
            surface.z as f32 + 0.5,
        );
        cam.pos = Vec3::new(feet.x, feet.y + player::EYE, feet.z);
        Self {
            cam,
            world: World::new(seed, render_dist),
            fallback_world,
            player: Player::new(feet),
            look: None,
            mining: MiningState::new(),
            dropped: Vec::new(),
            dropped_light_revision: 0,
            particles: ParticleSystem::new(),
            spawn_counter: 0,
            mining_dust_t: 0.0,
            item_entity_instances: Vec::new(),
            particle_instances: Vec::new(),
        }
    }

    pub fn tick(&mut self, dt: f32, input: &GameInput) -> GameEvents {
        self.apply_camera_input(input);
        self.apply_hotbar_input(input);
        self.tick_player(dt, input);
        self.tick_world();

        self.look = Player::raycast(self.cam.pos, self.cam.forward(), &self.world);
        let (placed_block, broke_block) = self.handle_block_actions(dt, input);

        self.tick_entities(dt);
        self.tick_mesh_budget();
        self.refresh_dropped_item_lights_after_world_light_update();

        GameEvents {
            placed_block,
            broke_block,
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
        let underwater = Block::from_id(self.world.chunk_block(
            eye.x.floor() as i32,
            eye.y.floor() as i32,
            eye.z.floor() as i32,
        )) == Block::Water;

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
        #[cfg(not(target_arch = "wasm32"))]
        const MESH_BUDGET: usize = 32;
        #[cfg(target_arch = "wasm32")]
        const MESH_BUDGET: usize = 6;
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
        Game::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), 1, 1)
    }

    fn hit(pos: IVec3, normal: IVec3) -> RaycastHit {
        RaycastHit {
            block: pos,
            normal,
            outline: SelectionShape::full_block(pos),
        }
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
            #[cfg(not(target_arch = "wasm32"))]
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
