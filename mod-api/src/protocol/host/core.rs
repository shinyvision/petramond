use crate::events::{EventFilter, EventKind};
use crate::ids::PlayerId;
use crate::legality::prelude::*;
use crate::sched::{AttachSide, Stage};

host_domain! {
    CoreCall {
        Log {
            msg: String,
        } => legal(EVERY, Any, Read),
        CurrentTick => legal(SERVER, Sim, Read),
        RngU64 {
            stream_key: String,
        } => legal(EVERY, Any, Read),
        RegisterTickSystem {
            stage: Stage,
            attach: AttachSide,
            priority: i32,
            system_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RegisterEventHandler {
            event: EventKind,
            priority: i32,
            handler_id: u32,
            filter: EventFilter,
        } => legal(EVERY, Init, Write),
        ShaderSetParam {
            key: String,
            value: [f32; 4],
        } => legal(SERVER, Sim, Write),
        RegisterHostileSpawner {
            callback_id: u32,
            priority: i32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RegisterBlockBehavior {
            key: String,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RegisterAiNode {
            key: String,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        RuntimeSide => legal(EVERY, Any, Read),
        EmitEvent {
            key: String,
            #[serde(with = "serde_bytes")]
            data: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        /// Sends one of this mod's own events to `player`'s client, the wire-crossing half of
        /// [`EmitEvent`](Self::EmitEvent) with the same guards. The client gets an ordinary
        /// [`EventKind::ModEvent`]. Returns [`HostRet::Bool`](crate::HostRet::Bool), false if
        /// no reachable session.
        ///
        /// Client mods can't see hits, timers or other players, so send the edge once and let
        /// the client run its own envelope. It shows up a replication batch late, so it's for
        /// presentation only.
        ///
        /// [`EventKind::ModEvent`]: crate::EventKind::ModEvent
        EmitEventTo {
            player: PlayerId,
            key: String,
            #[serde(with = "serde_bytes")]
            data: Vec<u8>,
        } => legal(SERVER, Sim, Write),
    }
}
