//! The server's mod runtime: the WASM mod host plus the engine seams its
//! registrations attach to (the event bus and the tick-stage systems), and
//! the one place a server dispatch builds its [`SimCtx`].
//!
//! Every dispatch names its actor explicitly and lends the whole session
//! roster by reference — the runtime never picks a player itself.

use crate::events::tick::TickEvents;
use crate::events::{Attach, EventBus, PlayerRoster, PostEvent, SimCtx, TickSystems};
use crate::modding::ModHost;
use crate::player::PlayerId;
use crate::world::World;

/// The WASM mod instances and the seams they (and the engine) register on.
pub struct ModRuntime {
    /// Pre events dispatch at their decision sites; post events queue and
    /// drain at tick-stage boundaries. Engine handlers register before any
    /// mod's.
    bus: EventBus,
    /// Systems attached between the fixed-tick stages.
    systems: TickSystems,
    /// The WASM mod instances. Their registered closures (held by
    /// `bus`/`systems`) share ownership; the host keeps the canonical handles
    /// for GUI click dispatch, custom-shape bakes and diagnostics.
    host: ModHost,
    /// Next deterministic handle for spatial sounds. The app owns playback;
    /// this counter only gives mods stable identities for stop calls, so it
    /// carries across every feed the server opens.
    next_spatial_sound_handle: u64,
}

impl ModRuntime {
    /// A runtime around `host` with empty seams (engine handlers and
    /// [`initialize`](Self::initialize) register next).
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

    /// Swap the mod host (fixtures installing hand-built guests).
    #[cfg(any(test, feature = "test-support"))]
    pub fn replace_host(&mut self, host: ModHost) {
        self.host = host;
    }

    /// Queue a post event for the next drain point (dropped if nothing
    /// listens).
    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        self.bus.emit(ev);
    }

    /// Run every mod's `mod_init` (after the engine's own registrations, so
    /// mods sort behind the engine at equal priority). Init belongs to no
    /// player.
    pub fn initialize(&mut self, world: &mut World) {
        let Self {
            bus,
            systems,
            host,
            next_spatial_sound_handle,
        } = self;
        host.initialize(world, bus, systems, next_spatial_sound_handle);
    }

    /// A fresh tick→presentation feed, continuing the spatial-sound handle
    /// sequence. Hand it back through [`settle_feed`](Self::settle_feed).
    pub fn open_feed(&self) -> TickEvents {
        TickEvents::with_next_spatial_sound_handle(self.next_spatial_sound_handle)
    }

    /// Adopt the handles `feed` allocated, so the next feed never reuses one.
    pub fn settle_feed(&mut self, feed: &TickEvents) {
        self.next_spatial_sound_handle = feed.next_spatial_sound_handle();
    }

    /// Run the systems attached at `at`. A stage seam belongs to no player:
    /// systems run actor-less and reach every session by id. A slot with
    /// nothing attached costs one bounds-checked read.
    pub fn run_systems(
        &mut self,
        at: Attach,
        world: &mut World,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
    ) {
        if self.systems.is_empty_at(at) {
            return;
        }
        let Self { bus, systems, .. } = self;
        systems.run(at, world, players, feed, bus.queue_mut());
    }

    /// Drain the queued post events, each dispatched for its own player.
    pub fn drain_posts(
        &mut self,
        world: &mut World,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
    ) {
        if self.bus.has_queued_posts() {
            self.bus.drain_post(world, players, feed);
        }
    }

    /// Run `f` against the mod host with a [`SimCtx`] over `world` and
    /// `players`, acting for `actor` (`None` = an actor-less dispatch) — the
    /// one place a server-side mod dispatch builds its context.
    pub fn dispatch<R>(
        &mut self,
        world: &mut World,
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
