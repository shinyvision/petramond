use std::cell::{Cell, RefCell};

use crate::world::ReplicaWorld;
use petramond_world::inventory::Inventory;

thread_local! {
    static ACTIVE_WORLD: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
    static ACTIVE_ACTOR: RefCell<Option<mod_api::PlayerSnapshot>> = const { RefCell::new(None) };
    static ACTIVE_INVENTORY: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

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

pub(in crate::modding) fn enter<R>(world: &ReplicaWorld, f: impl FnOnce() -> R) -> R {
    publish(&ACTIVE_WORLD, world, f)
}

pub(super) fn with_active<R>(f: impl FnOnce(&ReplicaWorld) -> R) -> Option<R> {
    unsafe { published(&ACTIVE_WORLD, f) }
}

pub(in crate::modding) fn enter_inventory<R>(inventory: &Inventory, f: impl FnOnce() -> R) -> R {
    publish(&ACTIVE_INVENTORY, inventory, f)
}

pub(super) fn with_inventory<R>(f: impl FnOnce(&Inventory) -> R) -> Option<R> {
    unsafe { published(&ACTIVE_INVENTORY, f) }
}

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

pub(in crate::modding) fn active_actor() -> Option<mod_api::PlayerSnapshot> {
    ACTIVE_ACTOR.with(|slot| slot.borrow().clone())
}
