use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use petramond_render::camera::Camera;
use petramond_world::crafting::CraftingCatalog;
use crate::particle::ParticleSystem;
use petramond::player::Player;
use petramond::server::game::ServerGame;
use petramond::server::handle::ServerHandle;
use petramond::player::PlayerId;
use petramond::worker::JobPool;
use petramond::world::{World, WorldRole};
use petramond_worldgen::density::surface::SurfaceDensitySystem;

use petramond::server::session_build::build_server_with_pool;

use super::section_cache::section_cache_registry_key;
use super::Game;

/// Everything `Game` needs beyond the [`ServerHandle`]: the client replica +
/// the join-time seeds mirrored off the freshly-built server (the in-process
/// stand-in for the remote join handshake's `JoinData`/`SelfRestore`).
pub struct ClientBootstrap {
    replica: World,
    client_player: Player,
    self_view: crate::game::replicated::SelfView,
    self_id: PlayerId,
    replicated_tick: u64,
    fallback_world: SurfaceDensitySystem,
    client_mods: petramond::modding::client::ClientModRuntime,
    crafting: CraftingCatalog,
}

impl Game {
    pub fn new(cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let t0 = std::time::Instant::now();
        let (server, bootstrap) = build_session(world_name, new_seed, render_dist);
        // The sim moves to its own self-clocked thread;
        // from here the client owns only the message handle.
        let handle = ServerHandle::spawn(server);
        let mut game = Self::assemble(cam, handle, bootstrap);
        // Loopback skips the remap, so the local vocabulary IS the session's
        // — binding it keys this cache for a later harvest (a remote join to
        // a server with identical tables may legitimately claim it).
        game.section_cache.adopt_session(section_cache_registry_key(
            &petramond::net::remap::local_name_tables(),
        ));
        log::debug!(
            target: "petramond::join::perf",
            "Game::new: {:.1} ms",
            t0.elapsed().as_secs_f64() * 1e3
        );
        game
    }

