//! The event bus: pre-event dispatch (synchronous, cancellable) and the post-event
//! queue (FIFO, drained at tick-stage boundaries).
//!
//! Determinism: handlers live in plain vectors kept sorted by `(priority
//! ascending, registration order)` via sorted insertion — dispatch never iterates
//! a map. Registration order will be engine first, then mods in load order.

use std::cell::RefCell;
use std::collections::VecDeque;

use crate::events::tick::TickEvents;
use crate::player::Player;
use crate::player::PlayerId;
use crate::world::World;

use super::payload::{
    AttackAttempt, BlockBreakPre, BlockPlacePre, DeferredAction, InteractAttempt, ItemUsePre,
    MobDamagePre, PlayerDamagePre, PostEvent, PostEventKind,
};

/// A pre handler's verdict. The first `Cancel` wins AND ends the dispatch:
/// handlers after it never run (2026-07-17 — closing the double-act gap:
/// nothing could tell a later mutating handler the click was already
/// consumed, so a `consume_held`/`set_block` handler could act on an event
/// another mod had cancelled). A consumed action is consumed; to observe one,
/// listen on the POST surface's `item_used`, which fires even for a cancelled
/// `item_use_pre` (cancel = consumed = used). Other posts (`player_damaged`,
/// `block_broken`) fire only when the action actually happened.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Cancel,
}

/// What handlers and attached systems see of the simulation: the split borrows
/// available at every dispatch site. `feed` is the lossy tick→presentation
/// channel ([`TickEvents`]) — the bus feeds it, never the other way around.
/// `queue` lets a handler enqueue follow-up post events. Seeded RNG streams and
/// the tick counter are reachable through `world`; WASM mods use their dedicated
/// per-mod `RngU64` host streams instead.
///
/// `player`/`gui_state` are the ACTING session's — a derived convenience, not
/// the whole roster. Player-plural code uses the sessions-view accessors
/// ([`acting_player_id`]/[`with_player`]), which reach EVERY
/// connected session's player wherever the dispatch site published the roster
/// (`ServerGame::with_sessions_view` — the tick-stage seams and the migrated
/// pre-event sites). The direct fields stay because the mod ABI's player
/// surface is per-acting-session and the WASM host reads them.
///
/// [`acting_player_id`]: Self::acting_player_id
/// [`with_player`]: Self::with_player
pub struct SimCtx<'a> {
    pub world: &'a mut World,
    pub player: &'a mut Player,
    /// The ACTING session's mod-GUI state map (one map per player session):
    /// `GuiStateSet/Get` HostCalls read/write it here.
    pub gui_state: &'a mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
    pub feed: &'a mut TickEvents,
    pub queue: &'a mut PostQueue,
}

/// One NON-acting session lent into [`with_sessions_scope`] — its stable id,
/// its authoritative player, and the two things a per-player GUI write needs:
/// that session's own state map and what it currently has open.
pub struct SessionPlayerRef<'a> {
    pub id: PlayerId,
    pub player: &'a mut Player,
    /// This session's mod-GUI state map. Lent alongside the player because a
    /// tick system's gauges belong to whoever is LOOKING, and the acting
    /// session (host, session 0) is nobody in particular on a server.
    pub gui_state: &'a mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
    /// The open GUI session: kind, and the cell it was opened on. `None` =
    /// nothing open (or a non-mod screen).
    pub gui: Option<OpenGui>,
}

/// One session's open GUI, as the roster publishes it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpenGui {
    pub kind: petramond_world::gui_state::GuiKind,
    pub anchor: Option<petramond_math::math::IVec3>,
}

struct ScopeEntry {
    id: PlayerId,
    player: *mut Player,
    gui_state: *mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
    gui: Option<OpenGui>,
}

/// The published sessions roster: the acting session's identity plus raw
/// handles to every OTHER session's player (the acting player deliberately
/// never appears here — it is exactly the `&mut` lent into the live
/// [`SimCtx`], and [`SimCtx::with_player`] routes its id through that borrow
/// so two paths to one player can never exist).
struct ScopeData {
    acting: PlayerId,
    /// What the ACTING session has open — its map is the live `SimCtx` borrow,
    /// so only this half of it can ride the roster.
    acting_gui: Option<OpenGui>,
    others: Vec<ScopeEntry>,
}

