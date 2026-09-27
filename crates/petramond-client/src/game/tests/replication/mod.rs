mod entity_store;
mod menu_sync;
mod state_rows;
mod world_events;

use super::super::tick::TICK_DT;
use super::common;

fn pump_one_tick(game: &mut super::common::TestGame) -> Box<petramond::net::protocol::TickUpdate> {
    let mut inbox = Vec::new();
    let out = game.sim_mut().pump(TICK_DT, &mut inbox);
    out.msgs
        .into_iter()
        .find_map(|msg| match msg {
            petramond::net::protocol::ServerToClient::Tick(update) => Some(update),
            _ => None,
        })
        .expect("a full tick's dt executes a tick and emits a batch")
}
