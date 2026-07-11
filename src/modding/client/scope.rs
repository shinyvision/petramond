//! Scoped read-only access to the client replica during a client-WASM
//! dispatch. This mirrors `scope`, but the published value is an immutable
//! world reference: client modules may sample explored presentation data and
//! can never obtain a simulation mutation surface.

use std::cell::Cell;

use crate::world::World;

thread_local! {
    static ACTIVE_WORLD: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

struct Restore(*const ());

impl Drop for Restore {
    fn drop(&mut self) {
        ACTIVE_WORLD.with(|slot| slot.set(self.0));
    }
}

pub(in crate::modding) fn enter<R>(world: &World, f: impl FnOnce() -> R) -> R {
    let prev = ACTIVE_WORLD.with(|slot| slot.replace(world as *const World as *const ()));
    let _restore = Restore(prev);
    f()
}

pub(super) fn with_active<R>(f: impl FnOnce(&World) -> R) -> Option<R> {
    let ptr = ACTIVE_WORLD.with(|slot| slot.get());
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `enter` publishes a live immutable reference only for the
    // dynamic extent of `f`; immutable re-entry is safe.
    Some(f(unsafe { &*(ptr as *const World) }))
}
