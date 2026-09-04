//! Scoped read-only access to the client replica during a client-WASM
//! dispatch. This mirrors `scope`, but the published values are immutable
//! references: client modules may sample explored presentation data and
//! can never obtain a simulation mutation surface.

use std::cell::{Cell, RefCell};

use crate::world::World;
use petramond_world::inventory::Inventory;

thread_local! {
    static ACTIVE_WORLD: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
    /// The acting player's snapshot, published for the dynamic extent of a
    /// PREDICTION dispatch (`ClientModRuntime::predict_claim`) so a client
    /// predictor answers `PlayerState` from the same snapshot vocabulary as
    /// the server side. Absent outside prediction dispatches.
    static ACTIVE_ACTOR: RefCell<Option<mod_api::PlayerSnapshot>> = const { RefCell::new(None) };
    /// The local player's replicated inventory, published like the world
    /// for every dispatch a client mod may read it in (`PlayerInventory`).
    static ACTIVE_INVENTORY: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

/// Publish `value` in `slot` for the duration of `f`, restoring whatever
/// was there before (nesting-safe, panic-safe).
fn publish<T, R>(
    slot: &'static std::thread::LocalKey<Cell<*const ()>>,
    value: &T,
    f: impl FnOnce() -> R,
) -> R {
    struct Restore(&'static std::thread::LocalKey<Cell<*const ()>>, *const ());
    impl Drop for Restore {
        fn drop(&mut self) {
            self.0.with(|slot| slot.set(self.1));
        }
    }
    let prev = slot.with(|s| s.replace(value as *const T as *const ()));
    let _restore = Restore(slot, prev);
    f()
}

/// Read what `slot` publishes, if anything.
///
/// # Safety
/// `slot` must only ever be set by [`publish`] with a `&T` of this same `T`;
/// the reference is live for the dynamic extent of the publishing call and
/// immutable re-entry is sound.
unsafe fn published<T, R>(
    slot: &'static std::thread::LocalKey<Cell<*const ()>>,
    f: impl FnOnce(&T) -> R,
) -> Option<R> {
    let ptr = slot.with(|s| s.get());
    if ptr.is_null() {
        return None;
    }
    Some(f(unsafe { &*(ptr as *const T) }))
}

pub(in crate::modding) fn enter<R>(world: &World, f: impl FnOnce() -> R) -> R {
    publish(&ACTIVE_WORLD, world, f)
}

pub(super) fn with_active<R>(f: impl FnOnce(&World) -> R) -> Option<R> {
    // SAFETY: `ACTIVE_WORLD` is only ever published by `enter` with a `&World`.
    unsafe { published(&ACTIVE_WORLD, f) }
}

/// Publish the local player's inventory for the duration of `f`.
pub(in crate::modding) fn enter_inventory<R>(inventory: &Inventory, f: impl FnOnce() -> R) -> R {
    publish(&ACTIVE_INVENTORY, inventory, f)
}

pub(super) fn with_inventory<R>(f: impl FnOnce(&Inventory) -> R) -> Option<R> {
    // SAFETY: `ACTIVE_INVENTORY` is only ever published by `enter_inventory`
    // with an `&Inventory`.
    unsafe { published(&ACTIVE_INVENTORY, f) }
}

/// Publish the acting player's snapshot for the duration of `f` (nested
/// around the world scope by the prediction dispatch).
pub(in crate::modding) fn enter_actor<R>(
    actor: mod_api::PlayerSnapshot,
    f: impl FnOnce() -> R,
) -> R {
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
