use std::collections::BTreeSet;
use std::sync::Arc;

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
    remote: bool,
    identity: super::capture::SessionIdentity,
}

impl ClientBootstrap {
    fn from_join(
        join: JoinData,
        jobs: Arc<JobPool>,
        render_dist: i32,
        fallback_world: SurfaceDensitySystem,
        client_mods: petramond::modding::client::ClientModRuntime,
        enabled: &BTreeSet<String>,
        remote: bool,
    ) -> Self {
        let replica = ReplicaWorld::with_pool(join.seed, render_dist, jobs.clone());
        let client_player = player_from_restore(&join.self_restore);
        let identity = super::capture::SessionIdentity {
            player_name: join.player_name.clone(),
            mods: petramond::modding::modset::active(&BTreeSet::new())
                .into_iter()
                .filter(|entry| enabled.contains(&entry.id))
                .collect(),
        };
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
            identity,
        }
    }

    pub(super) fn presented(
        replica: ReplicaWorld,
        jobs: Arc<JobPool>,
        viewer: Player,
        self_id: PlayerId,
        fallback_world: SurfaceDensitySystem,
        client_mods: petramond::modding::client::ClientModRuntime,
        crafting: CraftingCatalog,
    ) -> Self {
        Self {
            replica,
            jobs,
            self_view: crate::game::replicated::SelfView::seed_from(&viewer),
            client_player: viewer,
            self_id,
            players: Vec::new(),
            fallback_world,
            client_mods,
            crafting,
            remote: false,
            identity: Default::default(),
        }
    }

    pub(super) fn local(world_name: &str, render_dist: i32, session: LocalSession) -> Self {
        let t_client = std::time::Instant::now();
        let join = *session.join;
        let client_mods = petramond::modding::client::ClientModRuntime::load(
            join.seed,
            &petramond::modding::client::local_session_key(world_name),
            &session.enabled_mods,
            mod_api::ClientContext::Local {
                name: world_name.to_owned(),
                shared: false,
            },
        );
        let bootstrap = Self::from_join(
            join,
            session.jobs,
            render_dist,
            session.fallback_world,
            client_mods,
            &session.enabled_mods,
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
    pub fn jobs(&self) -> &Arc<JobPool> {
        &self.jobs
    }

    pub fn new(cam: Camera, world_name: &str, new_seed: u32, render_dist: i32) -> Self {
        let t0 = std::time::Instant::now();
        let (key, name) = local_player();
        let (handle, session) =
            petramond::local_host::launch(world_name, new_seed, render_dist, key, name);
        let bootstrap = ClientBootstrap::local(world_name, render_dist, session);
        let mut game = Self::assemble(cam, handle, bootstrap);
        game.replica
            .section_cache
            .adopt_session(section_cache_registry_key(
                &petramond::net::remap::local_name_tables(),
            ));
        log::debug!(
            target: "petramond::join::perf",
            "Game::new: {:.1} ms",
            t0.elapsed().as_secs_f64() * 1e3
        );
        game
    }

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
        let pool = Arc::new(JobPool::new(JobPool::default_threads()));
        let policy = join.client_policy;
        let mut enabled = server_mods.clone();
        if policy.presentation_packs {
            enabled.extend(petramond::modding::client::presentation_only_packs());
        }
        let client_mods = petramond::modding::client::ClientModRuntime::load(
            join.seed,
            &petramond::modding::client::remote_session_key(server_identity),
            &enabled,
            mod_api::ClientContext::Remote {
                name: server_identity.to_owned(),
                presentation_packs: policy.presentation_packs,
            },
        );
        let fallback_world = SurfaceDensitySystem::new(join.seed);
        let bootstrap = ClientBootstrap::from_join(
            join,
            pool,
            render_dist,
            fallback_world,
            client_mods,
            &enabled,
            true,
        );
        let mut game = Self::assemble(cam, handle, bootstrap);
        // A retained cache only re-promotes under the same id vocabulary, since its block ids
        // are client-local and this session's remap tables define them. `adopt_session` clears
        // on drift; a fresh cache just binds the key. Join claims already made for cleared
        // entries heal through the `SectionCacheMiss` fallback.
        let mut cache = retained_section_cache.unwrap_or_default();
        cache.adopt_session(registry_key);
        game.replica.section_cache = cache;
        game
    }

    pub fn take_section_cache(&mut self) -> crate::game::section_cache::SectionCache {
        std::mem::take(&mut self.replica.section_cache)
    }

    pub fn assemble(cam: Camera, handle: ServerHandle, bootstrap: ClientBootstrap) -> Self {
        let entities = super::replicated::EntityReplica::new(bootstrap.self_id, bootstrap.players);
        Self {
            jobs: bootstrap.jobs,
            notice: String::new(),
            tools: Default::default(),
            net: super::net_link::NetLink::new(handle, bootstrap.remote),
            replica: super::replica_state::ReplicaState::new(
                bootstrap.replica,
                entities,
                bootstrap.self_view,
                bootstrap.crafting,
                bootstrap.fallback_world,
            ),
            local: super::local_player::LocalPlayer::new(cam, bootstrap.client_player),
            client_mods: bootstrap.client_mods,
            prediction: super::prediction::PredictionLedger::new(),
            hand: Default::default(),
            fx: Default::default(),
            presented_entities_cache: Vec::new(),
            last_anchor_feet: None,
            anchor_missing: false,
            presenting: Default::default(),
            world_capture: super::capture::WorldCapture::new(&bootstrap.identity),
        }
    }
}

pub(crate) fn player_identity() -> std::io::Result<PlayerIdentity> {
    if cfg!(test) {
        PlayerIdentity::generate()
    } else {
        PlayerIdentity::load_or_create_default()
    }
}

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

fn player_from_restore(r: &petramond::net::protocol::SelfRestore) -> Player {
    let mut player = Player::new(r.transform.pos);
    player.set_mode(petramond::player::PlayerMode::from_u8(r.mode));
    player.vel = r.transform.vel;
    player.yaw = r.transform.yaw;
    player.pitch = r.transform.pitch;
    player.set_health(r.health);
    player.bed_spawn = r
        .bed_spawn
        .map(|(bed, spot)| petramond::player::BedSpawn { bed, spot });
    player.inventory = crate::game::replicated::inventory_from_wire(&r.inventory, r.active_slot);
    player.craft_craftable_only = r.craft_craftable_only;
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
