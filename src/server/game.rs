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

pub const MAX_TICKS_PER_FRAME: u32 = 4;

pub struct PumpOutput {
    pub msgs: Vec<ServerToClient>,
    pub remote: Vec<(PlayerId, Vec<ServerToClient>)>,
    pub kicked: Vec<(PlayerId, Option<String>)>,
}

pub struct SharedTickRows {
    tick: u64,
    clock: u64,
    recipients: Vec<entity_rows::RecipientEntities>,
    sleep_tally: SleepTally,
    open_chests: Vec<IVec3>,
    pub env: Option<Vec<(String, [f32; 4])>>,
    feeds: replication::WindowFeeds,
}

pub struct ServerGame {
    pub(in crate::server) world: ServerWorld,
    pub(in crate::server) sessions: SessionRegistry,
    pub(in crate::server) admissions: Admissions,
    pub(in crate::server) account_policy: crate::account::AccountPolicy,
    pub(in crate::server) client_policy: crate::net::protocol::ClientPolicy,
    pub(in crate::server) server_id: String,
    pub(in crate::server) operators: crate::server::permissions::Operators,
    pub(in crate::server) accounts: crate::server::accounts::PlayerRegistry,
    pub(in crate::server) catalog: RecipeCatalog,
    pub(in crate::server) mods: ModRuntime,
    pub(in crate::server) containers: ContainerViewers,
    pub(in crate::server) chat: ChatService,
    pub(in crate::server) clock: FrameClock,
    pub(in crate::server) broadcast: Broadcast,
    pub(in crate::server) hostile_spawn_cache: crate::mob::HostileSpawnCache,
    pub(in crate::server) seeds: DropSeeds,
    pub(in crate::server) exposure_scratch: crate::exposure::ExposureScratch,
}

pub(in crate::server) struct ServerParts {
    pub world: ServerWorld,
    pub local: Option<ConnectedPlayer>,
    pub operators: crate::server::permissions::Operators,
    pub accounts: crate::server::accounts::PlayerRegistry,
    pub catalog: RecipeCatalog,
    pub mods: crate::modding::ModHost,
    pub jobs: std::sync::Arc<crate::worker::JobPool>,
    pub account_policy: crate::account::AccountPolicy,
}

impl ServerGame {
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

    pub fn world(&self) -> &ServerWorld {
        &self.world
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn world_mut(&mut self) -> &mut ServerWorld {
        &mut self.world
    }

    pub fn sessions(&self) -> &SessionRegistry {
        &self.sessions
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn sessions_mut(&mut self) -> &mut SessionRegistry {
        &mut self.sessions
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn world_and_sessions_mut(&mut self) -> (&mut ServerWorld, &mut SessionRegistry) {
        (&mut self.world, &mut self.sessions)
    }

    pub fn set_max_players(&mut self, max_players: usize) {
        self.sessions.set_capacity(max_players);
    }

    pub fn set_account_policy(&mut self, policy: crate::account::AccountPolicy) {
        self.account_policy = policy;
    }

    pub fn account_policy(&self) -> crate::account::AccountPolicy {
        self.account_policy
    }

    pub fn set_client_policy(&mut self, policy: crate::net::protocol::ClientPolicy) {
        self.client_policy = policy;
    }

    pub fn set_sim_distance(&mut self, distance: crate::mob::SimDistance) {
        self.world.mobs_mut().set_sim_distance(distance);
    }

    pub fn recipes(&self) -> &Recipes {
        self.catalog.recipes()
    }

    pub fn bus_mut(&mut self) -> &mut EventBus {
        self.mods.bus_mut()
    }

    pub fn systems_mut(&mut self) -> &mut TickSystems {
        self.mods.systems_mut()
    }

    pub fn mod_host(&self) -> &crate::modding::ModHost {
        self.mods.host()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_mod_host_for_test(&mut self, host: crate::modding::ModHost) {
        self.mods.replace_host(host);
    }

    pub fn open_chests(&self) -> Vec<IVec3> {
        self.containers.open_chests()
    }

    pub fn chest_viewers(&self, pos: IVec3) -> u8 {
        self.containers.viewers(pos)
    }

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

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_paused_for_test(&mut self, paused: bool) {
        self.clock.request_pause(paused);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn open_to_lan_for_test(&mut self) {
        self.clock.open_to_lan();
    }

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

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<ServerGame>();
};