thread_local! {
    /// The scoped sessions roster, mirroring `modding::scope`: dispatch sites
    /// publish it around the region where a `SimCtx` is live, because the
    /// bus/scheduler signatures (and the `SimCtx` field set) are part of the
    /// frozen mod-facing surface and cannot thread it as a parameter.
    static SESSIONS_SCOPE: RefCell<Option<ScopeData>> = const { RefCell::new(None) };
}

/// Publish the sessions roster for the duration of `f`, then restore whatever
/// was published before (nesting-safe, panic-safe).
///
/// Soundness contract for the publisher (see `ServerGame::with_sessions_view`,
/// the one production caller): `others` must NOT include the session whose
/// player is lent into the `SimCtx`(s) built inside `f`, the referenced
/// players must not be reachable through any other live path while `f` runs,
/// and the underlying storage must stay untouched for the whole call. The
/// borrows in `others` prove validity at entry; [`SimCtx::with_player`]'s
/// deref relies on this contract for the rest.
pub fn with_sessions_scope<R>(
    acting: PlayerId,
    acting_gui: Option<OpenGui>,
    others: Vec<SessionPlayerRef<'_>>,
    f: impl FnOnce() -> R,
) -> R {
    struct Restore {
        prev: Option<ScopeData>,
    }
    impl Drop for Restore {
        fn drop(&mut self) {
            SESSIONS_SCOPE.with(|s| *s.borrow_mut() = self.prev.take());
        }
    }
    let entries = others
        .into_iter()
        .map(|o| ScopeEntry {
            id: o.id,
            player: o.player as *mut Player,
            gui_state: o.gui_state as *mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
            gui: o.gui,
        })
        .collect();
    let prev = SESSIONS_SCOPE.with(|s| {
        s.borrow_mut().replace(ScopeData {
            acting,
            acting_gui,
            others: entries,
        })
    });
    let _restore = Restore { prev };
    f()
}

impl SimCtx<'_> {
    /// The id of the ACTING session — whose `player`/`gui_state` this context
    /// carries. `None` when the dispatch site published no roster (mod init,
    /// unit fixtures, not-yet-migrated pre-event sites): the context is then
    /// single-session and anonymous, exactly the pre-roster behaviour.
    pub fn acting_player_id(&self) -> Option<PlayerId> {
        SESSIONS_SCOPE.with(|s| s.borrow().as_ref().map(|d| d.acting))
    }

    /// Lend session `id`'s authoritative player to `f`. The acting session's
    /// id resolves to `self.player` (the one live borrow); any other
    /// connected session resolves through the published roster. `None` = no
    /// such session, or no roster published here.
    pub fn with_player<R>(&mut self, id: PlayerId, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        enum Hit {
            Acting,
            Other(*mut Player),
        }
        let hit = SESSIONS_SCOPE.with(|s| {
            let scope = s.borrow();
            let d = scope.as_ref()?;
            if d.acting == id {
                return Some(Hit::Acting);
            }
            d.others
                .iter()
                .find(|e| e.id == id)
                .map(|e| Hit::Other(e.player))
        })?;
        match hit {
            Hit::Acting => Some(f(self.player)),
            // SAFETY: the pointer was published by `with_sessions_scope` from
            // a live `&mut` to a session OTHER than the acting one (the
            // publisher's contract), so it cannot alias `self.player`; the
            // publisher's split borrows keep it valid and exclusive for the
            // scope's extent, and this method's `&mut self` receiver plus the
            // module-private scope internals mean no second path can lend the
            // same player while `f` runs.
            Hit::Other(ptr) => Some(f(unsafe { &mut *ptr })),
        }
    }

    /// Lend session `id`'s MOD-GUI state map to `f`, the same routing
    /// [`with_player`](Self::with_player) does: the acting session resolves to
    /// the one live borrow, any other through the roster. `None` = no such
    /// session (or no roster published).
    ///
    /// This is the seam under a per-player gauge. Without it a tick system
    /// could only write the ACTING session's map — and the acting session for
    /// a tick stage is the host, so a machine's readings reached exactly one
    /// player however many were standing at machines.
    pub fn with_gui_state<R>(
        &mut self,
        id: PlayerId,
        f: impl FnOnce(&mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>) -> R,
    ) -> Option<R> {
        enum Hit {
            Acting,
            Other(*mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>),
        }
        let hit = SESSIONS_SCOPE.with(|s| {
            let scope = s.borrow();
            let d = scope.as_ref()?;
            if d.acting == id {
                return Some(Hit::Acting);
            }
            d.others
                .iter()
                .find(|e| e.id == id)
                .map(|e| Hit::Other(e.gui_state))
        })?;
        match hit {
            Hit::Acting => Some(f(self.gui_state)),
            // SAFETY: as `with_player` — the pointer came from a live `&mut`
            // to a NON-acting session's map (so it cannot alias
            // `self.gui_state`), the publisher keeps the storage untouched for
            // the scope's extent, and the `&mut self` receiver plus the
            // module-private scope internals leave no second path to it.
            Hit::Other(ptr) => Some(f(unsafe { &mut *ptr })),
        }
    }

    /// Every connected session that currently has a GUI open, in session
    /// order: `(id, kind, anchor cell)`. Empty when no roster is published.
    pub fn gui_viewers(&self) -> Vec<(PlayerId, OpenGui)> {
        SESSIONS_SCOPE.with(|s| {
            let scope = s.borrow();
            let Some(d) = scope.as_ref() else {
                return Vec::new();
            };
            let mut out: Vec<(PlayerId, OpenGui)> = d
                .others
                .iter()
                .map(|e| (e.id, e.gui))
                .chain(std::iter::once((d.acting, d.acting_gui)))
                .filter_map(|(id, gui)| gui.map(|g| (id, g)))
                .collect();
            out.sort_by_key(|(id, _)| id.0);
            out
        })
    }
}

