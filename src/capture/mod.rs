//! Capturing a presented world into a mod's files, and presenting one back.
//!
//! The engine writes the documented capture format (`mod_api::capture`, the
//! one implementation of its layout) and never decides what a capture is
//! for: which keys, how often, into which files are the mod's.
//!
//! - [`body`]: what every piece kind's body holds — the one codec.
//! - [`pieces`]: one piece as a record lays it out.
//! - [`moment`]: the parts of the presented world that are not terrain.
//! - [`state`]: `ClientWorldStateWrite` — a selection taken on the frame as
//!   `Arc` handles, encoded off it.
//! - [`events`]: events logs — one Frame record per frame that had anything.
//! - [`writer`]: the ordered per-file queues the records go out through.
//! - [`desk`]: what the client mods' calls and the client session share.
//! - [`present`]: presenting a world from mod-file byte ranges — [`source`]
//!   (the files and their incarnations), [`unpack`] (reading pieces and
//!   frames back), [`fold`] (preparing an apply), [`feed`] (the moment and
//!   the queue played forward), [`window`] (the stated and resident worlds).

pub mod body;
pub mod desk;
pub mod events;
pub mod feed;
pub mod fold;
pub mod moment;
pub mod pieces;
pub mod present;
pub mod source;
pub mod state;
pub mod unpack;
pub mod view;
pub mod window;
pub mod writer;

#[cfg(test)]
mod tests;
