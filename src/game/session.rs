use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::camera::Camera;
use crate::crafting::load_recipes_for;
use crate::entity::ParticleSystem;
use crate::mathh::Vec3;
use crate::player::Player;
use crate::save::{LevelData, WorldSave};
use crate::server::game::ServerGame;
use crate::server::handle::ServerHandle;
use crate::server::player::{ConnectedPlayer, PlayerId};
use crate::worker::JobPool;
use crate::world::{World, WorldRole};
use crate::worldgen::density::surface::SurfaceDensitySystem;

use super::Game;

struct OpenedSession {
    save: Option<WorldSave>,
    level: Option<LevelData>,
    /// Per-world disabled mod ids (`settings.json`; empty without a save).
    disabled_mods: BTreeSet<String>,
    /// The world's "Optimize explored terrain" setting (false without a save —
    /// there is nowhere to persist to).
    optimize_explored_terrain: bool,
}

/// Everything `Game` needs beyond the [`ServerHandle`]: the client replica +
/// the join-time seeds mirrored off the freshly-built server (the in-process
/// stand-in for the Phase E join handshake's `JoinData`/`SelfRestore`).
pub(crate) struct ClientBootstrap {
    replica: World,
    client_player: Player,
    self_view: crate::game::replicated::SelfView,
    self_id: PlayerId,
    replicated_tick: u64,
    fallback_world: SurfaceDensitySystem,
}

impl Game {
    pub fn new(cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let (server, bootstrap) = build_session(world_name, new_seed, render_dist);
        // The sim moves to its own self-clocked thread (multiplayer Phase D);
        // from here the client owns only the message handle.
        let handle = ServerHandle::spawn(server);
        Self::assemble(cam, handle, bootstrap)
    }

    /// The REMOTE client session (multiplayer Phase E): no save, no
    /// `ServerGame` — `handle` fronts a TCP connection
    /// ([`ServerHandle::from_remote`]) and `join` came from
    /// [`crate::net::handshake::client_handshake`]. The E2 connect worker
    /// runs the handshake off-thread, spawns the connection (which installs
    /// the id remap), and hands both here; everything after this constructor
    /// is the ordinary replicated-client path.
    pub fn new_remote(
        cam: Camera,
        join: Box<crate::net::protocol::JoinData>,
        handle: ServerHandle,
        render_dist: i32,
    ) -> Self {
        let join = *join;
        // The replica gets its OWN pool: unlike the in-process split there is
        // no server world in this process to share one with.
        let pool = Arc::new(JobPool::new(JobPool::default_threads()));
        let replica = World::new_with_pool(join.seed, render_dist, WorldRole::ClientReplica, pool);
        let client_player = player_from_restore(&join.self_restore);
        let bootstrap = ClientBootstrap {
            replica,
            self_view: crate::game::replicated::SelfView::seed_from(&client_player),
            client_player,
            self_id: join.player_id,
            replicated_tick: 0,
            fallback_world: SurfaceDensitySystem::new(join.seed),
        };
        let mut game = Self::assemble(cam, handle, bootstrap);
        game.remote = true;
        game.player_roster = join.players.into_iter().collect();
        game
    }

    /// Assemble the client half around an already-connected server handle.
    pub(crate) fn assemble(
        mut cam: Camera,
        handle: ServerHandle,
        bootstrap: ClientBootstrap,
    ) -> Self {
        sync_camera_to_player(&mut cam, &bootstrap.client_player);
        let last_player_eye_y = bootstrap.client_player.eye().y;
        Self {
            cam,
            player: bootstrap.client_player,
            look: None,
            targeted_mob: None,
            targeted_player: None,
            held_rotation: Default::default(),
            outbox: Vec::new(),
            frame_messages: Vec::new(),
            camera_step_y_offset: 0.0,
            last_player_eye_y,
            third_person: Default::default(),
            handle,
            remote: false,
            connection_lost: None,
            connection_lost_reported: false,
            last_sent_transform: None,
            replica_clock: Default::default(),
            stream_batch_started: None,
            stream_rate_ema: None,
            incoming: Vec::new(),
            replica: bootstrap.replica,
            replicated_mobs: Default::default(),
            replicated_items: Default::default(),
            self_view: bootstrap.self_view,
            menu_view: Default::default(),
            pending_events: Default::default(),
            self_id: bootstrap.self_id,
            player_roster: HashMap::new(),
            remote_players: Default::default(),
            replicated_tick: bootstrap.replicated_tick,
            open_chests: Default::default(),
            fallback_world: bootstrap.fallback_world,
            particles: ParticleSystem::new(),
            mining_dust_t: 0.0,
            chest_lids: HashMap::new(),
            door_swings: HashMap::new(),
        }
    }
}

