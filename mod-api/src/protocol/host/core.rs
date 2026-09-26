//! Instance basics: logging, the tick clock, the mod's RNG streams, the
//! `mod_init` registration window, shader parameters, and a mod's own events.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::events::{EventFilter, EventKind};
use crate::ids::PlayerId;
use crate::legality::prelude::*;
use crate::sched::{AttachSide, Stage};

host_domain! {
    /// Instance basics: logging, the tick clock, the mod's RNG streams, the
    /// `mod_init` registration window, shader parameters, and a mod's own events.
    CoreCall {
        /// Log through the engine logger (mods have no stdout).
        Log {
            msg: String,
        } => legal(EVERY, Any, Read),
        /// The current game tick (20 per second). → [`HostRet::U64`](crate::HostRet::U64).
        CurrentTick => legal(SERVER, Sim, Read),
        /// Next value of the mod's named deterministic RNG stream (SplitMix64,
        /// seeded from world seed + mod id + key). → [`HostRet::U64`](crate::HostRet::U64).
        RngU64 {
            stream_key: String,
        } => legal(EVERY, Any, Read),
        /// Attach a tick system. Legal ONLY during `mod_init`; the engine later
        /// dispatches [`GuestCall::TickSystem`](crate::GuestCall::TickSystem) with `system_id` every tick.
        RegisterTickSystem {
            stage: Stage,
            attach: AttachSide,
            priority: i32,
            system_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Register an event handler. Legal ONLY during `mod_init`; the engine
        /// later dispatches [`GuestCall::HandleEvent`](crate::GuestCall::HandleEvent)
        /// with `handler_id` — for the events `filter` admits only, evaluated
        /// host-side before any crossing (an empty [`EventFilter`] admits
        /// every event of the kind). A filter lane the kind never carries is
        /// refused with [`ErrorCode::InvalidArgument`](crate::ErrorCode::InvalidArgument)
        /// (see [`EventFilter::check`](crate::EventFilter::check)).
        RegisterEventHandler {
            event: EventKind,
            priority: i32,
            handler_id: u32,
            filter: EventFilter,
        } => legal(EVERY, Init, Write),
        /// Set one named visual shader parameter (`vec4<f32>`). Mods may write
        /// their own `mod_id:name` keys or exposed engine `petramond:*` keys; active
        /// shader packs map keys onto fixed GPU slots. Not persisted: re-apply it
        /// from mod state on load.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ShaderSetParam {
            key: String,
            value: [f32; 4],
        } => legal(SERVER, Sim, Write),
        /// Register a hostile-spawn callback. The core engine supplies candidate
        /// sites and enforces caps/body fit; the callback returns a hostile mob key
        /// if this mod wants to spawn something there. Legal ONLY during `mod_init`.
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterHostileSpawner {
            callback_id: u32,
            priority: i32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Register the reactive behavior for block rows whose `blocks.json`
        /// `behavior` field is `key` — a `mod_id:name` owned by THIS pack. The
        /// engine then dispatches [`GuestCall::BlockBehavior`](crate::GuestCall::BlockBehavior) with `callback_id`
        /// for every hook that fires on such a block. Legal ONLY during
        /// `mod_init`. → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterBlockBehavior {
            key: String,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Register the scripted AI node for `mobs.json` brain rows whose `node`
        /// key is `key` — a `mod_id:name` owned by THIS pack. The engine then
        /// dispatches [`GuestCall::AiNode`](crate::GuestCall::AiNode) with `callback_id` once per owning
        /// mob per game tick. Legal ONLY during `mod_init`. → [`HostRet::Unit`](crate::HostRet::Unit).
        RegisterAiNode {
            key: String,
            callback_id: u32,
        } => legal(SERVER_WORLDGEN, Init, Write),
        /// Identify this isolated module instance. → [`HostRet::RuntimeSide`](crate::HostRet::RuntimeSide).
        RuntimeSide => legal(EVERY, Any, Read),
        /// Emit one of the calling mod's OWN events onto the post-event queue,
        /// dispatched at the next drain point in this same tick (never inline —
        /// re-entering the bus from inside a guest dispatch is forbidden, exactly
        /// like [`DamagePlayer`](crate::PlayerCall::DamagePlayer) queueing its action).
        ///
        /// `key` must carry the calling mod's `mod_id:` prefix; `data` is that
        /// mod's own opaque payload, capped like a KV value. Every handler
        /// registered for [`EventKind::ModEvent`] sees it, so the key is the
        /// filter. → [`HostRet::Unit`](crate::HostRet::Unit).
        ///
        /// [`EventKind::ModEvent`]: crate::EventKind::ModEvent
        EmitEvent {
            key: String,
            #[serde(with = "serde_bytes")]
            data: Vec<u8>,
        } => legal(SERVER, Sim, Write),
        /// Deliver one of this mod's OWN events to `player`'s CLIENT instance —
        /// the wire-crossing half of [`EmitEvent`](Self::EmitEvent), with the same
        /// key/payload vocabulary and the same namespace and size guards. The
        /// client half of the pack receives it as an ordinary
        /// [`EventKind::ModEvent`] dispatch, so a mod handles it with the handler
        /// it already knows how to write. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = no such
        /// reachable session).
        ///
        /// It exists because a client mod can predict what LOCAL INPUT implies
        /// and nothing else. Anything the server decided — a hit landing, a timer
        /// expiring, another player acting — is otherwise unknowable there. Send
        /// the EDGE, not a state mirror: ship "it happened" once (like the hurt
        /// flash it sits beside) and let the client run its own envelope.
        ///
        /// The cue rides the recipient's next replication batch, so it arrives one
        /// batch late. That makes this lane presentation, never simulation:
        /// anything both sides must agree about stays on the server.
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
