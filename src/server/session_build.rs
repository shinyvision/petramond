//! Server-session construction: open (or create) the world, attach the save,
//! restore or spawn the local player, load recipes/loot/mods, run mod init,
//! and kick the first streaming wave. Shared by the listen server (`crate::local_host`
//! wraps it), the headless dedicated server, and
//! the in-process test harness.

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
use crate::world::{World, WorldRole};
use petramond_math::math::Vec3;
use petramond_world::crafting::load_recipes_for;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

struct OpenedSession {
    save: Option<(WorldSave, crate::world::SavedIndex)>,
    level: Option<LevelData>,
    /// Per-world disabled mod ids (`settings.json`; empty without a save).
    disabled_mods: BTreeSet<String>,
    /// Keep the inventory on death (`settings.json`; default without a save).
    keep_inventory: bool,
    /// Day length in real minutes (`settings.json`; default without a save).
    day_minutes: u32,
}

impl Default for OpenedSession {
    /// No save (or an unopenable one): world rules take the settings defaults.
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

/// A HEADLESS server: the same world/save/mods construction as the listen
/// server, with NO local session and no client half — every player joins
/// over TCP, the streamer windows every session, and the sim freezes while
/// nobody is connected (`ServerGame::pump_tagged`'s empty-session gate).
/// Driven by the same `server::handle::spawn` loop; the standalone binary
/// (`platform::server`) parks its main thread on it.
pub fn build_headless_session(world_name: &str, new_seed: u32, render_dist: i32) -> ServerGame {
    build_server(world_name, new_seed, render_dist, None).0
}

/// The listen server's own player: the host's identity (the client's
/// `net::identity::PlayerIdentity` key — it keys the host's save file like
/// any remote player's) and its configured display name (coerced to a valid
/// one here; see `net::identity::coerce_player_name`).
pub struct LocalPlayer {
    pub key: PlayerKey,
    pub name: String,
}

/// [`build_server`] with an INLINE job pool and a local session — the core
/// crate's in-process test harness: streaming work completes inside the pump
/// that queued it, so tests never sleep-wait on background workers. The
/// client-facing twin (`game::session::build_session_inline`) additionally
/// builds the client bootstrap; server-side tests need only the sim.
#[cfg(test)]
pub fn build_server_inline(world_name: &str, new_seed: u32, render_dist: i32) -> ServerGame {
    build_server_with_pool(
        world_name,
        new_seed,
        render_dist,
        Some(LocalPlayer {
            // A throwaway identity: tests never touch the developer's real
            // identity file.
            key: crate::net::identity::PlayerIdentity::generate()
                .expect("os randomness")
                .key(),
            name: crate::save::client::resolve_player_name(&crate::save::client::load()),
        }),
        Arc::new(JobPool::inline()),
    )
    .0
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

/// [`build_server`] over a caller-owned job pool. The in-process test harness
/// passes an INLINE pool ([`JobPool::inline`]) so queued gen/light work
/// completes inside the pump that queued it — deterministic, with no
/// wall-clock waiting on background workers.
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
        // The local session starts at the full server budget (the host's own
        // view distance built this world); a live slider change follows
        // through `SetViewDistance` like any connection.
        let session = ConnectedPlayer::new(PlayerId(0), key, claim.name, player, render_dist);
        (session, claim.first_seen)
    });
    perf.mark("player_restore_or_spawn");
    let disabled_mods = opened.disabled_mods;

    // ONE pool shared by the server world (gen/light) and the client replica
    // (light/mesh) — two machine-sized thread sets in one process would
    // oversubscribe every core. Caller-owned (see `build_server_with_pool`).
    // Warm the spawn area's surface tiles across the pool while the rest of
    // construction runs: the stream kick below then finds them hot, instead
    // of the first column job deriving the whole neighbourhood serially.
    // (Tiles are pure `(seed, tile)` functions — mod hooks don't affect them,
    // so warming before mod init is safe.)
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
    // The SERVER world: sim + gen + light, no meshing (a replica draws).
    let mut world =
        World::new_with_pool(seed, render_dist, WorldRole::ServerHeadless, pool.clone());
    perf.mark("pool_and_world");
    // Section records deflate on the same shared pool.
    let save = opened.save.map(|(mut save, saved)| {
        save.use_job_pool(pool.clone());
        (save, saved)
    });
    attach_save(&mut world, save);
    // Per-world mod enablement: the palette already applied it in
    // `save::open_at`; the world carries it for the natural spawner and
    // the mods.json record, and the mod host / recipes below take it.
    // Editing settings for a world that is NOT open only takes effect on
    // the next open — nothing re-reads settings.json mid-session.
    world.set_disabled_mods(disabled_mods.clone());
    world.set_keep_inventory(opened.keep_inventory);
    // Mobs far from every player simulate at a reduced rate or not at all;
    // the headless host overrides this from its settings.
    world
        .mobs_mut()
        .set_sim_distance(crate::mob::SimDistance::default());
    // BEFORE core systems install below — the day/night cycle captures it.
    world.set_day_cycle_ticks(crate::rules::daynight::cycle_ticks_for_day_minutes(
        opened.day_minutes,
    ));
    // The mod world KV and the world tick ride level.dat: restore both
    // before core systems and mod init below, so core day/night, scheduled
    // ticks, and init-time HostCalls (CurrentTick) see the persisted state.
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

    // The mod host answers `SmeltResult` from the same loaded catalog the
    // engine cooks from — install a shared snapshot (the process-wide pattern
    // gen hooks use). The unlock index is the other view of that catalog.
    let recipes = load_recipes_for(&disabled_mods);
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
    });
    server.install_core_systems();
    // Reconcile the restored record against THIS world's catalog before the
    // first tick (a pack installed since the player last played). The local
    // client half clones this player, so it starts already caught up.
    server.catch_up_sessions();
    // Replication is live from construction: block/water changes log into
    // the capture at the announce choke point and drain into each pump's
    // `TickUpdate`.
    server.world.set_replication_capture(true);
    // Mod init runs AFTER the engine registrations so mods sort behind the
    // engine at equal priority (the bus ordering contract), and after the
    // world state exists so init-time host calls see a real world. Init
    // belongs to no player — listen and headless servers alike run it
    // actor-less.
    server.mods.initialize(&mut server.world);
    perf.mark("mod_init");
    // Kick the first streaming wave NOW — after mod init, so the session's
    // worldgen hooks are installed before any gen job runs. The spawn area's
    // column jobs generate on the pool while the rest of session construction
    // (replica world, client mods, server-thread spawn) finishes, instead of
    // idling until the server thread's first pump.
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

/// Wall-clock phase marks for the session build, logged under the
/// `petramond::join::perf` debug target — the first stop for any
/// click-to-spawn latency diagnosis.
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

/// A fresh player at the seed's surface pick — the fallback for both the
/// local session and a remote join with no saved player yet.
pub fn spawn_player(seed: u32) -> Player {
    let surface = petramond_worldgen::spawn::find_spawn(seed);
    let feet = petramond_math::world_pos::WorldPos::block_min(surface) + Vec3::new(0.5, 1.0, 0.5);
    Player::new(feet)
}

pub fn attach_save(world: &mut World, save: Option<(WorldSave, crate::world::SavedIndex)>) {
    if let Some((save, saved)) = save {
        world.attach_save(save, saved);
    }
}
