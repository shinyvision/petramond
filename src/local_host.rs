//! Launching the in-process listen server for a local session.
//!
//! The one door the client uses to start a singleplayer world: it gets back
//! the same two things a remote join yields — a [`ServerHandle`] and the
//! session's [`JoinData`] — plus the few process-local resources an
//! in-process host can share. The client never sees the server's memory; the
//! server is built, seeded, and moved onto its own thread here.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::net::handle::ServerHandle;
use crate::net::identity::PlayerKey;
use crate::net::protocol::JoinData;
use crate::server::game::ServerGame;
use crate::server::session_build::{build_server_with_pool, LocalPlayer};
use crate::worker::JobPool;
use petramond_worldgen::density::surface::SurfaceDensitySystem;

/// Everything a local session's client needs beyond the handle.
pub struct LocalSession {
    /// The local player's join payload, exactly what a remote client
    /// receives on `JoinAccept` (in local ids — the loopback skips the remap).
    pub join: Box<JoinData>,
    /// The server's job pool. One process shares one pool: the client
    /// replica lights and meshes on it instead of spawning a second
    /// machine-sized thread set.
    pub jobs: Arc<JobPool>,
    /// The world seed's surface density, the mesh tint fallback for missing
    /// edge columns.
    pub fallback_world: SurfaceDensitySystem,
    /// The mod ids enabled for this world (installed minus its disabled set) —
    /// the same authority the server's mod host uses, so client mods activate
    /// for exactly these.
    pub enabled_mods: BTreeSet<String>,
}

/// Open (or create) `world_name`, build the full server session with the
/// identity `key` (display name `name`) as its permanent local player, and
/// move it onto its own self-clocked thread.
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

/// [`launch`] without the thread: the built server stays with the caller,
/// for test harnesses that pump it synchronously over a loopback pipe
/// (usually with an inline `pool`, so streaming work completes inside the
/// pump that queued it).
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
