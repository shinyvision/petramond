//! Copying the presented world into mod files: its state as a record, and
//! what happens in it, frame by frame, as an events log.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::capture::{ClientPieceKind, ClientStateSelect};
use crate::client::ClientStorageScope;
use crate::legality::prelude::*;

host_domain! {
    /// The presented world's state and events, into mod files. Starting a
    /// write needs a presented world; a log's handle is polled and ended
    /// wherever the instance runs.
    ClientCaptureCall {
        /// CLIENT: append a State record of the presented world, as the last
        /// presented frame left it, to `path` — the keys `select` names, narrowed
        /// to `kinds` (`None` = every state kind). The snapshot is taken inside
        /// the call; the record is encoded and written off the frame. An empty
        /// selection writes nothing and its ticket completes at once. With
        /// `envelopes`, the record's envelope entry is appended to that second
        /// file of the same scope once the record has landed.
        ///
        /// → [`HostRet::ClientStateTicket`](crate::HostRet::ClientStateTicket):
        /// `{ write, revision }` — `write` polls with
        /// [`ClientFilePoll`](crate::ClientFileCall::ClientFilePoll)
        /// (`Done { range, envelope }`), `revision` is the revision the state
        /// describes. [`ErrorCode::Refused`](crate::ErrorCode::Refused): the
        /// `World` scope in a presentation.
        ClientWorldStateWrite {
            scope: ClientStorageScope,
            path: String,
            select: ClientStateSelect,
            kinds: Option<Vec<ClientPieceKind>>,
            envelopes: Option<String>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: start an events log: from the next applied frame on, every
        /// presented frame in which anything happened appends one Frame record
        /// to `path` (and its envelope entry to `envelopes`, if named). No frame
        /// is sampled and none skipped.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket): the log's id.
        ClientWorldEventsBegin {
            scope: ClientStorageScope,
            path: String,
            envelopes: Option<String>,
        } => legal(CLIENT, Any, Write),
        /// CLIENT: the log takes no new frame; queued frames are written and the
        /// file synced. → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientWorldEventsEnd {
            events: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: where a log stands.
        /// → [`HostRet::ClientEvents`](crate::HostRet::ClientEvents): `None` = no
        /// such log of this instance; a final report stays until polled once.
        ClientWorldEventsPoll {
            events: u64,
        } => legal(CLIENT_SHELL, Any, Read),
    }
}