    /// The REMOTE client session: no save, no
    /// `ServerGame` — `handle` fronts a TCP connection
    /// ([`ServerHandle::from_remote`]) and `join` came from
    /// [`petramond::net::handshake::client_handshake`]. The connect worker
    /// runs the handshake off-thread, spawns the connection (which installs
    /// the id remap), and hands both here; everything after this constructor
    /// is the ordinary replicated-client path.
    pub fn new_remote(
        cam: Camera,
        join: Box<petramond::net::protocol::JoinData>,
        handle: ServerHandle,
        render_dist: i32,
        server_identity: &str,
        server_mods: &BTreeSet<String>,
        retained_section_cache: Option<crate::game::section_cache::SectionCache>,
    ) -> Self {
        let join = *join;
        let registry_key = section_cache_registry_key(&join.tables);
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
            // Client mods are gated by the SERVER's mod set (the handshake's
            // ModList): a locally installed client mod the server does not
            // run — e.g. the minimap against a server without it — must not
            // activate for this session.
            client_mods: petramond::modding::client::ClientModRuntime::load(
                join.seed,
                &petramond::modding::client::remote_session_key(server_identity),
                server_mods,
            ),
            crafting: CraftingCatalog::from_data(join.crafting_recipes),
        };
        let mut game = Self::assemble(cam, handle, bootstrap);
        game.remote = true;
        game.player_roster = join.players.into_iter().collect();
        // A cache retained from an earlier session re-promotes only under
        // the same id vocabulary — its blocks are client-local ids whose
        // meaning this session's remap tables define. adopt_session clears
        // on drift (a fresh cache just binds the key); any Join claims
        // already made for cleared entries heal through the
        // SectionCacheMiss fallback.
        let mut cache = retained_section_cache.unwrap_or_default();
        cache.adopt_session(registry_key);
        game.section_cache = cache;
        game
    }

    /// Hand the section cache to the app shell at session teardown — the next
    /// remote join's manifest claims it.
    pub fn take_section_cache(&mut self) -> crate::game::section_cache::SectionCache {
        std::mem::take(&mut self.section_cache)
    }

    /// Assemble the client half around an already-connected server handle.
    pub fn assemble(
        mut cam: Camera,
        handle: ServerHandle,
        bootstrap: ClientBootstrap,
    ) -> Self {
        sync_camera_to_player(&mut cam, &bootstrap.client_player);
        let last_player_eye_y = bootstrap.client_player.eye().y;
        // The camera the caller built carries the authored FOV; the per-frame
        // speed widening multiplies on top of it (see `speed_fov`).
        let speed_fov = super::speed_fov::SpeedFov::new(cam.fov_y);
        Self {
            cam,
            player: bootstrap.client_player,
            look: None,
            use_look: None,
            self_mount: None,
            targeted_mob: None,
            targeted_player: None,
            held_rotation: Default::default(),
            outbox: Vec::new(),
            frame_messages: Vec::new(),
            camera_step_y_offset: 0.0,
            camera_sneak_y_offset: 0.0,
            last_player_eye_y,
            third_person: Default::default(),
            handle,
            remote: false,
            connection_lost: None,
            connection_lost_reported: false,
            last_sent_transform: None,
            replica_clock: Default::default(),
            staged_rows: Default::default(),
            stream_batch_started: None,
            stream_rate_ema: None,
            incoming: Vec::new(),
            remote_section_installs: Vec::new(),
            pending_chat_lines: Vec::new(),
            replica: bootstrap.replica,
            client_mods: bootstrap.client_mods,
            replicated_mobs: Default::default(),
            replicated_items: Default::default(),
            self_view: bootstrap.self_view,
            menu_view: Default::default(),
            crafting: bootstrap.crafting,
            pending_events: Default::default(),
            self_id: bootstrap.self_id,
            player_roster: HashMap::new(),
            remote_players: Default::default(),
            replicated_tick: bootstrap.replicated_tick,
            open_chests: Default::default(),
            prediction: super::prediction::PredictionLedger::new(),
            local_mining: petramond_world::mining::MiningState::new(),
            predicted_input: Default::default(),
            view_bob: Default::default(),
            speed_fov,
            local_hand_jab: false,
            local_hand_swing: false,
            local_hand_threw: false,
            local_broke_block: None,
            local_placed_block: None,
            place_ghost: None,
            predicted_presentation_cells: Default::default(),
            section_cache: Default::default(),
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
/// synchronously; the `server/handle.rs` tests spawn it bare.
pub fn build_session(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
) -> (ServerGame, ClientBootstrap) {
    build_session_with_pool(
        world_name,
        new_seed,
        render_dist,
        Arc::new(JobPool::new(JobPool::default_threads())),
    )
}

/// [`build_session`] with an INLINE job pool (jobs run at submit, on the
/// caller) — the in-process test harness's deterministic variant: streaming
/// work completes inside the pump that queued it, so tests never sleep-wait
/// on background workers. (The join-profile test deliberately keeps the real
/// threaded pool — it MEASURES that pipeline.)
#[cfg(test)]
pub fn build_session_inline(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
) -> (ServerGame, ClientBootstrap) {
    build_session_with_pool(
        world_name,
        new_seed,
        render_dist,
        Arc::new(JobPool::inline()),
    )
}

/// [`build_session`] over a caller-owned job pool. The in-process test
/// harness passes an INLINE pool ([`JobPool::inline`]) so queued gen/light
/// work completes inside the pump that queued it — tests gate on one more
/// pump, never on wall-clock sleeps racing background workers.
pub fn build_session_with_pool(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    pool: Arc<JobPool>,
) -> (ServerGame, ClientBootstrap) {
    // The LOCAL player's identity (client.json / env / OS username) keys
    // its per-world save file: `players/<name>.dat`.
    let player_name = petramond::save::client::resolve_player_name(&petramond::save::client::load());
    let (server, pool, fallback_world) =
        build_server_with_pool(world_name, new_seed, render_dist, Some(player_name), pool);
    let t_client = std::time::Instant::now();

    // The CLIENT's replica world: fed by the server's terrain payloads and
    // deltas, it lights + meshes for the renderer and answers the client's
    // collision/raycast/placement reads. It never generates — the seed only
    // feeds the mesh tint fallback for missing edge columns.
    let replica = World::new_with_pool(
        server.world.seed,
        render_dist,
        WorldRole::ClientReplica,
        pool,
    );

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
    let crafting = server.recipes.crafting().clone();
    // Client mods activate for the session's ENABLED packs (installed minus
    // this world's disabled set) — the same authority the server host uses.
    let enabled_mods: BTreeSet<String> =
        petramond::modding::modset::active(server.world.disabled_mods())
            .into_iter()
            .map(|entry| entry.id)
            .collect();
    let client_mods = petramond::modding::client::ClientModRuntime::load(
        server.world.seed,
        &petramond::modding::client::local_session_key(world_name),
        &enabled_mods,
    );
    log::debug!(
        target: "petramond::join::perf",
        "client bootstrap (replica + client mods): {:.1} ms",
        t_client.elapsed().as_secs_f64() * 1e3
    );

    (
        server,
        ClientBootstrap {
            replica,
            client_player,
            self_view,
            self_id,
            replicated_tick,
            fallback_world,
            client_mods,
            crafting,
        },
    )
}

/// Rebuild the local predicted player from the join handshake's restore —
/// the wire twin of `save::player::PlayerData::restore` (wire ids arrived
/// remapped to local ids at the transport; effects travel by name).
fn player_from_restore(r: &petramond::net::protocol::SelfRestore) -> Player {
    let mut player = Player::new(r.transform.pos);
    player.set_mode(match r.mode {
        1 => petramond::player::PlayerMode::Spectator,
        _ => petramond::player::PlayerMode::Survival,
    });
    // `set_mode` clears velocity; restore motion after it.
    player.vel = r.transform.vel;
    player.yaw = r.transform.yaw;
    player.pitch = r.transform.pitch;
    player.set_health(r.health);
    player.bed_spawn = r
        .bed_spawn
        .map(|(bed, spot)| petramond::player::BedSpawn { bed, spot });
    player.inventory = crate::game::replicated::inventory_from_wire(&r.inventory, r.active_slot);
    player.craft_craftable_only = r.craft_craftable_only;
    // The client mirrors only the UNLOCKED set — what its browser may show.
    // The obtained set is server-side progression bookkeeping and never
    // crosses the wire.
    player
        .progression
        .restore(std::iter::empty(), r.unlocked_recipes.clone());
    for (name, remaining) in &r.effects {
        match petramond_world::effect::by_name(name) {
            Some(effect) => player.apply_effect(effect, *remaining),
            None => log::warn!("join restore: dropping unknown status effect '{name}'"),
        }
    }
    player
}

fn sync_camera_to_player(cam: &mut Camera, player: &Player) {
    cam.pos = player.eye();
    cam.yaw = player.yaw;
    cam.pitch = player.pitch;
}

