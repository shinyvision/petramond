pub use mod_api::capture::*;

use mod_api::{ClientFileRange, ClientFileRanges, ClientStorageScope};

use crate::__rt::{host_fn, try_host_fn};

try_host_fn! {
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
    pub fn client_world_events_end(events: u64) => ClientWorldEventsEnd { events }
}

host_fn! {
    pub fn client_world_events_poll(events: u64) -> Option<ClientEventsReport>
        => ClientWorldEventsPoll { events } => ClientEvents
}

host_fn! {
    pub fn client_presentation_open(
        tables: ClientFileRange,
        seed: u32,
        mods: Vec<String>,
        viewer: Option<ClientPose>,
    ) -> bool => ClientPresentationOpen { tables, seed, mods, viewer } => Bool
}

try_host_fn! {
    pub fn client_presentation_apply(
        state: Vec<ClientFileRanges>,
        events: Vec<ClientFileRanges>,
        at: f64,
    ) -> u64 => ClientPresentationApply { state, events, at } => Ticket
}

host_fn! {
    pub fn client_presentation_cancel(apply: u64) -> bool
        => ClientPresentationCancel { apply } => Bool
}

host_fn! {
    pub fn client_presentation_queue(events: Vec<ClientFileRanges>) -> bool
        => ClientPresentationQueue { events } => Bool
}

host_fn! {
    pub fn client_presentation_time(at: f64) -> bool => ClientPresentationTime { at } => Bool
}

host_fn! {
    pub fn client_presentation_viewer(pose: ClientPose, flying: bool) -> bool
        => ClientPresentationViewer { pose, flying } => Bool
}

pub fn client_presentation_state() -> ClientPresentationStateData {
    *crate::__rt::expect_value(
        "ClientPresentationState",
        crate::__rt::call(&crate::calls::ClientPresentationState)
            .decode_as(mod_api::ret_decode::ClientPresentationState),
    )
}

host_fn! {
    pub fn client_presentation_close() => ClientPresentationClose
}
