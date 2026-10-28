use std::collections::BTreeSet;
use std::sync::Arc;

use crate::net::handle::ServerHandle;
use crate::net::identity::PlayerKey;
use crate::net::protocol::JoinData;
use crate::server::game::ServerGame;
use crate::server::session_build::{build_server_with_pool, LocalPlayer};
use crate::worker::JobPool;
use petramond_worldgen::SurfaceDensitySystem;

pub struct LocalSession {
    pub content: petramond_world::content::Content,
    pub join: Box<JoinData>,
    pub jobs: Arc<JobPool>,
    pub fallback_world: SurfaceDensitySystem,
    pub enabled_mods: BTreeSet<String>,
}

pub fn launch(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    key: PlayerKey,
    name: String,
) -> Result<(ServerHandle, LocalSession), String> {
    let (server, session) = build(
        world_name,
        new_seed,
        render_dist,
        key,
        name,
        Arc::new(JobPool::new(JobPool::default_threads())),
    )?;
    Ok((crate::server::handle::spawn(server), session))
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_in_process(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    key: PlayerKey,
    name: String,
    pool: Arc<JobPool>,
) -> Result<(ServerGame, LocalSession), String> {
    build(world_name, new_seed, render_dist, key, name, pool)
}

fn build(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    key: PlayerKey,
    name: String,
    pool: Arc<JobPool>,
) -> Result<(ServerGame, LocalSession), String> {
    let (server, jobs, fallback_world) = build_server_with_pool(
        world_name,
        new_seed,
        render_dist,
        Some(LocalPlayer { key, name }),
        pool,
    )?;
    let content = server.content();
    let _content = petramond_world::content::pin(content);
    let join = server
        .local_join_data()
        .expect("a server built with a local player has a local session");
    let enabled_mods = crate::modding::modset::active(server.world().data().disabled_mods())
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    Ok((
        server,
        LocalSession {
            content,
            join,
            jobs,
            fallback_world,
            enabled_mods,
        },
    ))
}
