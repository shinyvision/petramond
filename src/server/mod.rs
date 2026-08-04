//! The internal game server.
//!
//! Owns the authoritative simulation ([`game::ServerGame`]: world, sessions,
//! recipes/loot, mod host, and the 20 TPS fixed-tick stage ladder) plus the
//! per-connected-player state ([`player::ConnectedPlayer`]). The
//! `ServerGame` runs on its OWN self-clocked thread
//! ([`handle::ServerHandle`]); the client (`crate::game::Game`) talks to it
//! exclusively over message channels. Remote (TCP) connections ride the same
//! loop through [`remote::RemoteHub`] ("Open to LAN").

pub mod actions;
pub mod bed;
pub mod breaking;
pub mod chat;
pub mod commands;
pub mod daynight;
pub mod drops;
pub mod entities;
pub mod game;
pub mod handle;
pub mod health;
pub mod interact;
pub mod item_use;
pub mod menu;
pub mod mob_target;
pub mod mod_actions;
pub mod movement;
pub mod permissions;
pub mod placement;
pub mod player;
pub mod session_build;
pub mod progression;
pub mod remote;
pub mod riding;
pub mod streaming;
