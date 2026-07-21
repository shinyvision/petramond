//! Scoped read-only access to the client replica during a client-WASM
//! dispatch. This mirrors `scope`, but the published value is an immutable
//! world reference: client modules may sample explored presentation data and
//! can never obtain a simulation mutation surface.

use std::cell::{Cell, RefCell};

use crate::world::World;

thread_local! {
    static ACTIVE_WORLD: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
    /// The acting player's snapshot, published for the dynamic extent of a
    /// PREDICTION dispatch (`ClientModRuntime::predict_claim`) so a client
    /// predictor answers `PlayerState` from the same snapshot vocabulary as
    /// the server side. Absent outside prediction dispatches.
    static ACTIVE_ACTOR: RefCell<Option<mod_api::PlayerSnapshot>> = const { RefCell::new(None) };
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

/// Publish the acting player's snapshot for the duration of `f` (nested
/// around the world scope by the prediction dispatch).
pub(in crate::modding) fn enter_actor<R>(actor: mod_api::PlayerSnapshot, f: impl FnOnce() -> R) -> R {
    struct RestoreActor(Option<mod_api::PlayerSnapshot>);
    impl Drop for RestoreActor {
        fn drop(&mut self) {
            ACTIVE_ACTOR.with(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let prev = ACTIVE_ACTOR.with(|slot| slot.borrow_mut().replace(actor));
    let _restore = RestoreActor(prev);
    f()
}

/// The published actor snapshot, if a prediction dispatch is live.
pub(in crate::modding) fn active_actor() -> Option<mod_api::PlayerSnapshot> {
    ACTIVE_ACTOR.with(|slot| slot.borrow().clone())
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
