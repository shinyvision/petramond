use std::collections::BTreeSet;
use std::sync::Arc;

use crate::net::identity::PlayerKey;
use crate::player::Player;
use crate::player::PlayerId;
use crate::save::{LevelData, WorldSave};
use crate::server::accounts::PlayerRegistry;
use crate::server::game::{ServerGame, ServerParts};
use crate::server::player::ConnectedPlayer;
use crate::server::progression::RecipeCatalog;
use crate::worker::JobPool;
use crate::world::ServerWorld;
use petramond_math::math::Vec3;
use petramond_world::crafting::load_recipes_for;
use petramond_worldgen::SurfaceDensitySystem;

struct OpenedSession {
    save: Option<(WorldSave, crate::world::SavedIndex)>,
    level: Option<LevelData>,
    disabled_mods: BTreeSet<String>,
    keep_inventory: bool,
    day_minutes: u32,
}

impl Default for OpenedSession {
    fn default() -> Self {
        let defaults = crate::save::settings::WorldSettings::default();
        OpenedSession {
            save: None,
            level: None,
            disabled_mods: defaults.disabled_mods,
            keep_inventory: defaults.keep_inventory,
            day_minutes: defaults.day_minutes,
        }
    }
}

pub fn build_headless_session(world_name: &str, new_seed: u32, render_dist: i32) -> ServerGame {
    build_server(world_name, new_seed, render_dist, None).0
}

pub struct LocalPlayer {
    pub key: PlayerKey,
    pub name: String,
}

#[cfg(test)]
pub fn build_server_inline(world_name: &str, new_seed: u32, render_dist: i32) -> ServerGame {
    let mut server = build_server_with_pool(
        world_name,
        new_seed,
        render_dist,
        Some(LocalPlayer {
            key: crate::net::identity::PlayerIdentity::generate()
                .expect("os randomness")
                .key(),
            name: crate::save::client::resolve_player_name(&crate::save::client::load()),
        }),
        Arc::new(JobPool::inline()),
    )
    .0;
    server.account_policy = crate::account::AccountPolicy::Offline;
    server
}

/// The ONE server constructor both shapes share. `local_player` decides
/// the shape: `Some` restores/spawns that player as the permanent session 0
/// (listen server); `None` starts with no sessions at all (headless) — mod
/// init then runs against a DISCARDED stand-in player (the single-player-
/// shaped ABI needs a body; anything an init hook grants it is dropped, and
/// the pause gate starts permanently open since remote players may join from
/// boot). Returns the server plus the shared job pool and gen fallback the
/// listen path's client bootstrap needs.
pub fn build_server(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    local_player: Option<LocalPlayer>,
) -> (ServerGame, Arc<JobPool>, SurfaceDensitySystem) {
    build_server_with_pool(
        world_name,
        new_seed,
        render_dist,
        local_player,
        Arc::new(JobPool::new(JobPool::default_threads())),
    )
}

