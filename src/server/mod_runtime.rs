use crate::events::tick::TickEvents;
use crate::events::{Attach, EventBus, PlayerRoster, PostEvent, SimCtx, TickSystems};
use crate::modding::ModHost;
use crate::player::PlayerId;
use crate::world::ServerWorld;

pub struct ModRuntime {
    bus: EventBus,
    systems: TickSystems,
    host: ModHost,
    next_spatial_sound_handle: u64,
}

impl ModRuntime {
    pub fn new(host: ModHost) -> Self {
        Self {
            bus: EventBus::default(),
            systems: TickSystems::default(),
            host,
            next_spatial_sound_handle: 1,
        }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    pub fn bus_mut(&mut self) -> &mut EventBus {
        &mut self.bus
    }

    pub fn systems_mut(&mut self) -> &mut TickSystems {
        &mut self.systems
    }

    pub fn host(&self) -> &ModHost {
        &self.host
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_host(&mut self, host: ModHost) {
        self.host = host;
    }

    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        self.bus.emit(ev);
    }

    pub fn initialize(&mut self, world: &mut ServerWorld) {
        let Self {
            bus,
            systems,
            host,
            next_spatial_sound_handle,
        } = self;
        host.initialize(world, bus, systems, next_spatial_sound_handle);
    }

    pub fn open_feed(&self) -> TickEvents {
        TickEvents::with_next_spatial_sound_handle(self.next_spatial_sound_handle)
    }

    pub fn settle_feed(&mut self, feed: &TickEvents) {
        self.next_spatial_sound_handle = feed.next_spatial_sound_handle();
    }

    pub fn run_systems(
        &mut self,
        at: Attach,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
    ) {
        if self.systems.is_empty_at(at) {
            return;
        }
        let Self { bus, systems, .. } = self;
        systems.run(at, world, players, feed, bus.queue_mut());
    }

    pub fn drain_posts(
        &mut self,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
    ) {
        if self.bus.has_queued_posts() {
            self.bus.drain_post(world, players, feed);
        }
    }

    pub fn dispatch<R>(
        &mut self,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
        actor: Option<PlayerId>,
        feed: &mut TickEvents,
        f: impl FnOnce(&mut ModHost, &mut SimCtx) -> R,
    ) -> R {
        let Self { bus, host, .. } = self;
        let mut ctx = SimCtx {
            world,
            actor,
            players,
            feed,
            queue: bus.queue_mut(),
        };
        f(host, &mut ctx)
    }
}
