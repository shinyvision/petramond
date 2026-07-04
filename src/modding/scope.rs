//! Scoped thread-local access to the live [`SimCtx`] during a guest dispatch.
//!
//! The reentrancy problem: `host_dispatch` calls arrive from inside
//! `wasmtime` while the engine call site (an event-bus handler or tick-system
//! closure) is holding the `&mut SimCtx` split borrows. Wasmtime host functions
//! must be `Send + Sync + 'static`, so the context cannot be captured — it is
//! published for the DURATION of the guest call through a guard-based scoped
//! thread-local instead: a raw pointer that exists only inside [`enter`]'s
//! dynamic extent and is never stored beyond the guard's lifetime.
//!
//! Soundness: [`with_active`] TAKES the pointer for the duration of its
//! closure, so even a re-entrant host call could never manufacture a second
//! `&mut SimCtx` to the same context; the guard (a `Drop` type) restores the
//! previous value on unwind, so a trap/panic through the guest cannot leak a
//! dangling pointer into the slot.

use std::cell::Cell;

use crate::events::SimCtx;

thread_local! {
    static ACTIVE_CTX: Cell<*mut ()> = const { Cell::new(std::ptr::null_mut()) };
}

/// Restores the slot to `.0` when dropped (including on unwind).
struct Restore(*mut ());

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE_CTX.with(|c| c.set(self.0));
    }
}

/// Publish `ctx` as the active simulation context while `f` runs (the guest
/// dispatch). Nested `enter`s stack: the previous pointer is restored on exit.
pub(super) fn enter<R>(ctx: &mut SimCtx<'_>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_CTX.with(|c| c.replace(ctx as *mut SimCtx<'_> as *mut ()));
    let _restore = Restore(prev);
    f()
}

/// Run `f` with the active [`SimCtx`], or return `None` when no guest dispatch
/// is in flight on this thread (a host call outside any [`enter`] scope).
pub(super) fn with_active<R>(f: impl FnOnce(&mut SimCtx<'_>) -> R) -> Option<R> {
    // Take the pointer so a nested `with_active` (or a host call the closure
    // itself triggers) sees "no context" instead of aliasing this `&mut`.
    let ptr = ACTIVE_CTX.with(|c| c.replace(std::ptr::null_mut()));
    if ptr.is_null() {
        return None;
    }
    let _restore = Restore(ptr);
    // SAFETY: `ptr` was published by `enter` from a live `&mut SimCtx` whose
    // guard is still on this thread's stack (we are inside its dynamic
    // extent), and taking it above made this the only path to it. The
    // reference handed to `f` cannot outlive `f`.
    let ctx = unsafe { &mut *(ptr as *mut SimCtx<'_>) };
    Some(f(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::PostQueue;
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::player::Player;
    use crate::world::World;

    #[test]
    fn scope_is_bounded_and_reentrancy_safe() {
        let mut world = World::new(1, 1);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();

        assert!(with_active(|_| ()).is_none(), "no scope outside enter");
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            feed: &mut feed,
            queue: &mut queue,
        };
        enter(&mut ctx, || {
            let tick = with_active(|ctx| {
                // A nested lookup while the ctx is lent out must NOT alias it.
                assert!(with_active(|_| ()).is_none(), "taken while in use");
                ctx.world.current_tick()
            });
            assert_eq!(tick, Some(0));
            assert!(with_active(|_| ()).is_some(), "restored after use");
        });
        assert!(with_active(|_| ()).is_none(), "cleared after the guard");
    }
}
