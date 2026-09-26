//! In-process session builders for the test harnesses: the server is built
//! through `petramond::local_host` but stays with the caller (a loopback
//! harness pumps it synchronously), and the client half boots from the
//! session's `JoinData` exactly as `Game::new` does.

use std::sync::Arc;

use petramond::server::game::ServerGame;
use petramond::worker::JobPool;

use crate::game::session::{local_player, ClientBootstrap};

/// The in-process session over the REAL threaded job pool, for tests that
/// measure the streaming pipeline.
pub(crate) fn build_session(
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
/// caller) — the deterministic variant: streaming work completes inside the
/// pump that queued it, so tests never sleep-wait on background workers.
pub(crate) fn build_session_inline(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
) -> (ServerGame, ClientBootstrap) {
    build_session_with_pool(world_name, new_seed, render_dist, Arc::new(JobPool::inline()))
}

fn build_session_with_pool(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    pool: Arc<JobPool>,
) -> (ServerGame, ClientBootstrap) {
    let (key, name) = local_player();
    let (server, session) = petramond::local_host::build_in_process(
        world_name,
        new_seed,
        render_dist,
        key,
        name,
        pool,
    );
    (
        server,
        ClientBootstrap::local(world_name, render_dist, session),
    )
}
