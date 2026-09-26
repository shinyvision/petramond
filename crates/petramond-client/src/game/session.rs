use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use crate::particle::ParticleSystem;
use petramond::local_host::LocalSession;
use petramond::net::handle::ServerHandle;
use petramond::net::identity::{PlayerIdentity, PlayerKey};
use petramond::net::protocol::JoinData;
use petramond::player::Player;
use petramond::player::PlayerId;
use petramond::worker::JobPool;
use petramond::world::ReplicaWorld;
use petramond_render::camera::Camera;
use petramond_world::crafting::CraftingCatalog;
use petramond_worldgen::SurfaceDensitySystem;

use super::section_cache::section_cache_registry_key;
use super::Game;

/// Everything `Game` needs beyond the [`ServerHandle`]: the client replica +
/// the join-time seeds, built from the session's [`JoinData`] — the same
/// payload whether the server is in-process ([`petramond::local_host`]) or
/// remote (the TCP handshake's `JoinAccept`).
pub struct ClientBootstrap {
    replica: ReplicaWorld,
    jobs: Arc<JobPool>,
    client_player: Player,
    self_view: crate::game::replicated::SelfView,
    self_id: PlayerId,
    players: Vec<(PlayerId, String)>,
    fallback_world: SurfaceDensitySystem,
    client_mods: petramond::modding::client::ClientModRuntime,
    crafting: CraftingCatalog,
    /// Whether the server is REMOTE (joined over TCP) rather than in-process.
    remote: bool,
}

impl ClientBootstrap {
    /// The bootstrap for a join: the replica world (fed by the server's
    /// terrain payloads and deltas, it lights + meshes for the renderer and
    /// answers the client's collision/raycast/placement reads — it never
    /// generates), the locally-simulated player restored from the join's
    /// `SelfRestore`, and the replicated self view seeded from the same
    /// restore so the HUD is right before the first tick's batch arrives.
    fn from_join(
        join: JoinData,
        jobs: Arc<JobPool>,
        render_dist: i32,
        fallback_world: SurfaceDensitySystem,
        client_mods: petramond::modding::client::ClientModRuntime,
        remote: bool,
    ) -> Self {
        let replica = ReplicaWorld::with_pool(join.seed, render_dist, jobs.clone());
        let client_player = player_from_restore(&join.self_restore);
        ClientBootstrap {
            replica,
            jobs,
            self_view: crate::game::replicated::SelfView::seed_from(&client_player),
            client_player,
            self_id: join.player_id,
            players: join.players,
            fallback_world,
            client_mods,
            crafting: CraftingCatalog::from_data(join.crafting_recipes),
            remote,
        }
    }

    /// The bootstrap for an in-process session: the replica shares the
    /// host's job pool, and client mods activate for the world's ENABLED
    /// packs — the same authority the server host uses.
    pub(super) fn local(world_name: &str, render_dist: i32, session: LocalSession) -> Self {
        let t_client = std::time::Instant::now();
        let join = *session.join;
        let client_mods = petramond::modding::client::ClientModRuntime::load(
            join.seed,
            &petramond::modding::client::local_session_key(world_name),
            &session.enabled_mods,
        );
        let bootstrap = Self::from_join(
            join,
            session.jobs,
            render_dist,
            session.fallback_world,
            client_mods,
            false,
        );
        log::debug!(
            target: "petramond::join::perf",
            "client bootstrap (replica + client mods): {:.1} ms",
            t_client.elapsed().as_secs_f64() * 1e3
        );
        bootstrap
    }
}

impl Game {
    /// The session's background pool, for presentation work that must stay
    /// off the frame thread.
    pub fn jobs(&self) -> &Arc<JobPool> {
        &self.jobs
    }

