use crate::data::{ContainerAddress, GuiValue};
use crate::ids::PlayerId;
use crate::legality::prelude::*;

host_domain! {
    GuiCall {
        GuiStateSet {
            key: String,
            value: GuiValue,
        } => legal(SERVER, Sim, Write),
        GuiStateGet {
            key: String,
        } => legal(SERVER, Sim, Read),
        GuiOpen {
            kind_key: String,
            at: Option<crate::ContainerAddress>,
        } => legal(SERVER, Sim, Write),
        GuiClose => legal(SERVER, Sim, Write),
        GuiViewers => legal(SERVER, Sim, Read),
        /// Write one key of a specific session's GUI state map, the per-player
        /// form of [`GuiStateSet`](Self::GuiStateSet). `false` = no such connected
        /// session.
        ///
        /// Implicit calls write the actor's map, but a tick system acts for nobody.
        /// A machine publishing gauges from its tick has no implicit map, so it
        /// needs this call instead.
        ///
        /// Pair with [`GuiViewers`](Self::GuiViewers) to publish per viewer. One
        /// session has one GUI open, so its map can't hold two machines' readings
        /// at once.
        GuiStateSetFor {
            player_id: PlayerId,
            key: String,
            value: GuiValue,
        } => legal(SERVER, Sim, Write),
        GuiStateGetFor {
            player_id: PlayerId,
            key: String,
        } => legal(SERVER, Sim, Read),
        GuiOpenFor {
            player_id: PlayerId,
            kind_key: String,
            at: Option<ContainerAddress>,
        } => legal(SERVER, Sim, Write),
        GuiCloseFor {
            player_id: PlayerId,
        } => legal(SERVER, Sim, Write),
    }
}
