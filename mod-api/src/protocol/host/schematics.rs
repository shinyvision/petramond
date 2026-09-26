//! Schematics: catalog reads, cell lists, player choices and positioning, ghosts.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    /// Schematics: catalog reads, cell lists, player choices and positioning, ghosts.
    SchematicCall {
        /// A world-held schematic's facts, starting a background decode on the
        /// first ask. Server only. → [`HostRet::Schematic`](crate::HostRet::Schematic).
        SchematicInfo {
            asset: crate::SchematicId,
        } => legal(SERVER, Sim, Read),
        /// Stored section `section` of a decoded schematic turned `turns` quarter
        /// turns clockwise, as construction records. `None` = not decoded yet
        /// (see [`SchematicInfo`](Self::SchematicInfo)) or no such section.
        /// Server only. → [`HostRet::SchematicCells`](crate::HostRet::SchematicCells).
        SchematicCells {
            asset: crate::SchematicId,
            section: u32,
            turns: u8,
        } => legal(SERVER, Sim, Read),
        /// Ask `player`'s client to choose a schematic from their library for
        /// `tag` (namespaced to this mod). The choice arrives as
        /// [`EventKind::SchematicChosen`](crate::EventKind::SchematicChosen) once
        /// the world holds the design — uploaded from the client if it did not.
        /// A newer request for the same player replaces an open one.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such connected player).
        SchematicChoose {
            player: PlayerId,
            tag: String,
        } => legal(SERVER, Sim, Write),
        /// Ask `player`'s client to position the world-held schematic `asset`
        /// for `tag` (namespaced to this mod) with the placement controls,
        /// starting at `origin` when given. Anchoring it arrives as
        /// [`EventKind::SchematicPositioned`](crate::EventKind::SchematicPositioned).
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such player or no such asset).
        SchematicPosition {
            player: PlayerId,
            tag: String,
            asset: crate::SchematicId,
            origin: Option<[i32; 3]>,
            turns: u8,
        } => legal(SERVER, Sim, Write),
        /// Anchor, move or remove (`None`) the ghost `key` (namespaced to this
        /// mod): a translucent render of a world-held schematic where it will be
        /// built, drawn by the viewers' clients from the remaining work.
        /// Presentation, never persisted: re-set it after a restart.
        /// → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such asset).
        SchematicGhostSet {
            key: String,
            ghost: Option<crate::SchematicGhostData>,
        } => legal(SERVER, Sim, Write),
    }
}
