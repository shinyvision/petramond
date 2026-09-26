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
    /// Whether the active dispatch is READ-ONLY: the published `SimCtx` may be
    /// queried but not mutated. Set for the `ShapePlacementPlan` dispatch, whose
    /// ABI promises read-only world access — a mutating host call errors instead
    /// of letting the guest edit the world it is being asked to validate against.
    static READ_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Restores the slot to `.0` when dropped (including on unwind).
struct Restore(*mut ());

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE_CTX.with(|c| c.set(self.0));
    }
}

/// Restores the read-only flag to `.0` when dropped (including on unwind).
struct RestoreReadOnly(bool);

impl Drop for RestoreReadOnly {
    fn drop(&mut self) {
        READ_ONLY.with(|c| c.set(self.0));
    }
}

/// Publish `ctx` as the active simulation context while `f` runs (the guest
/// dispatch). Nested `enter`s stack: the previous pointer is restored on exit.
pub(super) fn enter<R>(ctx: &mut SimCtx<'_>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_CTX.with(|c| c.replace(ctx as *mut SimCtx<'_> as *mut ()));
    let _restore = Restore(prev);
    f()
}

/// [`enter`] for a READ-ONLY dispatch: the context is lent out SHARED only.
/// [`with_active`] refuses for the whole dispatch, so no host handler can
/// obtain a `&mut SimCtx` — mutation is unreachable by construction, not by
/// each handler remembering a check. Only [`with_active_ref`] readers answer.
/// Used by the shape placement-plan dispatch.
pub(super) fn enter_read_only<R>(ctx: &mut SimCtx<'_>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_CTX.with(|c| c.replace(ctx as *mut SimCtx<'_> as *mut ()));
    let prev_ro = READ_ONLY.with(|c| c.replace(true));
    let _restore = Restore(prev);
    let _restore_ro = RestoreReadOnly(prev_ro);
    f()
}

/// Whether the active dispatch published a READ-ONLY context (mutating host
/// calls must refuse).
pub(super) fn read_only_active() -> bool {
    READ_ONLY.with(|c| c.get())
}

/// Take the published context pointer for one accessor call, restoring it
/// when the returned guard drops. Taking it means a nested accessor (or a
/// host call the closure itself triggers) sees "no context" instead of
/// aliasing the reference lent out.
fn take_active() -> Option<(*mut SimCtx<'static>, Restore)> {
    let ptr = ACTIVE_CTX.with(|c| c.replace(std::ptr::null_mut()));
    (!ptr.is_null()).then(|| (ptr as *mut SimCtx<'static>, Restore(ptr)))
}

/// Run `f` with EXCLUSIVE access to the active [`SimCtx`], or return `None`
/// when no guest dispatch is in flight on this thread (a host call outside
/// any [`enter`] scope) or the active dispatch is read-only.
pub(super) fn with_active<R>(f: impl FnOnce(&mut SimCtx<'_>) -> R) -> Option<R> {
    if read_only_active() {
        return None;
    }
    let (ptr, _restore) = take_active()?;
    // SAFETY: `ptr` was published by `enter` from a live `&mut SimCtx` whose
    // guard is still on this thread's stack (we are inside its dynamic
    // extent), and taking it made this the only path to it. The reference
    // handed to `f` cannot outlive `f`.
    let ctx = unsafe { &mut *ptr };
    Some(f(ctx))
}

/// Run `f` with SHARED access to the active [`SimCtx`] — the one accessor a
/// read-only dispatch answers. `None` when no guest dispatch is in flight.
pub(super) fn with_active_ref<R>(f: impl FnOnce(&SimCtx<'_>) -> R) -> Option<R> {
    let (ptr, _restore) = take_active()?;
    // SAFETY: as in `with_active`; the context is only read through the
    // shared reference, which cannot outlive `f`.
    let ctx = unsafe { &*ptr };
    Some(f(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, RosterRefs};
    use crate::world::ServerWorld;

    #[test]
    fn scope_is_bounded_and_reentrancy_safe() {
        let mut world = ServerWorld::new(1, 1);
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();

        assert!(with_active(|_| ()).is_none(), "no scope outside enter");
        let mut nobody = RosterRefs::empty();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
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

    #[test]
    fn a_read_only_scope_lends_shared_access_only() {
        let mut world = ServerWorld::new(1, 1);
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut nobody = RosterRefs::empty();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        enter_read_only(&mut ctx, || {
            assert!(read_only_active());
            assert!(with_active(|_| ()).is_none(), "no exclusive access at all");
            let tick = with_active_ref(|ctx| {
                assert!(with_active_ref(|_| ()).is_none(), "taken while in use");
                ctx.world.current_tick()
            });
            assert_eq!(tick, Some(0));
        });
        assert!(!read_only_active(), "the flag is restored after the guard");
        enter(&mut ctx, || {
            assert!(with_active(|_| ()).is_some(), "exclusive again outside");
            assert!(with_active_ref(|_| ()).is_some());
        });
    }
}
