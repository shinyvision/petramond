//! The event bus: pre-event dispatch (synchronous, cancellable) and the post-event
//! queue (FIFO, drained at tick-stage boundaries).
//!
//! Determinism: handlers live in plain vectors kept sorted by `(priority
//! ascending, registration order)` via sorted insertion — dispatch never iterates
//! a map. Registration order will be engine first, then mods in load order.

use std::collections::VecDeque;

use crate::events::tick::TickEvents;
use crate::player::Player;
use crate::player::PlayerId;
use crate::world::ServerWorld;

use super::payload::{
    AttackAttempt, BlockBreakPre, BlockPlacePre, CellsEditPre, DeferredAction, InteractAttempt,
    ItemUsePre, MobDamagePre, PlayerDamagePre, PostEvent, PostEventKind, ProjectileHit,
};
use super::roster::{OpenGui, PlayerRoster};

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
/// Players are reached ONLY by id through `players` ([`with_player`],
/// [`with_gui_state`]). `actor` names the session whose action this dispatch
/// runs for — the clicking, eating, damaged or dying player — and is `None`
/// for a global dispatch (tick systems, block hooks, spawn picks, mod init,
/// a mob's action): there is no implicit "current player" to fall back on.
///
/// [`with_player`]: Self::with_player
/// [`with_gui_state`]: Self::with_gui_state
pub struct SimCtx<'a> {
    pub world: &'a mut ServerWorld,
    /// The session this dispatch acts for; `None` = actor-less.
    pub actor: Option<PlayerId>,
    /// Every connected session, passed explicitly by the dispatch site.
    pub players: &'a mut dyn PlayerRoster,
    pub feed: &'a mut TickEvents,
    pub queue: &'a mut PostQueue,
}

