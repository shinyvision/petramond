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
) -> (ServerHandle, LocalSession) {
    let (server, session) = build(
        world_name,
        new_seed,
        render_dist,
        key,
        name,
        Arc::new(JobPool::new(JobPool::default_threads())),
    );
    (crate::server::handle::spawn(server), session)
}

#[cfg(any(test, feature = "test-support"))]
pub fn build_in_process(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    key: PlayerKey,
    name: String,
    pool: Arc<JobPool>,
) -> (ServerGame, LocalSession) {
    build(world_name, new_seed, render_dist, key, name, pool)
}

fn build(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    key: PlayerKey,
    name: String,
    pool: Arc<JobPool>,
) -> (ServerGame, LocalSession) {
    let (server, jobs, fallback_world) = build_server_with_pool(
        world_name,
        new_seed,
        render_dist,
        Some(LocalPlayer { key, name }),
        pool,
    );
    let join = server
        .local_join_data()
        .expect("a server built with a local player has a local session");
    let enabled_mods = crate::modding::modset::active(server.world().data().disabled_mods())
        .into_iter()
        .map(|entry| entry.id)
        .collect();
    (
        server,
        LocalSession {
            join,
            jobs,
            fallback_world,
            enabled_mods,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_join_mirrors_the_built_session() {
        let key = crate::net::identity::PlayerIdentity::generate()
            .expect("os randomness")
            .key();
        let (server, session) = build_in_process(
            "",
            3,
            1,
            key,
            "Local".to_string(),
            Arc::new(JobPool::inline()),
        );
        let local = &server.sessions()[0];
        assert_eq!(session.join.player_id, local.id());
        assert_eq!(session.join.seed, server.world().data().seed);
        assert_eq!(session.join.self_restore.transform.pos, local.player().pos);
        assert_eq!(session.join.self_restore.health, local.player().health());
        assert!(session.join.players.is_empty(), "nobody else is connected");
        assert_eq!(
            session.join.tables,
            crate::net::remap::local_name_tables(),
            "the loopback join speaks the local id vocabulary"
        );
    }
}
