//! The internal game server (multiplayer Phase A+).
//!
//! Owns the authoritative simulation ([`game::ServerGame`]: world, sessions,
//! recipes/loot, mod host, and the 20 TPS fixed-tick stage ladder) plus the
//! per-connected-player state ([`player::ConnectedPlayer`]). Since multiplayer
//! Phase D the `ServerGame` runs on its OWN self-clocked thread
//! ([`handle::ServerHandle`]); the client (`crate::game::Game`) talks to it
//! exclusively over message channels. Remote (TCP) connections ride the same
//! loop through [`remote::RemoteHub`] (multiplayer Phase E, "Open to LAN").

pub(crate) mod bed;
pub(crate) mod breaking;
pub(crate) mod chat;
pub(crate) mod daynight;
pub(crate) mod drops;
pub(crate) mod entities;
pub(crate) mod game;
pub(crate) mod handle;
pub(crate) mod health;
pub(crate) mod item_use;
pub(crate) mod menu;
pub(crate) mod mod_actions;
pub(crate) mod movement;
pub(crate) mod placement;
pub(crate) mod player;
pub(crate) mod remote;
pub(crate) mod streaming;
