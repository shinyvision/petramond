//! The server-side simulation: the authoritative world, the connected player
//! sessions, and the fixed-tick stage ladder.
//!
//! [`ServerGame`] owns everything the deterministic 20 TPS tick mutates.
//! It runs on its OWN thread (see [`super::handle`]),
//! self-clocked, talking to the client purely over message channels — it must
//! stay `Send` (asserted below). Presentation (camera, particles, lid/swing
//! animation) stays on the client side in `src/game/`.

use std::collections::{BTreeSet, HashMap};

use crate::events::{EventBus, TickSystems};
use crate::modding::ModHost;
use crate::net::protocol::{
    ItemStateRow, MobStateRow, PlayerActionKind, PlayerStateRow, ServerToClient, WorldEventMsg,
};
use crate::player::PlayerId;
use crate::server::player::ConnectedPlayer;
use crate::world::World;
use petramond_math::math::IVec3;
use petramond_world::crafting::Recipes;

mod fixed_tick;
mod player_actions;
mod pump;
mod replication;
mod session_lifecycle;
mod spatial_loops;
mod stream_events;
#[cfg(test)]
mod tests;

pub use replication::wire_world_events;

/// Most fixed ticks run in a single frame before the leftover is dropped. Caps
/// catch-up after a stall so the sim never spirals trying to replay lost time.
pub const MAX_TICKS_PER_FRAME: u32 = 4;

/// One pump's ordered server→client messages PER RECIPIENT — terrain payloads
/// first (column before its sections), then at most one `Tick(TickUpdate)`.
/// Each recipient applies its list in order and consumes NOTHING else (the
/// tick's events ride the `TickUpdate` itself).
pub struct PumpOutput {
    /// The LOCAL session's (index 0) messages, for the in-process pipe.
    pub msgs: Vec<ServerToClient>,
    /// Each REMOTE session's messages, tagged by `PlayerId` — the server
    /// thread routes them to the matching TCP connection.
    pub remote: Vec<(PlayerId, Vec<ServerToClient>)>,
}

/// Minimum number of game ticks between two attack swings: a swing FOLLOWS
/// THROUGH before the hand may attack again, so mashing the button neither
/// lands hits every tick (which would, e.g., instakill an owl) nor cuts the
/// swing out of its own animation. 8 ticks = 0.4 s, the longest swing the
/// engine's rigs play for an unclaimed hand. A pack pacing a tool off its own
/// animation claims [`mod_api::PlayerAttribute::AttackCooldown`] to `0.0` and
/// bars the next attack itself.
pub const ATTACK_COOLDOWN_TICKS: u32 = 8;

/// The per-tick replication parts every recipient shares: built once per tick
/// window by [`ServerGame::shared_tick_rows`], cloned into each recipient's
/// [`TickUpdate`](crate::net::protocol::TickUpdate).
pub struct SharedTickRows {
    tick: u64,
    clock: u64,
    mobs: std::sync::Arc<[MobStateRow]>,
    items: std::sync::Arc<[ItemStateRow]>,
    players: std::sync::Arc<[PlayerStateRow]>,
    player_actions: std::sync::Arc<[(PlayerId, PlayerActionKind)]>,
    open_chests: Vec<IVec3>,
    /// The full shader-param map when anything changed since the last window
    /// (`None` = unchanged) — see [`crate::net::protocol::TickUpdate::env`].
    pub env: Option<Vec<(String, [f32; 4])>>,
}

