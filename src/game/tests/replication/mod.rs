//! Contract tests for the entity + self replication batches:
//! the pump emits `TickUpdate`s, the client's replicated stores
//! feed presentation with prev/curr interpolation pairs, absent ids drop, the
//! inventory rides a `SelfState` only when its revision moved, and the HUD
//! read models mirror session truth through the batch — never by direct read.

mod entity_store;
mod menu_sync;
mod state_rows;
mod world_events;

use super::super::tick::TICK_DT;
use super::common;

/// One pump that must have executed at least one fixed tick, returning its
/// replication batch (the trailing `Tick` message of the pump's output).
fn pump_one_tick(game: &mut super::common::TestGame) -> Box<crate::net::protocol::TickUpdate> {
    let mut inbox = Vec::new();
    let out = game.server.pump(TICK_DT, &mut inbox);
    out.msgs
        .into_iter()
        .find_map(|msg| match msg {
            crate::net::protocol::ServerToClient::Tick(update) => Some(update),
            _ => None,
        })
        .expect("a full tick's dt executes a tick and emits a batch")
}
