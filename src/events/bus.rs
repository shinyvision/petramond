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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Continue,
    Cancel,
}

pub struct SimCtx<'a> {
    pub world: &'a mut ServerWorld,
    pub actor: Option<PlayerId>,
    pub players: &'a mut dyn PlayerRoster,
    pub feed: &'a mut TickEvents,
    pub queue: &'a mut PostQueue,
}

impl SimCtx<'_> {
    pub fn player_ids(&self) -> Vec<PlayerId> {
        (0..self.players.len())
            .map(|i| self.players.id_at(i))
            .collect()
    }

    pub fn session_index(&self, id: PlayerId) -> Option<usize> {
        self.players.index_of(id)
    }

    pub fn with_player<R>(&mut self, id: PlayerId, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        let index = self.players.index_of(id)?;
        Some(f(self.players.player_at(index)))
    }

    pub fn with_actor<R>(&mut self, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        let id = self.actor?;
        self.with_player(id, f)
    }

    pub fn with_gui_state<R>(
        &mut self,
        id: PlayerId,
        f: impl FnOnce(&mut std::sync::Arc<petramond_world::gui_state::GuiStateMap>) -> R,
    ) -> Option<R> {
        let index = self.players.index_of(id)?;
        Some(f(self.players.gui_state_at(index)))
    }

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

/// How many handler hops an event may be from one emitted outside the bus. A handler loop
/// re-emits forever while a legitimate burst (a world join streaming thousands of sections) is
/// wide but shallow, so only depth is bounded, never the count.
const MAX_CASCADE_DEPTH: u8 = 8;

#[derive(Default)]
pub struct PostQueue {
    /// Each event with its cascade depth.
    events: VecDeque<(PostEvent, u8)>,
    /// The depth an event emitted now gets: 0 outside a drain, one past the event being handled
    /// inside one.
    emit_depth: u8,
    wanted: u32,
    actions: Vec<DeferredAction>,
}

impl PostQueue {
    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        if self.wanted & (1 << ev.kind() as u32) != 0 {
            self.events.push_back((ev, self.emit_depth));
        }
    }

    #[inline]
    pub fn push_action(&mut self, action: DeferredAction) {
        self.actions.push(action);
    }

    #[inline]
    pub fn take_actions(&mut self) -> Vec<DeferredAction> {
        std::mem::take(&mut self.actions)
    }

    #[inline]
    pub fn has_actions(&self) -> bool {
        !self.actions.is_empty()
    }

    #[cfg(test)]
    pub fn want_for_test(&mut self, kind: PostEventKind) {
        self.wanted |= 1 << kind as u32;
    }

    #[cfg(test)]
    pub fn take_events_for_test(&mut self) -> Vec<PostEvent> {
        self.events.drain(..).map(|(ev, _)| ev).collect()
    }

    #[inline]
    fn wants(&self, kind: PostEventKind) -> bool {
        self.wanted & (1 << kind as u32) != 0
    }
}

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
                pub fn $on(
                    &mut self,
                    priority: i32,
                    f: impl FnMut(&mut SimCtx, &mut $ty) -> Outcome + Send + 'static,
                ) {
                    let at = self.$field.partition_point(|h| h.priority <= priority);
                    self.$field.insert(at, PreHandler { priority, f: Box::new(f) });
                }

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

    #[inline]
    pub fn emit(&mut self, ev: PostEvent) {
        self.queue.emit(ev);
    }

    #[inline]
    pub fn wants(&self, kind: PostEventKind) -> bool {
        self.queue.wants(kind)
    }

    #[inline]
    pub fn queue_mut(&mut self) -> &mut PostQueue {
        &mut self.queue
    }

    #[inline]
    pub fn has_queued_posts(&self) -> bool {
        !self.queue.events.is_empty()
    }

    pub fn drain_post(
        &mut self,
        world: &mut ServerWorld,
        players: &mut dyn PlayerRoster,
        feed: &mut TickEvents,
    ) {
        if self.queue.events.is_empty() {
            return;
        }
        let Self { post, queue, .. } = self;
        let mut cut: Option<(PostEventKind, usize)> = None;
        while let Some((ev, depth)) = queue.events.pop_front() {
            if depth > MAX_CASCADE_DEPTH {
                let (_, n) = cut.get_or_insert((ev.kind(), 0));
                *n += 1;
                continue;
            }
            queue.emit_depth = depth + 1;
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
        queue.emit_depth = 0;
        if let Some((kind, n)) = cut {
            log::error!(
                "post handlers kept re-emitting past depth {MAX_CASCADE_DEPTH} \
                 (first cut: {kind:?}); dropped {n} events"
            );
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

    #[test]
    fn a_wide_burst_is_delivered_whole_while_a_handler_loop_is_cut_at_depth() {
        let (mut world, mut feed) = sim();
        let mut bus = EventBus::default();
        let placed = Arc::new(AtomicI32::new(0));
        {
            let placed = placed.clone();
            bus.on_post(PostEventKind::BlockPlaced, 0, move |_, _| {
                placed.fetch_add(1, Ordering::Relaxed);
            });
        }
        let died = Arc::new(AtomicI32::new(0));
        {
            let died = died.clone();
            bus.on_post(PostEventKind::PlayerDied, 0, move |ctx, ev| {
                died.fetch_add(1, Ordering::Relaxed);
                ctx.queue.emit(ev.clone());
            });
        }
        for x in 0..20_000 {
            bus.emit(PostEvent::BlockPlaced {
                pos: IVec3::new(x, 0, 0),
                block: Block::Stone,
                player: None,
            });
        }
        bus.emit(PostEvent::PlayerDied {
            player: PlayerId(0),
        });
        bus.drain_post(&mut world, &mut RosterRefs::empty(), &mut feed);
        assert_eq!(placed.load(Ordering::Relaxed), 20_000);
        let looped = i32::from(MAX_CASCADE_DEPTH) + 1;
        assert_eq!(died.load(Ordering::Relaxed), looped);
        assert!(!bus.has_queued_posts());

        bus.emit(PostEvent::PlayerDied {
            player: PlayerId(0),
        });
        bus.drain_post(&mut world, &mut RosterRefs::empty(), &mut feed);
        assert_eq!(
            died.load(Ordering::Relaxed),
            2 * looped,
            "the next drain starts from depth 0 again"
        );
    }

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