/// The simulation half of the former `Game`: authoritative world + sessions +
/// the tick machinery. Field-visible to the client crate-side (`pub`)
/// because the client currently owns it in-process; the replica flip narrows
/// this to the wire.
pub struct ServerGame {
    pub world: World,
    /// The connected players' simulation sessions. On a LISTEN server (the
    /// in-game host) the LOCAL session is index 0 and always exists; on a
    /// HEADLESS server every session is remote and the list may be EMPTY —
    /// fixed ticks are skipped while it is (the world freezes between
    /// players), which is what keeps
    /// every `sessions[0]` mod-ABI site sound: they all run inside the tick.
    pub sessions: Vec<ConnectedPlayer>,
    /// Whether `sessions[0]` is THIS process's local player (listen server).
    /// False on a headless server ([`crate::game::session::
    /// build_headless_session`]): no local pipe recipient, every session
    /// windowed by the streaming ack loop, and the leave path may empty the
    /// list.
    pub has_local_session: bool,
    /// Case-folded player names promoted through `op`. Persisted in the
    /// world's engine KV map; the listen server's local session is always an
    /// operator independently of this set.
    pub operators: BTreeSet<String>,
    /// Loaded recipes (from layered `recipes.json`). Player CRAFT requests and
    /// machine-processing ticks share this immutable catalog, which is why it
    /// lives above individual menu sessions.
    pub recipes: Recipes,
    /// Which recipes each discovery opens, derived once from
    /// [`recipes`](Self::recipes). Shared with the engine's unlock handler on
    /// the bus (see `server::progression`) and read by every session start.
    pub unlocks: std::sync::Arc<petramond_world::crafting::UnlockIndex>,
    /// Memo for the hostile-spawn plan's player/terrain half (see
    /// [`crate::mob::HostileSpawnCache`]) — the planner runs every tick, its
    /// chunk-neighbourhood scans do not.
    pub hostile_spawn_cache: crate::mob::HostileSpawnCache,
    /// The modding event bus: pre events dispatch at their decision sites,
    /// post events queue and drain at tick-stage boundaries. Engine handlers
    /// register before any mod's.
    pub bus: EventBus,
    /// Systems attached between the fixed-tick stages.
    pub systems: TickSystems,
    /// The WASM mod instances. Their registered closures (held by
    /// `bus`/`systems`) share ownership; the host keeps the canonical handles
    /// for GUI click dispatch and diagnostics.
    pub mods: ModHost,
    pub spawn_counter: u32,
    /// Next deterministic session handle for spatial sounds. The app
    /// owns playback; this counter only gives mods stable identities for stop calls.
    pub next_spatial_sound_handle: u64,
    /// Wall-clock seconds banked toward the next fixed simulation tick.
    pub tick_accumulator: f32,
    /// Singleplayer pause (`ClientToServer::Pause`): while set, `pump` skips
    /// the fixed ticks ONLY — message drain, streaming, and autosave keep
    /// running — and banks no tick debt (the accumulator is pinned so resume
    /// never fast-forwards). Honored only while [`lan_ever_opened`] is false
    /// (the sole connection is the local one).
    ///
    /// [`lan_ever_opened`]: Self::lan_ever_opened
    pub paused: bool,
    /// Set (permanently, for the session) when "Open to LAN" first succeeds:
    /// the server force-unpauses and `Pause` messages are ignored from then
    /// on — remote players may exist (or reappear) at any time.
    pub lan_ever_opened: bool,
    /// World-anchored wire events produced OUTSIDE a tick window (a leaving
    /// session's menu close, e.g. its chest 1→0 transition), shipped with the
    /// next executed tick's batch so no observer misses them.
    pub pending_wire_events: Vec<WorldEventMsg>,
    /// Every spatial LOOP still playing (a `loop` row started and not yet
    /// stopped), by handle — replayed to a joining session, ended with its
    /// mob. See [`spatial_loops`].
    pub live_spatial_loops: spatial_loops::LiveSpatialLoops,
    /// Chat lines accepted since the last pump. Drained to currently connected
    /// sessions only (per [`crate::server::chat::ChatTargets`]); this is
    /// intentionally not history.
    pub pending_chat: Vec<crate::server::chat::PendingChat>,
    /// Reused buffers for every session's exposure tick.
    pub(crate) exposure_scratch: crate::exposure::ExposureScratch,
    pub next_chat_seq: u64,
    /// Wall-clock seconds since the last background autosave.
    pub autosave_t: f32,
    /// How many players currently have each chest's screen open, keyed by
    /// world position. Server-side state (drives what EVERY client's lid
    /// shows); entries are removed at zero. Updated by the menu open/close
    /// funnels; 0↔1 transitions emit `ChestOpened`/`ChestClosed` world events.
    pub chest_viewers: HashMap<IVec3, u8>,
    /// The live mobs holding each container open (`ContainerHold`), each
    /// counted once among its chest's viewers.
    pub container_holds: HashMap<IVec3, Vec<u64>>,
    /// The `WorldEnvironment` shader-param map the last `TickUpdate.env`
    /// shipped (value-compared per tick window; the map is tiny). `None` =
    /// nothing shipped yet, so the first window always carries the full set.
    /// Replication bookkeeping, not sim state.
    pub last_shipped_env: Option<std::sync::Arc<crate::world::environment::ShaderParamMap>>,
}

/// The whole sim moves to the server thread at spawn ([`super::handle`]);
/// keep the bound loud so a non-`Send` field is caught at ITS introduction,
/// not at the thread boundary.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ServerGame>();
};