pub fn build_server_with_pool(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    local_player: Option<LocalPlayer>,
    pool: Arc<JobPool>,
) -> (ServerGame, Arc<JobPool>, SurfaceDensitySystem) {
    let mut perf = JoinPerf::start();
    let opened = open_session(world_name);
    perf.mark("save_open");
    let seed = opened.level.as_ref().map(|l| l.seed).unwrap_or(new_seed);
    let fallback_world = SurfaceDensitySystem::new(seed);
    let mut accounts = PlayerRegistry::load(opened.save.as_ref().map(|(s, _)| s));
    let local = local_player.map(|LocalPlayer { key, name }| {
        let requested = crate::net::identity::coerce_player_name(&name);
        let save = opened.save.as_ref().map(|(s, _)| s);
        let claim = accounts.claim(save, key, &requested, |_| false);
        let player = claim.restored.unwrap_or_else(|| spawn_player(seed));
        let session = ConnectedPlayer::new(PlayerId(0), key, claim.name, player, render_dist);
        (session, claim.first_seen)
    });
    perf.mark("player_restore_or_spawn");
    let disabled_mods = opened.disabled_mods;

    if let Some((local, _)) = &local {
        let feet = local.player.pos;
        let (pcx, pcz) = (
            (feet.x.floor() as i32).div_euclid(16),
            (feet.z.floor() as i32).div_euclid(16),
        );
        let tiles = (-2..=2)
            .flat_map(|dz| (-2..=2).map(move |dx| (pcx + dx, pcz + dz)))
            .collect::<Vec<_>>();
        crate::worker::warm_surface_tiles(&pool, seed, tiles);
    }
    let mut world = ServerWorld::with_pool(seed, render_dist, pool.clone());
    perf.mark("pool_and_world");
    let save = opened.save.map(|(mut save, saved)| {
        save.use_job_pool(pool.clone());
        (save, saved)
    });
    attach_save(&mut world, save);
    world.set_disabled_mods(disabled_mods.clone());
    world.set_keep_inventory(opened.keep_inventory);
    world
        .mobs_mut()
        .set_sim_distance(crate::mob::SimDistance::default());
    world.set_day_cycle_ticks(crate::rules::daynight::cycle_ticks_for_day_minutes(
        opened.day_minutes,
    ));
    if let Some(level) = &opened.level {
        world.set_world_kv(level.world_kv.clone());
        world.restore_tick(level.tick);
        world.set_populated_columns(level.populated_columns.clone());
    }
    let mut operators = crate::server::permissions::load(&world);
    let local = local.map(|(session, first_seen)| {
        if first_seen && operators.claim_legacy(&session.name, session.key) {
            crate::server::permissions::store(&mut world, &operators);
        }
        session
    });
    perf.mark("save_attach");

    let recipes = load_recipes_for(&disabled_mods)
        .unwrap_or_else(|error| panic!("failed to load crafting recipes: {error}"));
    crate::modding::install_recipes(std::sync::Arc::new(recipes.clone()));
    let catalog = RecipeCatalog::new(recipes);
    perf.mark("recipes");
    let mods = crate::modding::ModHost::load(seed, &disabled_mods);
    perf.mark("mod_wasm_load");
    let mut server = ServerGame::assemble(ServerParts {
        world,
        local,
        operators,
        accounts,
        catalog,
        mods,
        jobs: pool.clone(),
        account_policy: crate::account::AccountPolicy::from_env(),
    });
    server.install_core_systems();
    server.catch_up_sessions();
    server.world.set_replication_capture(true);
    server.mods.initialize(&mut server.world);
    perf.mark("mod_init");
    if let Some(sess) = server.sessions.first() {
        let eye = sess.player.eye();
        server.world.update_load(
            (eye.x.floor() as i32).div_euclid(16),
            (eye.y.floor() as i32).div_euclid(16),
            (eye.z.floor() as i32).div_euclid(16),
        );
    }
    perf.mark("stream_kick");
    perf.finish("build_server");

    (server, pool, fallback_world)
}

struct JoinPerf {
    t0: std::time::Instant,
    last: std::time::Instant,
    phases: Vec<(&'static str, f64)>,
}

impl JoinPerf {
    fn start() -> Self {
        let now = std::time::Instant::now();
        Self {
            t0: now,
            last: now,
            phases: Vec::new(),
        }
    }

    fn mark(&mut self, phase: &'static str) {
        let now = std::time::Instant::now();
        self.phases
            .push((phase, (now - self.last).as_secs_f64() * 1e3));
        self.last = now;
    }

    fn finish(mut self, what: &str) {
        if !log::log_enabled!(target: "petramond::join::perf", log::Level::Debug) {
            return;
        }
        self.mark("rest");
        let total = self.t0.elapsed().as_secs_f64() * 1e3;
        let breakdown: Vec<String> = self
            .phases
            .iter()
            .map(|(phase, ms)| format!("{phase} {ms:.1}"))
            .collect();
        log::debug!(
            target: "petramond::join::perf",
            "{what}: {total:.1} ms ({})",
            breakdown.join(", ")
        );
    }
}

fn open_session(world_name: &str) -> OpenedSession {
    if world_name.is_empty() {
        return OpenedSession::default();
    }

    match crate::save::open(world_name) {
        Ok(opened) => OpenedSession {
            save: Some((opened.save, opened.saved)),
            level: opened.level,
            disabled_mods: opened.disabled_mods,
            keep_inventory: opened.keep_inventory,
            day_minutes: opened.day_minutes,
        },
        Err(e) => {
            log::warn!("save disabled: could not open world '{world_name}': {e}");
            OpenedSession::default()
        }
    }
}

pub fn spawn_player(seed: u32) -> Player {
    let surface = petramond_worldgen::spawn::find_spawn(seed);
    let feet = petramond_math::world_pos::WorldPos::block_min(surface) + Vec3::new(0.5, 1.0, 0.5);
    Player::new(feet)
}

pub fn attach_save(world: &mut ServerWorld, save: Option<(WorldSave, crate::world::SavedIndex)>) {
    if let Some((save, saved)) = save {
        world.attach_save(save, saved);
    }
}
