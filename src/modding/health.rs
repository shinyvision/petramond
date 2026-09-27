//! Mod health: ONE disabled flag per mod per world session, shared by every
//! instance of that mod.
//!
//! A mod runs in several wasm instances at once — the tick instance, one
//! worldgen instance per worker thread, the shape-bake dispatches on the
//! server — and each used to carry its own kill switch. A trap on worker A
//! then left workers B..N generating the mod's features (seams that depended
//! on which thread ran), and a failed server bake left clients baking boxes
//! the server no longer had. Now every instance of a session's mod holds the
//! same [`ModHealth`]: the first failure anywhere flips it, and every thread
//! observes the flip before its next dispatch.
//!
//! The session's [`ModHealthBoard`] also keeps the ORDER mods were disabled
//! in, so the server can tell each client the untold suffix
//! ([`ModHealthBoard::disabled_since`]) and the client instances of those
//! mods fall back together with the server's.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One mod's shared health within one session.
pub(crate) struct ModHealth {
    id: String,
    disabled: AtomicBool,
    fuel_warned: AtomicBool,
    /// The session's disablement log this flag reports into.
    log: Arc<Mutex<Vec<String>>>,
}

impl ModHealth {
    /// A health reporting into a log of its own — an instance outside any
    /// session board (test fixtures, the client's per-mod instances).
    pub(crate) fn standalone(id: &str) -> Arc<Self> {
        Arc::new(Self::new(id, Arc::default()))
    }

    fn new(id: &str, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id: id.to_owned(),
            disabled: AtomicBool::new(false),
            fuel_warned: AtomicBool::new(false),
            log,
        }
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    /// The threshold is diagnostic across all instances of this session mod.
    pub(crate) fn warn_fuel_once(&self, why: &str) -> bool {
        if !self.fuel_warned.swap(true, Ordering::AcqRel) {
            log::warn!("mod '{}': {why}; continuing", self.id);
            true
        } else {
            false
        }
    }

    /// Disable the mod for the rest of the session, on every thread. The
    /// first caller logs the one visible error line and records the mod in
    /// the session's disablement log; later callers (other threads failing
    /// on the same cause) are no-ops. Returns whether this call flipped it.
    pub(crate) fn disable(&self, why: &str) -> bool {
        if self.disabled.swap(true, Ordering::AcqRel) {
            return false;
        }
        log::error!("mod '{}' disabled for this session: {why}", self.id);
        self.log.lock().unwrap().push(self.id.clone());
        true
    }
}

/// Every mod's health for one world session, plus the order they were
/// disabled in. Cheap to clone (shared).
#[derive(Clone, Default)]
pub(crate) struct ModHealthBoard {
    mods: Arc<Mutex<Vec<Arc<ModHealth>>>>,
    log: Arc<Mutex<Vec<String>>>,
}

impl ModHealthBoard {
    /// The shared health of mod `id` (created healthy on first request).
    pub(crate) fn health(&self, id: &str) -> Arc<ModHealth> {
        let mut mods = self.mods.lock().unwrap();
        if let Some(health) = mods.iter().find(|h| h.id == id) {
            return Arc::clone(health);
        }
        let health = Arc::new(ModHealth::new(id, Arc::clone(&self.log)));
        mods.push(Arc::clone(&health));
        health
    }

    /// Mods disabled so far this session, in disable order, after the first
    /// `seen` — the suffix a recipient that has heard `seen` of them is
    /// missing. Append-only, so a counter is the whole bookkeeping.
    pub(crate) fn disabled_since(&self, seen: usize) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .get(seen..)
            .map_or_else(Vec::new, <[String]>::to_vec)
    }

    /// How many mods this session has disabled.
    pub(crate) fn disabled_count(&self) -> usize {
        self.log.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_flag_per_mod_shared_by_every_holder() {
        let board = ModHealthBoard::default();
        let a = board.health("alpha");
        let a_again = board.health("alpha");
        let b = board.health("beta");
        assert!(Arc::ptr_eq(&a, &a_again));
        assert!(a.disable("trap on worker 1"));
        assert!(a_again.is_disabled(), "every holder observes the flip");
        assert!(!a_again.disable("the same trap on worker 2"), "flips once");
        assert!(!b.is_disabled(), "other mods are unaffected");
        assert_eq!(board.disabled_since(0), vec!["alpha".to_owned()]);
    }

    #[test]
    fn the_disable_log_is_an_ordered_suffix() {
        let board = ModHealthBoard::default();
        board.health("beta").disable("x");
        board.health("alpha").disable("y");
        board.health("beta").disable("again");
        assert_eq!(board.disabled_count(), 2);
        assert_eq!(board.disabled_since(1), vec!["alpha".to_owned()]);
        assert!(board.disabled_since(2).is_empty());
        assert!(
            board.disabled_since(7).is_empty(),
            "a stale counter is harmless"
        );
    }

    #[test]
    fn a_standalone_health_is_its_own() {
        let board = ModHealthBoard::default();
        let lone = ModHealth::standalone("alpha");
        lone.disable("fixture");
        assert!(!board.health("alpha").is_disabled());
        assert_eq!(board.disabled_count(), 0);
    }

    #[test]
    fn fuel_warning_is_shared_across_instances_for_one_session() {
        let board = ModHealthBoard::default();
        assert!(board
            .health("alpha")
            .warn_fuel_once("first costly dispatch"));
        assert!(!board
            .health("alpha")
            .warn_fuel_once("another costly dispatch"));
        assert!(board.health("beta").warn_fuel_once("independent mod"));
        assert!(!board.health("alpha").is_disabled());
    }
}