/// Open (or create) the world and build the full server session — save
/// attachment, player restore/fresh spawn, recipes/loot/mods, mod init — plus
/// the client bootstrap mirrored off it. `Game::new` hands the server to
/// [`ServerHandle::spawn`]; the test harness keeps it in-process and pumps it
/// synchronously; the Phase D handle tests spawn it bare.
pub(crate) fn build_session(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
) -> (ServerGame, ClientBootstrap) {
    let opened = open_session(world_name);
    let seed = opened.level.as_ref().map(|l| l.seed).unwrap_or(new_seed);
    let fallback_world = SurfaceDensitySystem::new(seed);
    // The LOCAL player's identity (client.json / env / OS username) keys
    // its per-world save file: `players/<name>.dat`.
    let player_name = crate::save::client::resolve_player_name(&crate::save::client::load());
    let player = player_for_session(opened.save.as_ref(), &player_name, seed);
    let disabled_mods = opened.disabled_mods;

    // ONE background pool shared by the server world (gen/light) and the
    // client replica (light/mesh) — two machine-sized thread sets in one
    // process would oversubscribe every core.
    let pool = Arc::new(JobPool::new(JobPool::default_threads()));
    // The SERVER world: sim + gen + light, no meshing (the replica draws).
    let mut world =
        World::new_with_pool(seed, render_dist, WorldRole::ServerHeadless, pool.clone());
    attach_save(&mut world, opened.save);
    // Per-world mod enablement: the palette already applied it in
    // `save::open_at`; the world carries it for the natural spawner and
    // the mods.json record, and the mod host / recipes below take it.
    // Editing settings for a world that is NOT open only takes effect on
    // the next open — nothing re-reads settings.json mid-session.
    world.set_disabled_mods(disabled_mods.clone());
    world.set_optimize_explored_terrain(opened.optimize_explored_terrain);
    // The mod world KV and the world tick ride level.dat: restore both
    // before core systems and mod init below, so core day/night, scheduled
    // ticks, and init-time HostCalls (CurrentTick) see the persisted state.
    if let Some(level) = &opened.level {
        world.set_mod_kv(level.world_kv.clone());
        world.restore_tick(level.tick);
    }

    let mut server = ServerGame {
        world,
        sessions: vec![ConnectedPlayer::new(PlayerId(0), player_name, player)],
        recipes: {
            // The mod host answers `SmeltResult` from the same loaded
            // catalog the engine cooks from — install a shared snapshot
            // (the process-wide pattern gen hooks use).
            let recipes = load_recipes_for(&disabled_mods);
            crate::modding::install_recipes(std::sync::Arc::new(recipes.clone()));
            recipes
        },
        loot: crate::mob::load_loot(),
        bus: crate::events::EventBus::default(),
        systems: crate::events::TickSystems::default(),
        mods: crate::modding::ModHost::load(seed, &disabled_mods),
        spawn_counter: 0,
        next_mod_sound_handle: 1,
        tick_accumulator: 0.0,
        paused: false,
        lan_ever_opened: false,
        pending_wire_events: Vec::new(),
        autosave_t: 0.0,
        chest_viewers: HashMap::new(),
        last_shipped_env: None,
    };
    crate::server::daynight::install_core(&mut server.world, &mut server.systems);
    // Replication is live from construction: block/water changes log into
    // the capture at the announce choke point and drain into each pump's
    // `TickUpdate`.
    server.world.set_replication_capture(true);
    // Mod init runs AFTER any engine registrations so mods sort behind the
    // engine at equal priority (the bus ordering contract), and after the
    // full session state exists so init-time host calls see a real world.
    // The mod ABI is single-player-shaped: init (and global tick stages)
    // see the HOST session's player (session 0) — see WIKI/modding.md.
    {
        let ServerGame {
            world,
            sessions,
            bus,
            systems,
            mods,
            next_mod_sound_handle,
            ..
        } = &mut server;
        let host_session = &mut sessions[0];
        mods.initialize(
            world,
            &mut host_session.player,
            &mut host_session.gui_state,
            bus,
            systems,
            next_mod_sound_handle,
        );
    }

    // The CLIENT's replica world: fed by the server's terrain payloads and
    // deltas, it lights + meshes for the renderer and answers the client's
    // collision/raycast/placement reads. It never generates — the seed only
    // feeds the mesh tint fallback for missing edge columns.
    let replica = World::new_with_pool(seed, render_dist, WorldRole::ClientReplica, pool);

    // The client's locally-simulated player starts as an exact clone of
    // the session player (AFTER mod init, which may have granted items) —
    // they stay transform-identical through the verbatim PlayerUpdate
    // round-trip; only its inventory CONTENTS go stale (session-owned).
    let client_player = server.sessions[0].player.clone();
    let replicated_tick = server.world.current_tick();
    // The replicated self view seeds from the same restored player — the
    // in-process stand-in for the join handshake's `SelfRestore` — so the
    // HUD is right before the first tick's batch arrives.
    let self_view = crate::game::replicated::SelfView::seed_from(&server.sessions[0].player);
    let self_id = server.sessions[0].id;

    (
        server,
        ClientBootstrap {
            replica,
            client_player,
            self_view,
            self_id,
            replicated_tick,
            fallback_world,
        },
    )
}

