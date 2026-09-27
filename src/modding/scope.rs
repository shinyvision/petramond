use std::cell::Cell;

use crate::events::SimCtx;

thread_local! {
    static ACTIVE_CTX: Cell<*mut ()> = const { Cell::new(std::ptr::null_mut()) };
    static READ_ONLY: Cell<bool> = const { Cell::new(false) };
}

struct Restore(*mut ());

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE_CTX.with(|c| c.set(self.0));
    }
}

struct RestoreReadOnly(bool);

impl Drop for RestoreReadOnly {
    fn drop(&mut self) {
        READ_ONLY.with(|c| c.set(self.0));
    }
}

pub(super) fn enter<R>(ctx: &mut SimCtx<'_>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_CTX.with(|c| c.replace(ctx as *mut SimCtx<'_> as *mut ()));
    let _restore = Restore(prev);
    f()
}

pub(super) fn enter_read_only<R>(ctx: &mut SimCtx<'_>, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_CTX.with(|c| c.replace(ctx as *mut SimCtx<'_> as *mut ()));
    let prev_ro = READ_ONLY.with(|c| c.replace(true));
    let _restore = Restore(prev);
    let _restore_ro = RestoreReadOnly(prev_ro);
    f()
}

pub(super) fn read_only_active() -> bool {
    READ_ONLY.with(|c| c.get())
}

fn take_active() -> Option<(*mut SimCtx<'static>, Restore)> {
    let ptr = ACTIVE_CTX.with(|c| c.replace(std::ptr::null_mut()));
    (!ptr.is_null()).then(|| (ptr as *mut SimCtx<'static>, Restore(ptr)))
}

pub(super) fn with_active<R>(f: impl FnOnce(&mut SimCtx<'_>) -> R) -> Option<R> {
    if read_only_active() {
        return None;
    }
    let (ptr, _restore) = take_active()?;
    let ctx = unsafe { &mut *ptr };
    Some(f(ctx))
}

pub(super) fn with_active_ref<R>(f: impl FnOnce(&SimCtx<'_>) -> R) -> Option<R> {
    let (ptr, _restore) = take_active()?;
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