    pub fn new(cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let t0 = std::time::Instant::now();
        let (key, name) = local_player();
        // The host builds the server and moves the sim onto its own
        // self-clocked thread; from here the client owns only the message
        // handle and the join payload, exactly like a remote join.
        let (handle, session) =
            petramond::local_host::launch(world_name, new_seed, render_dist, key, name);
        let bootstrap = ClientBootstrap::local(world_name, render_dist, session);
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
        join: Box<JoinData>,
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
        // Client mods are gated by the SERVER's mod set (the handshake's
        // ModList): a locally installed client mod the server does not run —
        // e.g. the minimap against a server without it — must not activate
        // for this session.
        let client_mods = petramond::modding::client::ClientModRuntime::load(
            join.seed,
            &petramond::modding::client::remote_session_key(server_identity),
            server_mods,
        );
        let fallback_world = SurfaceDensitySystem::new(join.seed);
        let bootstrap =
            ClientBootstrap::from_join(join, pool, render_dist, fallback_world, client_mods, true);
        let mut game = Self::assemble(cam, handle, bootstrap);
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
    pub fn assemble(mut cam: Camera, handle: ServerHandle, bootstrap: ClientBootstrap) -> Self {
        sync_camera_to_player(&mut cam, &bootstrap.client_player);
        // The camera the caller built carries the authored FOV; the per-frame
        // speed widening multiplies on top of it (see `CameraRig`).
        let camera_rig =
            super::camera_rig::CameraRig::new(cam.fov_y, bootstrap.client_player.eye().y);
        Self {
            cam,
            player: bootstrap.client_player,
            look: None,
            use_look: None,
            targeted_mob: None,
            targeted_player: None,
            held_rotation: Default::default(),
            camera_rig,
            third_person: Default::default(),
            net: super::net_link::NetLink::new(handle, bootstrap.remote),
            last_sent_transform: None,
            remote_section_installs: Vec::new(),
            pending_chat_lines: Vec::new(),
            replica: bootstrap.replica,
            client_mods: bootstrap.client_mods,
            self_view: bootstrap.self_view,
            menu_view: Default::default(),
            crafting: bootstrap.crafting,
            pending_events: Default::default(),
            entities: super::replicated::EntityReplica::new(bootstrap.self_id, bootstrap.players),
            prediction: super::prediction::PredictionLedger::new(),
            jobs: bootstrap.jobs,
            notice: String::new(),
            tools: Default::default(),
            flight_toggle: Default::default(),
            break_repeat: Default::default(),
            local_mining: petramond_world::mining::MiningState::new(),
            predicted_input: Default::default(),
            local_bones: Default::default(),
            local_bone_target: Vec::new(),
            intent_use_held: false,
            hand: Default::default(),
            section_cache: Default::default(),
            fallback_world: bootstrap.fallback_world,
            particles: ParticleSystem::new(),
            mining_feedback: Default::default(),
            mob_digging: HashMap::new(),
            block_animations: Default::default(),
        }
    }
}

/// This machine's player identity (`<data>/identity.key`, created on first
/// use). Under test a throwaway one, so the suite never creates or reads the
/// developer's real identity file.
pub(crate) fn player_identity() -> std::io::Result<PlayerIdentity> {
    if cfg!(test) {
        PlayerIdentity::generate()
    } else {
        PlayerIdentity::load_or_create_default()
    }
}

/// The LOCAL player: this machine's identity keys its per-world save file
/// (`players/<key>.dat`); the display name comes from client.json / env /
/// OS username.
pub(super) fn local_player() -> (PlayerKey, String) {
    let name = petramond::save::client::resolve_player_name(&petramond::save::client::load());
    let key = match player_identity() {
        Ok(identity) => identity.key(),
        Err(e) => {
            log::error!(
                "could not load the player identity ({e}); this world saves the player \
                 under an offline stand-in identity until it loads again"
            );
            petramond::net::identity::offline_key(&name)
        }
    };
    (key, name)
}

/// Rebuild the local predicted player from the join handshake's restore —
/// the wire twin of `save::player::PlayerData::restore` (wire ids arrived
/// remapped to local ids at the transport; effects travel by name).
fn player_from_restore(r: &petramond::net::protocol::SelfRestore) -> Player {
    let mut player = Player::new(r.transform.pos);
    player.set_mode(petramond::player::PlayerMode::from_u8(r.mode));
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
