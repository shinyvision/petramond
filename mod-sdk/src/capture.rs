//! Capturing a presented world into this mod's files, and presenting one back
//! from them — plus the capture format itself (`mod_sdk::capture::records`,
//! `decode_documented`, ...), the one implementation the engine writes and
//! reads with.
//!
//! The engine writes what a sandboxed mod cannot read (the replica's full
//! content) where the mod asks, and presents what the mod hands back. How the
//! files are laid out, when they are written and how they are read back is
//! the mod's.

pub use mod_api::capture::*;

use mod_api::{ClientFileRange, ClientFileRanges, ClientStorageScope, HostRet};

use crate::__rt::{host_fn, try_host_fn};

try_host_fn! {
    /// CLIENT: append a State record of the presented world, as the last
    /// presented frame left it, to `path` in `scope` — the keys `select`
    /// names, narrowed to `kinds` (`None` = every state kind). With
    /// `envelopes`, each record's envelope entry is appended to that file too.
    /// `Ok { write, revision }`: `write` polls with [`crate::client_file_poll`],
    /// `revision` is the revision the state describes. `Err` = refused, with a
    /// reason a player can read.
    pub fn client_world_state_write(
        scope: ClientStorageScope,
        path: &str,
        select: ClientStateSelect,
        kinds: Option<Vec<ClientPieceKind>>,
        envelopes: Option<&str>,
    ) -> ClientStateTicketData => ClientWorldStateWrite {
        scope,
        path: path.into(),
        select,
        kinds,
        envelopes: envelopes.map(Into::into),
    } => ClientStateTicket
}

try_host_fn! {
    /// CLIENT: start an events log: every presented frame in which anything
    /// happened appends one Frame record to `path` (and its envelope entry to
    /// `envelopes`). `Ok(log)`; `Err` = refused.
    pub fn client_world_events_begin(
        scope: ClientStorageScope,
        path: &str,
        envelopes: Option<&str>,
    ) -> u64 => ClientWorldEventsBegin {
        scope,
        path: path.into(),
        envelopes: envelopes.map(Into::into),
    } => Ticket
}

host_fn! {
    /// CLIENT: end an events log: queued frames are written, the file synced.
    pub fn client_world_events_end(events: u64) => ClientWorldEventsEnd { events }
}

host_fn! {
    /// CLIENT: where an events log stands; `None` = no such log.
    pub fn client_world_events_poll(events: u64) -> Option<ClientEventsReport>
        => ClientWorldEventsPoll { events } => ClientEvents
}

host_fn! {
    /// CLIENT: open a world-less presentation from the shell: `tables` is one
    /// complete `Tables` piece, `mods` the client mods to host beside this one.
    /// `false` = refused, the reason in [`client_presentation_state`]'s `error`.
    pub fn client_presentation_open(
        tables: ClientFileRange,
        seed: u32,
        mods: Vec<String>,
        viewer: Option<ClientPose>,
    ) -> bool => ClientPresentationOpen { tables, seed, mods, viewer } => Bool
}

try_host_fn! {
    /// CLIENT: make the presented world the previous one ⊕ `state` ⊕ `events`
    /// folded in as state, presented at `at`. Applies land in issue order.
    /// `Ok(apply)`; `Err` = this mod owns no open presentation.
    pub fn client_presentation_apply(
        state: Vec<ClientFileRanges>,
        events: Vec<ClientFileRanges>,
        at: f64,
    ) -> u64 => ClientPresentationApply { state, events, at } => Ticket
}

host_fn! {
    /// CLIENT: drop a pending apply. `false` = it already landed.
    pub fn client_presentation_cancel(apply: u64) -> bool
        => ClientPresentationCancel { apply } => Bool
}

host_fn! {
    /// CLIENT: append events ranges to play forward. `false` = none open.
    pub fn client_presentation_queue(events: Vec<ClientFileRanges>) -> bool
        => ClientPresentationQueue { events } => Bool
}

host_fn! {
    /// CLIENT: move the presented position to `at` (fractional ticks),
    /// releasing queued frames as it passes them. `false` = back past the
    /// committed tick pair, or none open.
    pub fn client_presentation_time(at: f64) -> bool => ClientPresentationTime { at } => Bool
}

host_fn! {
    /// CLIENT: place the presentation's viewer, its eye at `pose`, flying or
    /// walking. `false` = not the owner, or none open.
    pub fn client_presentation_viewer(pose: ClientPose, flying: bool) -> bool
        => ClientPresentationViewer { pose, flying } => Bool
}

host_fn! {
    /// CLIENT: where the presentation stands.
    pub fn client_presentation_state() -> ClientPresentationStateData
        => ClientPresentationState
        => HostRet::ClientPresentationState(state) => *state
}

host_fn! {
    /// CLIENT: close this mod's presentation at the next frame.
    pub fn client_presentation_close() => ClientPresentationClose
}
