//! The wire protocol: everything a client and server say to each other.
//!
//! Messages are plain Rust values passed UNSERIALIZED over the in-process
//! connection; the TCP transport postcard-encodes them on its own threads.
//! Registry ids on the wire are RAW session ids (`u8`/`u16` fields named
//! `*_id`); the TCP client remaps them against the [`JoinData::tables`] name
//! tables at the transport boundary (see `net::remap`) — everything above the
//! transport speaks purely client-local ids. The local connection skips the
//! remap entirely (same process, same registries).
//!
//! Wire-compat: break freely and bump [`super::PROTOCOL_VERSION`] — nothing is
//! released, so there are no old clients to keep decoding an older dialect.

use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::Vec3;
use crate::server::player::PlayerId;

mod actions;
mod chat;
mod join;
mod menu;
mod state;
mod terrain;

#[cfg(test)]
mod tests;

pub(crate) use actions::*;
pub(crate) use chat::*;
pub(crate) use join::*;
pub(crate) use menu::*;
pub(crate) use state::*;
pub(crate) use terrain::*;

/// The kinematic transform every player-shaped wire row carries: feet
/// position, velocity, and look angles. Embedded, not flattened — postcard
/// encodes a nested struct as its fields in order, so the wire bytes equal the
/// four fields spelled inline.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Transform {
    pub pos: Vec3,
    pub vel: Vec3,
    pub yaw: f32,
    pub pitch: f32,
}

/// One inventory slot on the wire. `item_id` is a wire item id.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ItemSlotWire {
    pub item_id: u8,
    pub count: u8,
}

/// Client → server messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ClientToServer {
    /// First frame on any fresh connection; nothing else parses across a
    /// protocol mismatch.
    Hello {
        protocol: u16,
    },
    /// "Which mods do you have?" — asked (and answered) BEFORE joining, per
    /// the mod-handshake contract.
    ModQuery,
    Join {
        player_name: String,
        /// The client's view distance in chunks. The server streams
        /// `min(this, its own maximum)` for the session.
        view_distance: u8,
        /// The sections this client still holds in its section cache from an
        /// earlier session, by server-domain content hash — seeds the server's
        /// per-connection cache belief so a reconnect can re-promote instead
        /// of re-streaming. Empty on a fresh client.
        cached_sections: Vec<SectionCacheClaim>,
    },
    PlayerUpdate(PlayerUpdate),
    Action(PlayerAction),
    /// A hit-tested container-menu click (slot identity + button + Shift +
    /// the client's double-click gather verdict), latched to the next tick.
    MenuClick {
        slot: MenuSlotWire,
        button: u8,
        shift: bool,
        gather: bool,
        request_id: ClientRequestId,
    },
    /// One cursor-stack distribution gesture. `slots` is the bounded,
    /// first-hit order of distinct logical destinations; the server performs
    /// the split atomically on the next deterministic tick.
    MenuDrag {
        slots: Vec<MenuSlotWire>,
        button: u8,
        request_id: ClientRequestId,
    },
    /// Drop one item or a whole stack directly from the hovered menu slot.
    MenuDrop {
        slot: MenuSlotWire,
        all: bool,
        request_id: ClientRequestId,
    },
    /// Craft one stable, name-addressed recipe into the open crafting
    /// session's output slot (merging onto a same-item output stack). The
    /// server validates station, ingredients, and output fit on the next
    /// deterministic tick. `bulk` (shift-craft) repeats until ingredients run
    /// out or the output stack fills.
    CraftRecipe {
        recipe: String,
        bulk: bool,
        request_id: ClientRequestId,
    },
    /// A player-submitted chat line. The server trims/sanitizes/formats it and
    /// broadcasts the resulting [`ServerToClient::ChatLine`].
    ChatSend {
        text: String,
    },
    /// Acknowledge one streaming batch (`StreamBatchStart`..`StreamBatchEnd`)
    /// and report the rate this client actually applied it at — the
    /// end-to-end flow-control signal the server sizes future batches from
    /// (the 1.20.2 chunk-batching design).
    StreamBatchAck {
        messages_per_second: f32,
    },
    /// The server sent [`ServerToClient::SectionCached`] for a section this
    /// client no longer holds (cap eviction, declined cache, hash drift). The
    /// server forgets its belief and re-streams the full payload — the
    /// self-healing path for ANY cache-bookkeeping divergence.
    SectionCacheMiss {
        pos: SectionPos,
    },
    /// The client changed its view distance (Options → Graphics). The server
    /// re-clamps to its own maximum and streams the new radius; terrain
    /// outside it unloads client-side through the ordinary diff.
    SetViewDistance {
        chunks: u8,
    },
    /// The client toggled the recipe browser's craftable-only filter — a
    /// fire-and-forget preference the server stores on the player so it
    /// persists with the world's player data.
    SetCraftFilter {
        craftable_only: bool,
    },
    Pause(bool),
    KeepAlive,
    Disconnect,
}

