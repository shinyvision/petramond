//! Presenting a world from mod-file byte ranges: a world-less presentation a
//! client mod opens from the shell and moves through by applying ranges.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::capture::ClientPose;
use crate::files::{ClientFileRange, ClientFileRanges};
use crate::legality::prelude::*;

host_domain! {
    /// A world presented from mod-file byte ranges. It opens FROM the shell, so
    /// the whole family answers there; each call says what it answers with none
    /// open.
    ClientPresentationCall {
        /// CLIENT: open a world-less presentation, from the SHELL only: `tables`
        /// is one complete `Tables` piece, `seed` the world seed, `mods` the
        /// client mods to host beside the caller (their ids), `viewer` where the
        /// viewer's eye starts. The world starts empty.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = refused (a live
        /// world is presented, or another mod owns the presentation), the reason
        /// in the state's `error`.
        ClientPresentationOpen {
            tables: ClientFileRange,
            seed: u32,
            mods: Vec<String>,
            viewer: Option<ClientPose>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: make the presented world the previous one ⊕ `state` ⊕ `events`
        /// folded in as state, presented at `at` (fractional ticks). Applies
        /// land in issue order, each onto the world the one before left.
        /// → [`HostRet::Ticket`](crate::HostRet::Ticket): the id, landed once
        /// the state's `applied` is it; [`ErrorCode::Refused`](crate::ErrorCode::Refused)
        /// = this instance owns no open
        /// presentation.
        ClientPresentationApply {
            state: Vec<ClientFileRanges>,
            events: Vec<ClientFileRanges>,
            at: f64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: drop a pending apply. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `true` = it will never land; `false` = it already landed.
        ClientPresentationCancel {
            apply: u64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: append events ranges to play forward from where the most
        /// recent apply leaves the world. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` = no open presentation of this instance.
        ClientPresentationQueue {
            events: Vec<ClientFileRanges>,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: move the presented position to `at`, releasing queued frames
        /// as it passes them. Back only inside the committed tick pair.
        /// → [`HostRet::Bool`](crate::HostRet::Bool).
        ClientPresentationTime {
            at: f64,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: place the presentation's viewer: its eye at `pose`, flying or
        /// walking. → [`HostRet::Bool`](crate::HostRet::Bool): `false` = not the
        /// owner, or none open.
        ClientPresentationViewer {
            pose: ClientPose,
            flying: bool,
        } => legal(CLIENT_SHELL, Any, Write),
        /// CLIENT: where the presentation stands.
        /// → [`HostRet::ClientPresentationState`](crate::HostRet::ClientPresentationState).
        ClientPresentationState => legal(CLIENT_SHELL, Any, Read),
        /// CLIENT: close this instance's presentation at the next frame.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ClientPresentationClose => legal(CLIENT_SHELL, Any, Write),
    }
}
