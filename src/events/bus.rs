//! The event bus: pre-event dispatch (synchronous, cancellable) and the post-event
//! queue (FIFO, drained at tick-stage boundaries).
//!
//! Determinism: handlers live in plain vectors kept sorted by `(priority
//! ascending, registration order)` via sorted insertion — dispatch never iterates
//! a map. Registration order will be engine first, then mods in load order.

use std::collections::VecDeque;

use crate::game::TickEvents;
use crate::player::Player;
use crate::world::World;

use super::payload::{
    BlockBreakPre, BlockInteract, BlockPlacePre, ItemUsePre, MobDamagePre, MobInteract, ModAction,
    PlayerDamagePre, PostEvent, PostEventKind,
};

/// A pre handler's verdict. The first `Cancel` wins; handlers after it still run
/// (observe-only — their verdict can no longer change the outcome).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Continue,
    Cancel,
}

/// What handlers and attached systems see of the simulation: the split borrows
/// available at every dispatch site. `feed` is the lossy tick→presentation
/// channel ([`TickEvents`]) — the bus feeds it, never the other way around.
/// `queue` lets a handler enqueue follow-up post events. Seeded RNG streams and
/// the tick counter are reachable through `world` until the Phase 3 host API
/// carries dedicated per-mod streams.
#[allow(dead_code)] // only handlers read the fields; none registered until Phase 2.
pub(crate) struct SimCtx<'a> {
    pub world: &'a mut World,
    pub player: &'a mut Player,
    /// The ACTING session's mod-GUI state map (per-session since multiplayer
    /// C2c-iii): `GuiStateSet/Get` HostCalls read/write it here.
    pub gui_state: &'a mut std::sync::Arc<crate::gui::GuiStateMap>,
    pub feed: &'a mut TickEvents,
    pub queue: &'a mut PostQueue,
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
pub(crate) struct PostQueue {
    events: VecDeque<PostEvent>,
    /// Bit per [`PostEventKind`] with at least one registered handler.
    wanted: u32,
    /// Engine actions queued by mod HostCalls from inside a guest dispatch
    /// (see [`ModAction`]); `Game` drains them at its per-tick action points.
    /// Never gated: a queued action always applies.
    actions: Vec<ModAction>,
}

impl PostQueue {
    /// Queue `ev` for the next drain point; dropped if nothing listens for its kind.
    #[inline]
    pub(crate) fn emit(&mut self, ev: PostEvent) {
        if self.wanted & (1 << ev.kind() as u32) != 0 {
            self.events.push_back(ev);
        }
    }

    /// Queue an engine action for `Game`'s next action drain point.
    #[inline]
    pub(crate) fn push_action(&mut self, action: ModAction) {
        self.actions.push(action);
    }

    /// Take the queued actions (FIFO). Actions queued while a taken batch is
    /// being applied land in the fresh vector, for the NEXT drain point — no
    /// recursion.
    #[inline]
    pub(crate) fn take_actions(&mut self) -> Vec<ModAction> {
        std::mem::take(&mut self.actions)
    }

    /// Whether any action is queued, so the per-stage fast path stays a read.
    #[inline]
    pub(crate) fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }

    #[inline]
    fn wants(&self, kind: PostEventKind) -> bool {
        self.wanted & (1 << kind as u32) != 0
    }
}

/// Handler registry + post-event queue, owned by `Game` alongside the world.
#[derive(Default)]
pub(crate) struct EventBus {
    pre_block_place: Vec<PreHandler<BlockPlacePre>>,
    pre_block_break: Vec<PreHandler<BlockBreakPre>>,
    pre_block_interact: Vec<PreHandler<BlockInteract>>,
    pre_item_use: Vec<PreHandler<ItemUsePre>>,
    pre_mob_interact: Vec<PreHandler<MobInteract>>,
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
                #[allow(dead_code)] // Phase 2+ mod surface; exercised by tests today.
                pub(crate) fn $on(
                    &mut self,
                    priority: i32,
                    f: impl FnMut(&mut SimCtx, &mut $ty) -> Outcome + Send + 'static,
                ) {
                    let at = self.$field.partition_point(|h| h.priority <= priority);
                    self.$field.insert(at, PreHandler { priority, f: Box::new(f) });
                }

