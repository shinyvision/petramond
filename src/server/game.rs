//! The server-side simulation: the authoritative world, the connected player
//! sessions, and the fixed-tick stage ladder.
//!
//! [`ServerGame`] owns everything the deterministic 20 TPS tick mutates.
//! It runs on its OWN thread (see [`super::handle`]),
//! self-clocked, talking to the client purely over message channels — it must
//! stay `Send` (asserted below). Presentation (camera, particles, lid/swing
//! animation) stays on the client side in `src/game/`.

use crate::events::{EventBus, TickSystems};
use crate::net::protocol::{ServerToClient, SleepTally};
use crate::player::PlayerId;
use crate::server::admissions::Admissions;
use crate::server::chat::ChatService;
use crate::server::drops::DropSeeds;
use crate::server::mod_runtime::ModRuntime;
use crate::server::player::ConnectedPlayer;
use crate::server::progression::RecipeCatalog;
use crate::server::sessions::SessionRegistry;
use crate::server::viewers::ContainerViewers;
use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::crafting::Recipes;

mod clock;
mod entity_rows;
mod event_scope;
mod fixed_tick;
mod interest;
mod isolation;
mod player_actions;
mod pump;
mod replication;
mod session_lifecycle;
mod spatial_loops;
mod stream_events;
#[cfg(test)]
mod tests;

use clock::FrameClock;
pub use interest::EntityInterest;
pub use replication::wire_world_events;
use replication::Broadcast;

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
    /// Remote sessions evicted this pump because their work panicked (see
    /// `isolation`), with their names when the leave path completed. The
    /// server thread disconnects them and announces the leave.
    pub kicked: Vec<(PlayerId, Option<String>)>,
}

/// One tick window's replication parts: built once per window by
/// [`ServerGame::shared_tick_rows`], cut into each recipient's
/// [`TickUpdate`](crate::net::protocol::TickUpdate).
pub struct SharedTickRows {
    tick: u64,
    clock: u64,
    /// Each session's entity lanes, indexed like `sessions` — selections over
    /// row tables shared between them.
    recipients: Vec<entity_rows::RecipientEntities>,
    sleep_tally: SleepTally,
    open_chests: Vec<IVec3>,
    /// The shader params that changed since the last window (`None` =
    /// unchanged) — see [`crate::net::protocol::TickSection::Env`].
    pub env: Option<Vec<(String, [f32; 4])>>,
    /// The window's drained world feeds: events, cell deltas and live loops,
    /// scoped per recipient when its batch is cut. Empty for a batch built
    /// outside a tick window.
    feeds: replication::WindowFeeds,
}