type PreFn<E> = Box<dyn FnMut(&mut SimCtx, &mut E) -> Outcome + Send>;
type PostFn = Box<dyn FnMut(&mut SimCtx, &PostEvent) + Send>;

struct PreHandler<E> {
    priority: i32,
    f: PreFn<E>,
}

struct PostHandler {
    priority: i32,
    f: PostFn,
}

/// The queued post events plus the listener mask that gates enqueueing: an event
/// kind nobody listens to is dropped at `emit`, so the gameplay hot paths neither
/// allocate nor queue while the bus is idle.
#[derive(Default)]
pub struct PostQueue {
    events: VecDeque<PostEvent>,
    /// Bit per [`PostEventKind`] with at least one registered handler.
    wanted: u32,
    /// Engine actions queued by mod HostCalls from inside a guest dispatch
    /// (see [`DeferredAction`]); `Game` drains them at its per-tick action points.
    /// Never gated: a queued action always applies.
    actions: Vec<DeferredAction>,
}

impl PostQueue {
    /// Queue `ev` for the next drain point; dropped if nothing listens for its kind.
    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        if self.wanted & (1 << ev.kind() as u32) != 0 {
            self.events.push_back(ev);
        }
    }

    /// Queue an engine action for `Game`'s next action drain point.
    #[inline]
    pub fn push_action(&mut self, action: DeferredAction) {
        self.actions.push(action);
    }

    /// Take the queued actions (FIFO). Actions queued while a taken batch is
    /// being applied land in the fresh vector, for the NEXT drain point — no
    /// recursion.
    #[inline]
    pub fn take_actions(&mut self) -> Vec<DeferredAction> {
        std::mem::take(&mut self.actions)
    }

    /// Whether any action is queued, so the per-stage fast path stays a read.
    #[inline]
    pub fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }

    /// Mark `kind` as listened-to without a bus (emission contract tests).
    #[cfg(test)]
    pub fn want_for_test(&mut self, kind: PostEventKind) {
        self.wanted |= 1 << kind as u32;
    }

    /// Drain the queued events (emission contract tests).
    #[cfg(test)]
    pub fn take_events_for_test(&mut self) -> Vec<PostEvent> {
        self.events.drain(..).collect()
    }

    #[inline]
    fn wants(&self, kind: PostEventKind) -> bool {
        self.wanted & (1 << kind as u32) != 0
    }
}

