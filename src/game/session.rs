use std::collections::HashMap;

use crate::camera::Camera;
use crate::crafting::load_recipes;
use crate::entity::ParticleSystem;
use crate::mathh::Vec3;
use crate::mining::MiningState;
use crate::mob::load_loot;
use crate::player::Player;
use crate::save::{LevelData, WorldSave};
use crate::world::World;
use crate::worldgen::classic::world::CascadeWorld;

use super::drops::DropQueue;
use super::{ContainerMenu, Game};

struct OpenedSession {
    save: Option<WorldSave>,
    level: Option<LevelData>,
}

impl Game {
    pub fn new(mut cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let opened = open_session(world_name);
        let seed = opened.level.as_ref().map(|l| l.seed).unwrap_or(new_seed);
        let fallback_world = CascadeWorld::new(seed);
        let player = player_for_session(opened.level.as_ref(), &fallback_world, seed);

        sync_camera_to_player(&mut cam, &player);

        let mut world = World::new(seed, render_dist);
        attach_save(&mut world, opened.save);

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
            drop_queue: DropQueue::default(),
            pending_menu_clicks: Vec::new(),
            chest_lids: HashMap::new(),
            door_swings: HashMap::new(),
            tick_accumulator: 0.0,
            autosave_t: 0.0,
            recipes: load_recipes(),
            loot: load_loot(),
            menu: ContainerMenu::new(),
            request_open_table: false,
            request_open_furnace: None,
            request_open_chest: None,
            request_open_workbench: None,
            toggled_door: false,
        }
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + player + inventory). A no-op without an attached save.
    pub fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(self.world.seed, &self.player, 0));
        }
    }

    pub(super) fn maybe_autosave(&mut self, dt: f32) {
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
}

fn open_session(world_name: &str) -> OpenedSession {
    if world_name.is_empty() {
        return OpenedSession {
            save: None,
            level: None,
        };
    }

    match crate::save::open(world_name) {
        Ok(opened) => OpenedSession {
            save: Some(opened.save),
            level: opened.level,
        },
        Err(e) => {
            log::warn!("save disabled: could not open world '{world_name}': {e}");
            OpenedSession {
                save: None,
                level: None,
            }
        }
    }
}

fn player_for_session(
    level: Option<&LevelData>,
    fallback_world: &CascadeWorld,
    seed: u32,
) -> Player {
    match level {
        Some(level) => restore_player(level),
        None => spawn_player(fallback_world, seed),
    }
}

fn restore_player(level: &LevelData) -> Player {
    let mut player = Player::new(level.player_pos);
    player.set_mode(level.player_mode);
    // `set_mode` clears velocity, so restore saved motion after mode.
    player.vel = level.player_vel;
    player.yaw = level.player_yaw;
    player.pitch = level.player_pitch;
    player.inventory = level.inventory.clone();
    player
}

fn spawn_player(fallback_world: &CascadeWorld, seed: u32) -> Player {
    let surface = crate::worldgen::spawn::find_spawn(fallback_world, seed);
    let feet = Vec3::new(
        surface.x as f32 + 0.5,
        (surface.y + 1) as f32,
        surface.z as f32 + 0.5,
    );
    Player::new(feet)
}

fn sync_camera_to_player(cam: &mut Camera, player: &Player) {
    cam.pos = player.eye();
    cam.yaw = player.yaw;
    cam.pitch = player.pitch;
}

fn attach_save(world: &mut World, save: Option<WorldSave>) {
    if let Some(save) = save {
        world.attach_save(save);
    }
}
