use std::sync::Arc;

use petramond::server::game::ServerGame;
use petramond::worker::JobPool;

use crate::game::session::{local_player, ClientBootstrap};

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

pub(crate) fn build_session_inline(
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

fn build_session_with_pool(
    world_name: &str,
    new_seed: u32,
    render_dist: i32,
    pool: Arc<JobPool>,
) -> (ServerGame, ClientBootstrap) {
    let (key, name) = local_player();
    let (mut server, session) =
        petramond::local_host::build_in_process(world_name, new_seed, render_dist, key, name, pool);
    server.set_account_policy(petramond::account::AccountPolicy::Offline);
    (
        server,
        ClientBootstrap::local(world_name, render_dist, session),
    )
}