impl SimCtx<'_> {
    /// Every connected session's id, in roster order.
    pub fn player_ids(&self) -> Vec<PlayerId> {
        (0..self.players.len())
            .map(|i| self.players.id_at(i))
            .collect()
    }

    /// Session `id`'s roster index — where its per-player tick events live.
    /// `None` = no such session.
    pub fn session_index(&self, id: PlayerId) -> Option<usize> {
        self.players.index_of(id)
    }

    /// Lend session `id`'s authoritative player to `f`. `None` = no such
    /// session.
    pub fn with_player<R>(&mut self, id: PlayerId, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        let index = self.players.index_of(id)?;
        Some(f(self.players.player_at(index)))
    }

    /// [`with_player`](Self::with_player) for the ACTOR. `None` = an
    /// actor-less dispatch, or an actor no longer connected.
    pub fn with_actor<R>(&mut self, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        let id = self.actor?;
        self.with_player(id, f)
    }

    /// Lend session `id`'s MOD-GUI state map to `f`. `None` = no such
    /// session.
    ///
    /// This is the seam under a per-player gauge: a machine's readings belong
    /// to whoever is LOOKING, so a tick system writes each viewer's map by id
    /// rather than any one privileged session's.
    pub fn with_gui_state<R>(
        &mut self,
        id: PlayerId,
        f: impl FnOnce(&mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>) -> R,
    ) -> Option<R> {
        let index = self.players.index_of(id)?;
        Some(f(self.players.gui_state_at(index)))
    }

    /// Every connected session that currently has a GUI open, in id order:
    /// `(id, kind + anchor)`.
    pub fn gui_viewers(&self) -> Vec<(PlayerId, OpenGui)> {
        let mut out: Vec<(PlayerId, OpenGui)> = (0..self.players.len())
            .filter_map(|i| {
                self.players
                    .open_gui_at(i)
                    .map(|gui| (self.players.id_at(i), gui))
            })
            .collect();
        out.sort_by_key(|(id, _)| id.0);
        out
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
    pre_cells_edit: Vec<PreHandler<CellsEditPre>>,
    pre_interact_attempt: Vec<PreHandler<InteractAttempt>>,
    pre_use_unclaimed: Vec<PreHandler<InteractAttempt>>,
    pre_attack_attempt: Vec<PreHandler<AttackAttempt>>,
    pre_item_use: Vec<PreHandler<ItemUsePre>>,
    pre_mob_damage: Vec<PreHandler<MobDamagePre>>,
    pre_player_damage: Vec<PreHandler<PlayerDamagePre>>,
    pre_projectile_hit: Vec<PreHandler<ProjectileHit>>,
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
                /// `actor` is the session the event happens for (`None` for a
                /// mob's action).
                pub fn $dispatch(
                    &mut self,
                    world: &mut ServerWorld,
                    players: &mut dyn PlayerRoster,
                    actor: Option<PlayerId>,
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
                            actor,
                            players: &mut *players,
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
        on_cells_edit_pre,
        cells_edit_pre,
        pre_cells_edit,
        CellsEditPre
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
    (
        on_projectile_hit,
        projectile_hit,
        pre_projectile_hit,
        ProjectileHit
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
    /// drain site can skip its setup on the common empty tick edge.
    #[inline]
    pub fn has_queued_posts(&self) -> bool {
        !self.queue.events.is_empty()
    }

    /// Drain the queued post events FIFO, running each event's handlers in order.
    /// Handlers may enqueue follow-ups, which run in the same drain after the
    /// already-queued events (no recursion). The bound stops a runaway handler
    /// cascade from hanging the tick: hitting it is a handler bug, and the
    /// remainder of the queue is dropped loudly.
    ///
    /// Each event's handlers run with the event's own player as the actor
    /// ([`PostEvent::actor`]) — the damaged, dying, collecting or clicking
    /// session — and actor-less for world events.
    pub fn drain_post(
        &mut self,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
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
            let actor = ev.actor();
            for h in post[ev.kind() as usize].iter_mut() {
                let mut ctx = SimCtx {
                    world: &mut *world,
                    actor,
                    players: &mut *players,
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
    use super::super::roster::{RosterRefs, SessionPlayerRef};
    use super::*;
    use petramond_math::math::IVec3;
    use petramond_math::world_pos::WorldPos;
    use petramond_world::block::Block;
    use petramond_world::item::ItemType;

    fn sim() -> (ServerWorld, TickEvents) {
        (ServerWorld::new(1, 1), TickEvents::default())
    }

    /// The explicit roster: a handler reaches every session's player by id,
    /// the actor is whoever the dispatch site named, and an actor-less
    /// dispatch has no player to fall back on.
    #[test]
    fn a_context_reaches_players_only_by_id_through_its_roster() {
        let (mut world, mut feed) = sim();
        let mut a = Player::new(WorldPos::new(0.0, 80.0, 0.0));
        let mut b = Player::new(WorldPos::new(4.0, 80.0, 0.0));
        let mut a_gui = petramond_world::gui_state::empty_gui_state();
        let mut b_gui = petramond_world::gui_state::empty_gui_state();
        let mut queue = PostQueue::default();
        {
            let mut roster = RosterRefs::new(vec![
                SessionPlayerRef {
                    id: PlayerId(0),
                    player: &mut a,
                    gui_state: &mut a_gui,
                    gui: None,
                },
                SessionPlayerRef {
                    id: PlayerId(1),
                    player: &mut b,
                    gui_state: &mut b_gui,
                    gui: None,
                },
            ]);
            let mut ctx = SimCtx {
                world: &mut world,
                actor: Some(PlayerId(1)),
                players: &mut roster,
                feed: &mut feed,
                queue: &mut queue,
            };
            assert_eq!(ctx.with_actor(|p| p.pos.x), Some(4.0), "the named actor");
            assert_eq!(ctx.session_index(PlayerId(1)), Some(1));
            ctx.with_player(PlayerId(0), |p| p.set_health(3));
            assert!(ctx.with_player(PlayerId(9), |_| ()).is_none());

            ctx.actor = None;
            assert!(
                ctx.with_actor(|_| ()).is_none(),
                "an actor-less dispatch lends nobody"
            );
        }
        assert_eq!(a.health(), 3, "the write landed on the addressed player");
    }

    #[test]
    fn pre_handlers_run_in_priority_then_registration_order() {
        let (mut world, mut feed) = sim();
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
        let out = bus.item_use_pre(
            &mut world,
            &mut RosterRefs::empty(),
            None,
            &mut feed,
            &mut ev,
        );
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
        let (mut world, mut feed) = sim();
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
        let out = bus.player_damage_pre(
            &mut world,
            &mut RosterRefs::empty(),
            None,
            &mut feed,
            &mut ev,
        );
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
        let (mut world, mut feed) = sim();
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
                    ctx.queue.emit(PostEvent::PlayerDied {
                        player: PlayerId(0),
                    });
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
            player: None,
        });
        bus.emit(PostEvent::BlockPlaced {
            pos: IVec3::new(1, 0, 0),
            block: Block::Stone,
            player: None,
        });
        bus.drain_post(&mut world, &mut RosterRefs::empty(), &mut feed);
        assert_eq!(
            *seen.lock().unwrap(),
            vec![("placed", 0), ("placed", 1), ("died", -1)]
        );
    }

    /// A post handler acts for the event's OWN player: two deaths in one
    /// drain dispatch as two different actors, and a world event as none —
    /// never as whichever session happens to come first.
    #[test]
    fn post_handlers_act_for_the_events_own_player() {
        let (mut world, mut feed) = sim();
        let mut bus = EventBus::default();
        let actors = Arc::new(Mutex::new(Vec::new()));
        for kind in [PostEventKind::PlayerDied, PostEventKind::BlockPlaced] {
            let actors = actors.clone();
            bus.on_post(kind, 0, move |ctx, _| {
                actors.lock().unwrap().push(ctx.actor);
            });
        }
        bus.emit(PostEvent::PlayerDied {
            player: PlayerId(2),
        });
        bus.emit(PostEvent::BlockPlaced {
            pos: IVec3::ZERO,
            block: Block::Stone,
            player: None,
        });
        bus.emit(PostEvent::PlayerDied {
            player: PlayerId(5),
        });
        bus.drain_post(&mut world, &mut RosterRefs::empty(), &mut feed);
        assert_eq!(
            *actors.lock().unwrap(),
            vec![Some(PlayerId(2)), None, Some(PlayerId(5))]
        );
    }
}