/// Handler registry + post-event queue, owned by `Game` alongside the world.
#[derive(Default)]
pub struct EventBus {
    pre_block_place: Vec<PreHandler<BlockPlacePre>>,
    pre_block_break: Vec<PreHandler<BlockBreakPre>>,
    pre_interact_attempt: Vec<PreHandler<InteractAttempt>>,
    pre_use_unclaimed: Vec<PreHandler<InteractAttempt>>,
    pre_attack_attempt: Vec<PreHandler<AttackAttempt>>,
    pre_item_use: Vec<PreHandler<ItemUsePre>>,
    pre_mob_damage: Vec<PreHandler<MobDamagePre>>,
    pre_player_damage: Vec<PreHandler<PlayerDamagePre>>,
    post: [Vec<PostHandler>; PostEventKind::COUNT],
    queue: PostQueue,
}

macro_rules! pre_events {
    ($(($on:ident, $dispatch:ident, $field:ident, $ty:ty)),* $(,)?) => {
        impl EventBus {
            $(
                /// Register a handler; runs in `(priority ascending, registration
                /// order)`.
                pub fn $on(
                    &mut self,
                    priority: i32,
                    f: impl FnMut(&mut SimCtx, &mut $ty) -> Outcome + Send + 'static,
                ) {
                    let at = self.$field.partition_point(|h| h.priority <= priority);
                    self.$field.insert(at, PreHandler { priority, f: Box::new(f) });
                }

                /// Dispatch inline at the decision site, in `(priority,
                /// registration)` order. The first `Cancel` ends the dispatch —
                /// later handlers never see a consumed event (see [`Outcome`]).
                /// `player`/`gui_state` are the ACTING session's.
                pub fn $dispatch(
                    &mut self,
                    world: &mut World,
                    player: &mut Player,
                    gui_state: &mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
                    feed: &mut TickEvents,
                    ev: &mut $ty,
                ) -> Outcome {
                    if self.$field.is_empty() {
                        return Outcome::Continue;
                    }
                    let Self { $field: handlers, queue, .. } = self;
                    for h in handlers.iter_mut() {
                        let mut ctx = SimCtx {
                            world: &mut *world,
                            player: &mut *player,
                            gui_state: &mut *gui_state,
                            feed: &mut *feed,
                            queue: &mut *queue,
                        };
                        if (h.f)(&mut ctx, ev) == Outcome::Cancel {
                            return Outcome::Cancel;
                        }
                    }
                    Outcome::Continue
                }
            )*
        }
    };
}

pre_events!(
    (
        on_block_place_pre,
        block_place_pre,
        pre_block_place,
        BlockPlacePre
    ),
    (
        on_block_break_pre,
        block_break_pre,
        pre_block_break,
        BlockBreakPre
    ),
    (
        on_interact_attempt,
        interact_attempt,
        pre_interact_attempt,
        InteractAttempt
    ),
    (
        on_use_unclaimed,
        use_unclaimed,
        pre_use_unclaimed,
        InteractAttempt
    ),
    (
        on_attack_attempt,
        attack_attempt,
        pre_attack_attempt,
        AttackAttempt
    ),
    (on_item_use_pre, item_use_pre, pre_item_use, ItemUsePre),
    (
        on_mob_damage_pre,
        mob_damage_pre,
        pre_mob_damage,
        MobDamagePre
    ),
    (
        on_player_damage_pre,
        player_damage_pre,
        pre_player_damage,
        PlayerDamagePre
    ),
);

impl EventBus {
    /// Register a post-event handler for `kind`; runs in `(priority ascending,
    /// registration order)` when the queue drains.
    pub fn on_post(
        &mut self,
        kind: PostEventKind,
        priority: i32,
        f: impl FnMut(&mut SimCtx, &PostEvent) + Send + 'static,
    ) {
        let list = &mut self.post[kind as usize];
        let at = list.partition_point(|h| h.priority <= priority);
        list.insert(
            at,
            PostHandler {
                priority,
                f: Box::new(f),
            },
        );
        self.queue.wanted |= 1 << kind as u32;
    }