/// The authoritative server: a thin coordinator over owned subsystems. Each
/// subsystem keeps its own state behind its own API (the session registry,
/// the mod runtime, container viewers, chat, the frame clock, replication
/// bookkeeping, the recipe catalog); the stage systems in the sibling
/// modules borrow the pieces they need. Nothing outside `crate::server`
/// reaches a field: the rest of the engine and the client talk to the server
/// through the methods below and the message pipe.
pub struct ServerGame {
    pub(in crate::server) world: ServerWorld,
    /// The connected players' simulation sessions (see [`SessionRegistry`]).
    pub(in crate::server) sessions: SessionRegistry,
    /// Joins whose player is being restored off the server thread (see
    /// [`crate::server::admissions`]).
    pub(in crate::server) admissions: Admissions,
    /// Whether joining players must prove a Petramond account. Online is the
    /// default for every server this process builds; the headless host's
    /// `settings.json` and `PETRAMOND_ONLINE_MODE` can turn it off for a private
    /// server, and the in-process test harness constructs servers Offline.
    pub(in crate::server) account_policy: crate::account::AccountPolicy,
    /// What joining clients are told this server consents to (their own
    /// presentation packs). On unless its settings say not.
    pub(in crate::server) client_policy: crate::net::protocol::ClientPolicy,
    /// The opaque id this run advertises in `HelloAck`, and presents again when
    /// it redeems a joining client's ticket. Fresh per server (see
    /// [`crate::account::new_server_id`]).
    pub(in crate::server) server_id: String,
    /// Player identities promoted through `op`. Persisted in the world's
    /// engine KV map; the listen server's local session is always an
    /// operator independently of this set.
    pub(in crate::server) operators: crate::server::permissions::Operators,
    /// Which identity goes by which display name on this world (the one
    /// name → identity map; see `server::accounts`).
    pub(in crate::server) accounts: crate::server::accounts::PlayerRegistry,
    /// Loaded recipes and the unlock index derived from them.
    pub(in crate::server) catalog: RecipeCatalog,
    /// The WASM mods, the event bus and the tick-stage systems.
    pub(in crate::server) mods: ModRuntime,
    /// Who holds each container open (players' screens, mobs' holds).
    pub(in crate::server) containers: ContainerViewers,
    /// Chat lines accepted since the last pump.
    pub(in crate::server) chat: ChatService,
    /// Tick debt, the pause gate and the autosave timer.
    pub(in crate::server) clock: FrameClock,
    /// Replication bookkeeping shared by every recipient.
    pub(in crate::server) broadcast: Broadcast,
    /// Memo for the hostile-spawn plan's player/terrain half (see
    /// [`crate::mob::HostileSpawnCache`]) — the planner runs every tick, its
    /// chunk-neighbourhood scans do not.
    pub(in crate::server) hostile_spawn_cache: crate::mob::HostileSpawnCache,
    /// The seed sequence for scattered drops and rolls.
    pub(in crate::server) seeds: DropSeeds,
    /// Reused buffers for every session's exposure tick.
    pub(in crate::server) exposure_scratch: crate::exposure::ExposureScratch,
}

/// The pieces a freshly built server starts from (see
/// [`crate::server::session_build`]).
pub(in crate::server) struct ServerParts {
    pub world: ServerWorld,
    pub local: Option<ConnectedPlayer>,
    pub operators: crate::server::permissions::Operators,
    pub accounts: crate::server::accounts::PlayerRegistry,
    pub catalog: RecipeCatalog,
    pub mods: crate::modding::ModHost,
    /// The shared job pool admissions restore joining players on.
    pub jobs: std::sync::Arc<crate::worker::JobPool>,
    pub account_policy: crate::account::AccountPolicy,
}

impl ServerGame {
    /// Assemble a server around its world and (on a listen server) the local
    /// session. A headless server's pause gate starts open: remote players
    /// may exist from boot.
    pub(in crate::server) fn assemble(parts: ServerParts) -> Self {
        let remote_from_boot = parts.local.is_none();
        Self {
            world: parts.world,
            sessions: SessionRegistry::new(parts.local),
            admissions: Admissions::new(parts.jobs),
            account_policy: parts.account_policy,
            client_policy: Default::default(),
            server_id: crate::account::new_server_id(),
            operators: parts.operators,
            accounts: parts.accounts,
            catalog: parts.catalog,
            mods: ModRuntime::new(parts.mods),
            containers: ContainerViewers::default(),
            chat: ChatService::default(),
            clock: FrameClock::new(remote_from_boot),
            broadcast: Broadcast::default(),
            hostile_spawn_cache: Default::default(),
            seeds: DropSeeds::default(),
            exposure_scratch: Default::default(),
        }
    }

    /// The authoritative world (read-only).
    pub fn world(&self) -> &ServerWorld {
        &self.world
    }

    /// The authoritative world, for fixtures that stage terrain and entities
    /// directly.
    #[cfg(any(test, feature = "test-support"))]
    pub fn world_mut(&mut self) -> &mut ServerWorld {
        &mut self.world
    }

    /// The connected sessions (read-only).
    pub fn sessions(&self) -> &SessionRegistry {
        &self.sessions
    }

    /// The connected sessions, for fixtures that stage player state directly.
    #[cfg(any(test, feature = "test-support"))]
    pub fn sessions_mut(&mut self) -> &mut SessionRegistry {
        &mut self.sessions
    }