fn open_session(world_name: &str) -> OpenedSession {
    if world_name.is_empty() {
        return OpenedSession {
            save: None,
            level: None,
            disabled_mods: BTreeSet::new(),
            optimize_explored_terrain: false,
        };
    }

    match crate::save::open(world_name) {
        Ok(opened) => OpenedSession {
            save: Some(opened.save),
            level: opened.level,
            disabled_mods: opened.disabled_mods,
            optimize_explored_terrain: opened.optimize_explored_terrain,
        },
        Err(e) => {
            log::warn!("save disabled: could not open world '{world_name}': {e}");
            OpenedSession {
                save: None,
                level: None,
                disabled_mods: BTreeSet::new(),
                optimize_explored_terrain: false,
            }
        }
    }
}

/// Rebuild the local predicted player from the join handshake's restore —
/// the wire twin of `save::player::PlayerData::restore` (wire ids arrived
/// remapped to local ids at the transport; effects travel by name).
fn player_from_restore(r: &crate::net::protocol::SelfRestore) -> Player {
    let mut player = Player::new(r.pos);
    player.set_mode(match r.mode {
        1 => crate::player::PlayerMode::Spectator,
        _ => crate::player::PlayerMode::Survival,
    });
    // `set_mode` clears velocity; restore motion after it.
    player.vel = r.vel;
    player.yaw = r.yaw;
    player.pitch = r.pitch;
    player.set_health(r.health);
    player.bed_spawn = r
        .bed_spawn
        .map(|(bed, spot)| crate::player::BedSpawn { bed, spot });
    player.inventory = crate::game::replicated::inventory_from_wire(&r.inventory, r.active_slot);
    for (name, remaining) in &r.effects {
        match crate::effect::by_name(name) {
            Some(effect) => player.apply_effect(effect, *remaining),
            None => log::warn!("join restore: dropping unknown status effect '{name}'"),
        }
    }
    player
}

/// Restore this player from `players/<name>.dat` when present, else spawn
/// fresh at the seed's surface pick (a brand-new world OR a new player joining
/// an existing one).
fn player_for_session(save: Option<&WorldSave>, name: &str, seed: u32) -> Player {
    save.and_then(|s| s.load_player(name))
        .and_then(|bytes| crate::save::player::decode(&bytes))
        .map(|data| data.restore())
        .unwrap_or_else(|| spawn_player(seed))
}

/// A fresh player at the seed's surface pick — the fallback for both the
/// local session and a remote join with no `players/<name>.dat` yet.
pub(crate) fn spawn_player(seed: u32) -> Player {
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
