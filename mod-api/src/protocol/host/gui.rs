//! Mod GUIs: session state, opening and closing, viewers.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::{ContainerAddress, GuiValue};
use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    /// Mod GUIs: session state, opening and closing, viewers.
    GuiCall {
        /// Write a key of the open GUI session's state map (tick-owned; the
        /// renderer reads a snapshot per frame). Keys are mod-local — the map
        /// belongs to one GUI session and is cleared on open/close. Sim-scoped.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        GuiStateSet {
            key: String,
            value: GuiValue,
        } => legal(SERVER, Sim, Write),
        /// Read a key of the GUI state map (`None` = absent). Sim-scoped.
        /// → [`HostRet::GuiValue`](crate::HostRet::GuiValue).
        GuiStateGet {
            key: String,
        } => legal(SERVER, Sim, Read),
        /// Ask the app shell to open the mod GUI registered under `kind_key`
        /// (`"wheel:wheel"` — a baked manifest or `open_gui` block row must have
        /// registered it). Queued like [`PlayerCall::DamagePlayer`](crate::PlayerCall::DamagePlayer); the screen
        /// opens after this tick and may replace an open menu. `at` anchors the
        /// session: a block (its container backs the document's `container`
        /// slots) or a live mob (its carried slots do; the session closes when
        /// the mob leaves). `false` = unknown / non-mod kind. → [`HostRet::Bool`](crate::HostRet::Bool).
        GuiOpen {
            kind_key: String,
            at: Option<crate::ContainerAddress>,
        } => legal(SERVER, Sim, Write),
        /// Close the open mod GUI (a no-op if none is open — engine containers
        /// are not closable from mods). Queued like [`GuiCall::GuiOpen`].
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        GuiClose => legal(SERVER, Sim, Write),
        /// Every connected session with a mod GUI open right now, in session
        /// order. → [`HostRet::GuiViewers`](crate::HostRet::GuiViewers).
        ///
        /// This is the "who is looking" snapshot [`GuiStateSetFor`] is addressed
        /// with, and it is a QUERY rather than a pair of events on purpose: a mod
        /// that tracked opens and closes itself would have to be right about
        /// disconnects, deaths and world unloads to stay in step, and it holds one
        /// answer for the whole server anyway.
        ///
        /// [`GuiStateSetFor`]: Self::GuiStateSetFor
        GuiViewers => legal(SERVER, Sim, Read),
        /// Write one key of a SPECIFIC session's GUI state map — the per-player
        /// form of [`GuiStateSet`](Self::GuiStateSet). `false` = no such connected
        /// session.
        ///
        /// The implicit call writes the dispatch's ACTOR's map, and a TICK SYSTEM
        /// acts for nobody — so a machine publishing gauges from its tick has no
        /// implicit map to write. Every mod with a live readout needs this one;
        /// nothing on the implicit surface can substitute for it.
        ///
        /// Pair it with [`GuiViewers`](Self::GuiViewers): publish per viewer, and
        /// the flat key space stops being a problem too — one session has one GUI
        /// open, so its map cannot hold two machines' readings at once.
        GuiStateSetFor {
            player_id: PlayerId,
            key: String,
            value: GuiValue,
        } => legal(SERVER, Sim, Write),
        /// [`GuiStateGet`](Self::GuiStateGet) from a named session's GUI state
        /// map — the read twin of [`GuiStateSetFor`](Self::GuiStateSetFor).
        /// → [`HostRet::GuiValue`](crate::HostRet::GuiValue): `None` = unset key or no such session.
        GuiStateGetFor {
            player_id: PlayerId,
            key: String,
        } => legal(SERVER, Sim, Read),
        /// [`GuiOpen`](Self::GuiOpen) for a named session. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` = unknown kind, absent anchor, or no such session.
        GuiOpenFor {
            player_id: PlayerId,
            kind_key: String,
            at: Option<ContainerAddress>,
        } => legal(SERVER, Sim, Write),
        /// [`GuiClose`](Self::GuiClose) for a named session. → [`HostRet::Bool`](crate::HostRet::Bool):
        /// `false` = no such connected session.
        GuiCloseFor {
            player_id: PlayerId,
        } => legal(SERVER, Sim, Write),
    }
}