                /// Dispatch inline at the decision site. Every handler runs — a
                /// cancelled event is still observed by later handlers — and the
                /// first `Cancel` wins. `player`/`gui_state` are the ACTING
                /// session's.
                pub(crate) fn $dispatch(
                    &mut self,
                    world: &mut World,
                    player: &mut Player,
                    gui_state: &mut std::sync::Arc<crate::gui::GuiStateMap>,
                    feed: &mut TickEvents,
                    ev: &mut $ty,
                ) -> Outcome {
                    if self.$field.is_empty() {
                        return Outcome::Continue;
                    }
                    let Self { $field: handlers, queue, .. } = self;
                    let mut out = Outcome::Continue;
                    for h in handlers.iter_mut() {
                        let mut ctx = SimCtx {
                            world: &mut *world,
                            player: &mut *player,
                            gui_state: &mut *gui_state,
                            feed: &mut *feed,
                            queue: &mut *queue,
                        };
                        let o = (h.f)(&mut ctx, ev);
                        if out == Outcome::Continue {
                            out = o;
                        }
                    }
                    out
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
        on_block_interact,
        block_interact,
        pre_block_interact,
        BlockInteract
    ),
    (on_item_use_pre, item_use_pre, pre_item_use, ItemUsePre),
    (on_mob_interact, mob_interact, pre_mob_interact, MobInteract),
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
    #[allow(dead_code)] // Phase 2+ mod surface; exercised by tests today.
    pub(crate) fn on_post(
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
    pub(crate) fn emit(&mut self, ev: PostEvent) {
        self.queue.emit(ev);
    }

    /// Whether any handler listens for `kind` — gates optional producer-side work
    /// (e.g. the world's stream-event capture).
    #[inline]
    pub(crate) fn wants(&self, kind: PostEventKind) -> bool {
        self.queue.wants(kind)
    }

    /// The out-queue, for lending into a [`SimCtx`] built outside a bus dispatch
    /// (the tick-stage scheduler).
    #[inline]
    pub(crate) fn queue_mut(&mut self) -> &mut PostQueue {
        &mut self.queue
    }

    /// Drain the queued post events FIFO, running each event's handlers in order.
    /// Handlers may enqueue follow-ups, which run in the same drain after the
    /// already-queued events (no recursion). The bound stops a runaway handler
    /// cascade from hanging the tick: hitting it is a handler bug, and the
    /// remainder of the queue is dropped loudly.
    pub(crate) fn drain_post(
        &mut self,
        world: &mut World,
        player: &mut Player,
        gui_state: &mut std::sync::Arc<crate::gui::GuiStateMap>,
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
    use crate::block::Block;
    use crate::item::ItemType;
    use crate::mathh::{IVec3, Vec3};

    fn sim() -> (
        World,
        Player,
        std::sync::Arc<crate::gui::GuiStateMap>,
        TickEvents,
    ) {
        (
            World::new(1, 1),
            Player::new(Vec3::new(0.0, 80.0, 0.0)),
            crate::gui::empty_gui_state(),
            TickEvents::default(),
        )
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

    #[test]
    fn first_cancel_wins_but_later_handlers_still_observe_the_payload() {
        let (mut world, mut player, mut gui, mut feed) = sim();
        let mut bus = EventBus::default();
        let seen_by_later = Arc::new(AtomicI32::new(0));
        bus.on_player_damage_pre(0, |_, ev| {
            ev.amount = 3;
            Outcome::Cancel
        });
        {
            let seen = seen_by_later.clone();
            bus.on_player_damage_pre(1, move |_, ev| {
                seen.store(ev.amount, Ordering::Relaxed);
                // A later verdict can't override the first Cancel.
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
            seen_by_later.load(Ordering::Relaxed),
            3,
            "handlers after a cancel still run and see the mutated payload"
        );
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