    /// Queue a post event for the next drain point (dropped if nothing listens).
    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        self.queue.emit(ev);
    }

    /// Whether any handler listens for `kind` — gates optional producer-side work
    /// (e.g. the world's stream-event capture).
    #[inline]
    pub fn wants(&self, kind: PostEventKind) -> bool {
        self.queue.wants(kind)
    }

    /// The out-queue, for lending into a [`SimCtx`] built outside a bus dispatch
    /// (the tick-stage scheduler).
    #[inline]
    pub fn queue_mut(&mut self) -> &mut PostQueue {
        &mut self.queue
    }

    /// Whether any post event is queued — the caller-side fast path, so a
    /// drain site can skip its setup (publishing the sessions roster) on the
    /// common empty tick edge.
    #[inline]
    pub fn has_queued_posts(&self) -> bool {
        !self.queue.events.is_empty()
    }

    /// Drain the queued post events FIFO, running each event's handlers in order.
    /// Handlers may enqueue follow-ups, which run in the same drain after the
    /// already-queued events (no recursion). The bound stops a runaway handler
    /// cascade from hanging the tick: hitting it is a handler bug, and the
    /// remainder of the queue is dropped loudly.
    pub fn drain_post(
        &mut self,
        world: &mut World,
        player: &mut Player,
        gui_state: &mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
        feed: &mut TickEvents,
    ) {
        if self.queue.events.is_empty() {
            return;
        }
        const DRAIN_BOUND: usize = 4096;
        let Self { post, queue, .. } = self;
        let mut processed = 0usize;
        while let Some(ev) = queue.events.pop_front() {
            processed += 1;
            if processed > DRAIN_BOUND {
                log::error!(
                    "post-event drain exceeded {DRAIN_BOUND} events in one tick; dropping {}",
                    queue.events.len() + 1
                );
                queue.events.clear();
                break;
            }
            for h in post[ev.kind() as usize].iter_mut() {
                let mut ctx = SimCtx {
                    world: &mut *world,
                    player: &mut *player,
                    gui_state: &mut *gui_state,
                    feed: &mut *feed,
                    queue: &mut *queue,
                };
                (h.f)(&mut ctx, &ev);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::sync::{Arc, Mutex};

    use super::super::payload::*;
    use super::*;
    use petramond_math::math::{IVec3, Vec3};
    use petramond_world::block::Block;
    use petramond_world::item::ItemType;

    fn sim() -> (
        World,
        Player,
        std::sync::Arc<petramond_world::gui_state::GuiStateMap>,
        TickEvents,
    ) {
        (
            World::new(1, 1),
            Player::new(Vec3::new(0.0, 80.0, 0.0)),
            petramond_world::gui_state::empty_gui_state(),
            TickEvents::default(),
        )
    }

    /// The sessions view: a published roster lets a handler reach EVERY
    /// session's player by id — the acting one routed through the `SimCtx`'s
    /// own borrow (never a second path), the rest through the scope — while an
    /// unpublished context stays honestly single-session and anonymous.
    #[test]
    fn the_sessions_view_reaches_every_session_and_routes_the_acting_player() {
        use crate::player::PlayerId;

        let (mut world, mut acting, mut gui, mut feed) = sim();
        let mut other = Player::new(Vec3::new(4.0, 80.0, 0.0));
        let mut queue = PostQueue::default();

        // No roster published: anonymous single-session context.
        {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut acting,
                gui_state: &mut gui,
                feed: &mut feed,
                queue: &mut queue,
            };
            assert_eq!(ctx.acting_player_id(), None);
            assert!(ctx.with_player(PlayerId(0), |_| ()).is_none());
        }

        let mut other_gui = petramond_world::gui_state::empty_gui_state();
        let others = vec![SessionPlayerRef {
            id: PlayerId(0),
            player: &mut other,
            gui_state: &mut other_gui,
            gui: None,
        }];
        with_sessions_scope(PlayerId(1), None, others, || {
            let mut ctx = SimCtx {
                world: &mut world,
                player: &mut acting,
                gui_state: &mut gui,
                feed: &mut feed,
                queue: &mut queue,
            };
            assert_eq!(ctx.acting_player_id(), Some(PlayerId(1)));
            let touched = ctx.with_player(PlayerId(1), |p| {
                p.set_health(5);
                p.pos.x
            });
            assert_eq!(touched, Some(0.0), "the acting id lends ctx.player");
            let touched = ctx.with_player(PlayerId(0), |p| {
                p.set_health(3);
                p.pos.x
            });
            assert_eq!(touched, Some(4.0), "another id lends that session's player");
            assert!(ctx.with_player(PlayerId(9), |_| ()).is_none());
        });
        // The mutations landed on the real players, and the scope is gone.
        assert_eq!(acting.health(), 5);
        assert_eq!(other.health(), 3);
        let ctx = SimCtx {
            world: &mut world,
            player: &mut acting,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        assert_eq!(ctx.acting_player_id(), None, "restored after the guard");
    }

    #[test]
    fn pre_handlers_run_in_priority_then_registration_order() {
        let (mut world, mut player, mut gui, mut feed) = sim();
        let mut bus = EventBus::default();
        let order = Arc::new(Mutex::new(Vec::new()));
        // Two handlers share priority 10: they must keep registration order.
        for (label, priority) in [("a", 10), ("b", 0), ("c", 10), ("d", -5)] {
            let order = order.clone();
            bus.on_item_use_pre(priority, move |_, _| {
                order.lock().unwrap().push(label);
                Outcome::Continue
            });
        }
        let mut ev = ItemUsePre {
            item: ItemType::Dirt,
            target: None,
        };
        let out = bus.item_use_pre(&mut world, &mut player, &mut gui, &mut feed, &mut ev);
        assert_eq!(out, Outcome::Continue);
        assert_eq!(*order.lock().unwrap(), vec!["d", "b", "a", "c"]);
    }

    /// The first `Cancel` ends the dispatch: a later handler never sees the
    /// consumed event, so a mutating handler (`consume_held`, `set_block`)
    /// can never double-act on a click another mod already handled. Earlier
    /// payload mutations still land (the engine reads them back only when
    /// the event was NOT cancelled anyway).
    #[test]
    fn the_first_cancel_ends_the_dispatch() {
        let (mut world, mut player, mut gui, mut feed) = sim();
        let mut bus = EventBus::default();
        let later_ran = Arc::new(AtomicI32::new(0));
        bus.on_player_damage_pre(0, |_, ev| {
            ev.amount = 3;
            Outcome::Cancel
        });
        {
            let ran = later_ran.clone();
            bus.on_player_damage_pre(1, move |_, _| {
                ran.fetch_add(1, Ordering::Relaxed);
                Outcome::Continue
            });
        }
        let mut ev = PlayerDamagePre {
            amount: 7,
            source: DamageSource::Fall,
            origin: None,
        };
        let out = bus.player_damage_pre(&mut world, &mut player, &mut gui, &mut feed, &mut ev);
        assert_eq!(out, Outcome::Cancel);
        assert_eq!(
            later_ran.load(Ordering::Relaxed),
            0,
            "a handler after the cancel must not run — the event is consumed"
        );
        assert_eq!(ev.amount, 3, "mutations before the cancel still land");
    }

    #[test]
    fn post_queue_drains_fifo_and_follow_ups_run_in_the_same_drain() {
        let (mut world, mut player, mut gui, mut feed) = sim();
        let mut bus = EventBus::default();
        let seen = Arc::new(Mutex::new(Vec::new()));
        {
            let seen = seen.clone();
            bus.on_post(PostEventKind::BlockPlaced, 0, move |ctx, ev| {
                let PostEvent::BlockPlaced { pos, .. } = ev else {
                    unreachable!()
                };
                seen.lock().unwrap().push(("placed", pos.x));
                if pos.x == 0 {
                    // Follow-ups queue behind everything already pending and
                    // still run within this drain (same tick).
                    ctx.queue.emit(PostEvent::PlayerDied);
                }
            });
        }
        {
            let seen = seen.clone();
            bus.on_post(PostEventKind::PlayerDied, 0, move |_, _| {
                seen.lock().unwrap().push(("died", -1));
            });
        }
        bus.emit(PostEvent::BlockPlaced {
            pos: IVec3::new(0, 0, 0),
            block: Block::Stone,
        });
        bus.emit(PostEvent::BlockPlaced {
            pos: IVec3::new(1, 0, 0),
            block: Block::Stone,
        });
        bus.drain_post(&mut world, &mut player, &mut gui, &mut feed);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("placed", 0), ("placed", 1), ("died", -1)]
        );
    }
}
