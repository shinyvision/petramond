use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use petramond_world::pack_manifest::ResourceNeeds;

use super::watchdog::{declared_needs, Watchdog};

/// One mod's standing for the session, shared by every instance of it: whether it has been
/// disabled, and the watchdog that decides how much it may use.
pub(crate) struct ModHealth {
    id: String,
    disabled: AtomicBool,
    watchdog: Watchdog,
    log: Arc<Mutex<Vec<String>>>,
}

impl ModHealth {
    /// A health of its own with standard allowances, for fixtures that have no pack.
    pub(crate) fn standalone(id: &str) -> Arc<Self> {
        Arc::new(Self::new(id, &ResourceNeeds::default(), Arc::default()))
    }

    /// A health of its own with the allowances the mod's installed pack declares.
    pub(crate) fn for_pack(id: &str) -> Arc<Self> {
        Arc::new(Self::new(id, &declared_needs(id), Arc::default()))
    }

    fn new(id: &str, needs: &ResourceNeeds, log: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            id: id.to_owned(),
            disabled: AtomicBool::new(false),
            watchdog: Watchdog::new(needs),
            log,
        }
    }

    pub(crate) fn watchdog(&self) -> &Watchdog {
        &self.watchdog
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    pub(crate) fn disable(&self, why: &str) -> bool {
        if self.disabled.swap(true, Ordering::AcqRel) {
            return false;
        }
        log::error!("mod '{}' disabled for this session: {why}", self.id);
        self.log.lock().unwrap().push(self.id.clone());
        true
    }
}

#[derive(Clone, Default)]
pub(crate) struct ModHealthBoard {
    mods: Arc<Mutex<Vec<Arc<ModHealth>>>>,
    log: Arc<Mutex<Vec<String>>>,
}

impl ModHealthBoard {
    pub(crate) fn health(&self, id: &str) -> Arc<ModHealth> {
        let mut mods = self.mods.lock().unwrap();
        if let Some(health) = mods.iter().find(|h| h.id == id) {
            return Arc::clone(health);
        }
        let health = Arc::new(ModHealth::new(
            id,
            &declared_needs(id),
            Arc::clone(&self.log),
        ));
        mods.push(Arc::clone(&health));
        health
    }

    pub(crate) fn disabled_since(&self, seen: usize) -> Vec<String> {
        self.log
            .lock()
            .unwrap()
            .get(seen..)
            .map_or_else(Vec::new, <[String]>::to_vec)
    }

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
    fn one_mod_shares_one_watchdog_across_its_instances() {
        let board = ModHealthBoard::default();
        board.health("alpha").watchdog().grow_memory(1).unwrap();
        board.health("alpha").watchdog().release_memory(1);
        assert!(std::ptr::eq(
            board.health("alpha").watchdog(),
            board.health("alpha").watchdog()
        ));
        assert!(!std::ptr::eq(
            board.health("alpha").watchdog(),
            board.health("beta").watchdog()
        ));
    }
}
