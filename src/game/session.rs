use std::collections::{BTreeSet, HashMap};

use crate::camera::Camera;
use crate::crafting::load_recipes_for;
use crate::entity::ParticleSystem;
use crate::mathh::Vec3;
use crate::mining::MiningState;
use crate::mob::load_loot;
use crate::player::Player;
use crate::save::{LevelData, WorldSave};
use crate::world::World;
use crate::worldgen::density::surface::SurfaceDensitySystem;

use super::container::ContainerMenu;
use super::drops::DropQueue;
use super::Game;

struct OpenedSession {
    save: Option<WorldSave>,
    level: Option<LevelData>,
    /// Per-world disabled mod ids (`settings.json`; empty without a save).
    disabled_mods: BTreeSet<String>,
}

impl Game {
    pub fn new(mut cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let opened = open_session(world_name);
        let seed = opened.level.as_ref().map(|l| l.seed).unwrap_or(new_seed);
        let fallback_world = SurfaceDensitySystem::new(seed);
        let player = player_for_session(opened.level.as_ref(), seed);
        let disabled_mods = opened.disabled_mods;

        sync_camera_to_player(&mut cam, &player);

        let mut world = World::new(seed, render_dist);
        attach_save(&mut world, opened.save);
        // Per-world mod enablement: the palette already applied it in
        // `save::open_at`; the world carries it for the natural spawner and
        // the mods.json record, and the mod host / recipes below take it.
        // Editing settings for a world that is NOT open only takes effect on
        // the next open — nothing re-reads settings.json mid-session.
        world.set_disabled_mods(disabled_mods.clone());
        // The mod world KV rides level.dat: restore it before core systems and
        // mod init below, so core day/night and init-time HostCalls see it.
        if let Some(level) = &opened.level {
            world.set_mod_kv(level.world_kv.clone());
        }

        let mut game = Self {
            cam,
            camera_step_y_offset: 0.0,
            last_player_eye_y: player.eye().y,
            world,
            fallback_world,
            player,
            look: None,
            targeted_mob: None,
            mining: MiningState::new(),
            dropped_light_revision: 0,
            particles: ParticleSystem::new(),
            spawn_counter: 0,
            next_mod_sound_handle: 1,
            mining_dust_t: 0.0,
            attack_cooldown: 0,
            intent_break_held: false,
            intent_sneak: false,
            intent_gameplay: false,
            pending_attack: false,
            pending_place: false,
            held_rotation_item: None,
            held_block_rotated: false,
            drop_queue: DropQueue::default(),
            pending_menu_clicks: Vec::new(),
            chest_lids: HashMap::new(),
            door_swings: HashMap::new(),
            tick_accumulator: 0.0,
            autosave_t: 0.0,
            recipes: load_recipes_for(&disabled_mods),
            loot: load_loot(),
            menu: ContainerMenu::new(),
            request_open_table: false,
            request_open_furnace: None,
            request_open_chest: None,
            request_open_workbench: None,
            request_open_mod_gui: None,
            request_close_mod_gui: false,
            request_open_sleep: false,
            sleep: None,
            wake_requested: false,
            respawn_requested: false,
            toggled_door: None,
            bus: crate::events::EventBus::default(),
            systems: crate::events::TickSystems::default(),
            mods: crate::modding::ModHost::load(seed, &disabled_mods),
        };
        super::daynight::install_core(&mut game.world, &mut game.systems);
        // Mod init runs AFTER any engine registrations so mods sort behind the
        // engine at equal priority (the bus ordering contract), and after the
        // full session state exists so init-time host calls see a real world.
        {
            let Self {
                world,
                player,
                bus,
                systems,
                mods,
                next_mod_sound_handle,
                ..
            } = &mut game;
            mods.initialize(world, player, bus, systems, next_mod_sound_handle);
        }
        game
    }

    /// Persist everything: flush modified chunks to the save thread, then write
    /// `level.dat` (seed + player + inventory + mod world KV) and the save's
    /// mod-set record (`mods.json`). A no-op without an attached save.
    pub fn save_all(&mut self) {
        self.world.flush_modified_chunks();
        if let Some(save) = self.world.save() {
            save.save_level(crate::save::level::encode(
                self.world.seed,
                &self.player,
                0,
                self.world.mod_kv(),
            ));
            save.save_mods_json(crate::modding::modset::encode_active(
                self.world.disabled_mods(),
            ));
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
            disabled_mods: BTreeSet::new(),
        };
    }

    match crate::save::open(world_name) {
        Ok(opened) => OpenedSession {
            save: Some(opened.save),
            level: opened.level,
            disabled_mods: opened.disabled_mods,
        },
        Err(e) => {
            log::warn!("save disabled: could not open world '{world_name}': {e}");
            OpenedSession {
                save: None,
                level: None,
                disabled_mods: BTreeSet::new(),
            }
        }
    }
}

fn player_for_session(level: Option<&LevelData>, seed: u32) -> Player {
    match level {
        Some(level) => restore_player(level),
        None => spawn_player(seed),
    }
}

fn restore_player(level: &LevelData) -> Player {
    let mut player = Player::new(level.player_pos);
    player.set_mode(level.player_mode);
    // `set_mode` clears velocity, so restore saved motion after mode.
    player.vel = level.player_vel;
    player.yaw = level.player_yaw;
    player.pitch = level.player_pitch;
    player.set_health(level.player_health);
    player.inventory = level.inventory.clone();
    player.bed_spawn = level.bed_spawn;
    player
}

fn spawn_player(seed: u32) -> Player {
    let surface = crate::worldgen::spawn::find_spawn(seed);
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