/// Server → client messages.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) enum ServerToClient {
    HelloAck {
        protocol: u16,
    },
    HelloReject {
        server_protocol: u16,
    },
    /// The hosted world's ENABLED mod set (`modding/modset.rs::active`).
    ModList {
        mods: Vec<ModEntry>,
    },
    JoinAccept(Box<JoinData>),
    JoinReject {
        reason: JoinRejectReason,
    },
    ColumnData(ColumnPayload),
    /// Boxed like [`Tick`](Self::Tick): the payload's sparse state vecs make
    /// it by far the largest variant, and it dominates channel traffic during
    /// a world load.
    SectionData(Box<SectionPayload>),
    /// A server light bake landed for a section already sent to this
    /// recipient: the fresh cubes replace the seeded ones. This is the only
    /// AUTHORITATIVE post-install light writer; disposable prediction bakes
    /// are local presentation and lose to this payload.
    LightData(LightPayload),
    /// Drop one section that left the keep shape while its column stays. When
    /// `cache_hash` is present the server still held the section and vouches
    /// that the recipient's replica copy equals content-hash `cache_hash` (the
    /// connection is ordered, so every delta/light write landed before this) —
    /// the client may park that copy in its section cache for a later
    /// [`SectionCached`](Self::SectionCached) re-promotion. `None` = drop only.
    SectionUnload {
        pos: SectionPos,
        cache_hash: Option<u64>,
    },
    /// Drop a whole column and, implicitly, every live section in it (no
    /// per-section [`SectionUnload`](Self::SectionUnload) precedes this).
    /// `cache_hashes` carries `(cy, content hash)` for each dropped section
    /// the server can vouch for, same contract as `SectionUnload::cache_hash`.
    ColumnUnload {
        pos: ChunkPos,
        cache_hashes: Vec<(i32, u64)>,
    },
    /// In place of a [`SectionData`](Self::SectionData) whose content the
    /// recipient already holds cached (matching claim in the server's
    /// per-connection belief): re-promote the cached copy keyed by `hash`.
    /// A client that no longer holds it answers
    /// [`SectionCacheMiss`](ClientToServer::SectionCacheMiss). Counts as a
    /// streaming message inside batch brackets, exactly like `SectionData`.
    SectionCached {
        pos: SectionPos,
        hash: u64,
    },
    /// Brackets the start of one streaming batch (terrain/light/unload
    /// messages) on a WINDOWED connection: the client times Start→End
    /// application and answers `StreamBatchAck`. Loopback uses the same
    /// protocol with a one-batch window, bounding its unbounded channel.
    StreamBatchStart,
    /// Ends the batch `StreamBatchStart` opened; `count` is the number of
    /// streaming messages in between (the client's rate denominator).
    StreamBatchEnd {
        count: u32,
    },
    Tick(Box<TickUpdate>),
    PlayerJoined {
        id: PlayerId,
        name: String,
    },
    PlayerLeft {
        id: PlayerId,
    },
    ChatLine(ChatLine),
    ServerClosing,
    KeepAlive,
    Disconnect {
        reason: String,
    },
}
