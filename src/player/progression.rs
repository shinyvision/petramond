//! What one player has discovered: every item kind they have ever held, and
//! every crafting recipe that has been unlocked for them.
//!
//! Both sets are pure PLAYER state — they persist in `players/<name>.dat`,
//! ride the join handshake, and are authoritative on the server. Nothing here
//! decides WHAT unlocks: that is a consequence some event handler draws (the
//! engine's ingredient-discovery rule in `server::progression`, or any mod's
//! `UnlockRecipe`). This type only owns the record and its transitions.
//!
//! `obtained` is the memory that makes `item_obtained` a once-per-kind event,
//! so no handler needs one of its own.

use std::collections::HashSet;

use petramond_world::item::{ItemSet, ItemType};

/// One player's discovery record.
#[derive(Clone, Default)]
pub struct Progression {
    /// Every item kind this player has ever held, by SESSION id (persisted as
    /// registry names — ids move between sessions).
    obtained: ItemSet,
    /// Unlocked recipe keys in UNLOCK ORDER. The order is load-bearing on the
    /// wire: unlocking only ever appends, so the owning client is caught up by
    /// shipping the suffix it has not seen (`server::game::replication`).
    unlocked: Vec<String>,
    /// Membership index over `unlocked` — the browser asks per recipe per
    /// rebuild, and the craft action asks on every request.
    index: HashSet<String>,
}

impl Progression {
    /// Every item kind held at least once.
    #[inline]
    pub fn obtained(&self) -> &ItemSet {
        &self.obtained
    }

    /// Record that the player now holds `item`. `true` = the FIRST time ever,
    /// which is exactly when `item_obtained` fires.
    #[inline]
    pub fn obtain(&mut self, item: ItemType) -> bool {
        item != ItemType::Air && self.obtained.insert(item)
    }

    #[inline]
    pub fn is_unlocked(&self, recipe: &str) -> bool {
        self.index.contains(recipe)
    }

    /// Unlock `recipe`. `true` = it was not unlocked before (the caller's cue
    /// that something changed: replication, feedback).
    pub fn unlock(&mut self, recipe: &str) -> bool {
        if self.index.contains(recipe) {
            return false;
        }
        self.index.insert(recipe.to_owned());
        self.unlocked.push(recipe.to_owned());
        true
    }

    /// Unlocked recipe keys in unlock order.
    #[inline]
    pub fn unlocked(&self) -> &[String] {
        &self.unlocked
    }

    /// Rebuild from persisted/replicated records (save restore, join). Unknown
    /// item names — a pack that is gone — are simply dropped, like effects.
    pub fn restore(&mut self, obtained: impl IntoIterator<Item = ItemType>, unlocked: Vec<String>) {
        self.obtained = obtained.into_iter().collect();
        self.unlocked = Vec::with_capacity(unlocked.len());
        self.index = HashSet::with_capacity(unlocked.len());
        for key in unlocked {
            if self.index.insert(key.clone()) {
                self.unlocked.push(key);
            }
        }
    }
}