    /// The world and the sessions at once, for fixtures staging a session
    /// against world state (a menu opened on a placed block).
    #[cfg(any(test, feature = "test-support"))]
    pub fn world_and_sessions_mut(&mut self) -> (&mut ServerWorld, &mut SessionRegistry) {
        (&mut self.world, &mut self.sessions)
    }

    /// Cap concurrent players (a headless server's `max_players`): further
    /// joins are refused with `ServerFull`.
    pub fn set_max_players(&mut self, max_players: usize) {
        self.sessions.set_capacity(max_players);
    }

    /// Whether joining players must prove a Petramond account (a headless
    /// server's `online_mode`).
    pub fn set_account_policy(&mut self, policy: crate::account::AccountPolicy) {
        self.account_policy = policy;
    }

    pub fn account_policy(&self) -> crate::account::AccountPolicy {
        self.account_policy
    }

    /// What joining clients are told this server consents to about their own
    /// packs.
    pub fn set_client_policy(&mut self, policy: crate::net::protocol::ClientPolicy) {
        self.client_policy = policy;
    }

    /// How far from the nearest player mobs simulate (a headless server's
    /// `simulation_distance`).
    pub fn set_sim_distance(&mut self, distance: crate::mob::SimDistance) {
        self.world.mobs_mut().set_sim_distance(distance);
    }

    /// The loaded recipe catalog.
    pub fn recipes(&self) -> &Recipes {
        self.catalog.recipes()
    }

    /// The event bus, for registering engine-side handlers.
    pub fn bus_mut(&mut self) -> &mut EventBus {
        self.mods.bus_mut()
    }

    /// The tick-stage seams, for attaching engine-side systems.
    pub fn systems_mut(&mut self) -> &mut TickSystems {
        self.mods.systems_mut()
    }

    /// The loaded WASM mods.
    pub fn mod_host(&self) -> &crate::modding::ModHost {
        self.mods.host()
    }

    /// Swap the mod host (fixtures installing hand-built guests).
    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_mod_host_for_test(&mut self, host: crate::modding::ModHost) {
        self.mods.replace_host(host);
    }

    /// Every chest some player or mob holds open — the replicated set that
    /// drives every client's lids.
    pub fn open_chests(&self) -> Vec<IVec3> {
        self.containers.open_chests()
    }

    /// How many players and mobs hold the chest at `pos` open.
    pub fn chest_viewers(&self, pos: IVec3) -> u8 {
        self.containers.viewers(pos)
    }

    /// One more (or one fewer) viewer of the chest at `pos`, standing in for
    /// another player's screen.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_chest_viewed_for_test(
        &mut self,
        pos: IVec3,
        viewed: bool,
        events: &mut crate::events::tick::TickEvents,
    ) {
        if viewed {
            self.containers.add_viewer(pos, events);
        } else {
            self.containers.drop_viewer(pos, events);
        }
    }

    /// Singleplayer pause, exactly as the local client's `Pause` message
    /// requests it.
    #[cfg(any(test, feature = "test-support"))]
    pub fn set_paused_for_test(&mut self, paused: bool) {
        self.clock.request_pause(paused);
    }

    /// The server as it is once "Open to LAN" succeeded: unpaused, and the
    /// pause gate closed for good.
    #[cfg(any(test, feature = "test-support"))]
    pub fn open_to_lan_for_test(&mut self) {
        self.clock.open_to_lan();
    }

    /// Run `f` inside a mod dispatch context over the live server, acting
    /// for `actor` — fixtures driving host calls directly.
    #[cfg(any(test, feature = "test-support"))]
    pub fn dispatch_for_test<R>(
        &mut self,
        actor: Option<PlayerId>,
        feed: &mut crate::events::tick::TickEvents,
        f: impl FnOnce(&mut crate::events::SimCtx) -> R,
    ) -> R {
        let Self {
            world,
            sessions,
            mods,
            ..
        } = self;
        mods.dispatch(world, sessions, actor, feed, |_, ctx| f(ctx))
    }
}

/// The whole sim moves to the server thread at spawn ([`super::handle`]);
/// keep the bound loud so a non-`Send` field is caught at ITS introduction,
/// not at the thread boundary.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ServerGame>();
};
