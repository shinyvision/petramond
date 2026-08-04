//! Core recipe unlocking: the engine's own progression policy.
//!
//! Deliberately built through the seams a mod would use — a post-event handler
//! on the bus plus `Player::progression` — for the same reason day/night is
//! (`server::daynight`). Nothing below reaches into crafting internals that a
//! `UnlockRecipe` HostCall could not.
//!
//! Two halves:
//!
//! - **The signal.** `item_obtained` is emitted here, from ONE per-tick scan of
//!   each session's inventory, because "the player now has one of these" has no
//!   single funnel: a pickup, a craft, a furnace output, a chest withdrawal and
//!   a mod's `GiveItem` all arrive by different paths. The scan compares
//!   against the player's persisted obtained set, so the event is a genuine
//!   first-ever transition rather than "an item moved".
//! - **The consequence.** One handler applies the default rule from
//!   `crafting::unlock`: hold everything a recipe needs and it opens. Mods add
//!   their own consequences by registering for the same event (or any other)
//!   and calling `UnlockRecipe`; the engine's registration runs first only
//!   because engine registrations always do.

use std::sync::Arc;

use petramond_world::crafting::UnlockIndex;
use crate::events::{EventBus, PostEvent, PostEventKind};
use crate::player::Player;

use super::game::ServerGame;

/// Register the engine's default unlock rule. Runs before mod init, so a mod
/// handler at equal priority observes an already-applied default.
pub fn install_core(bus: &mut EventBus, unlocks: Arc<UnlockIndex>) {
    bus.on_post(PostEventKind::ItemObtained, 0, move |ctx, ev| {
        let PostEvent::ItemObtained { player, item } = *ev else {
            return;
        };
        ctx.with_player(player, |p| {
            for key in unlocks.opened_by(item, p.progression.obtained()) {
                p.progression.unlock(key);
            }
        });
    });
}

/// Open every recipe the player's already-obtained items satisfy. Run when a
/// session starts: it reconciles a restored record with the catalog this world
/// actually loaded, so installing a pack (or editing a recipe row) never
/// leaves an earned recipe permanently invisible.
pub fn catch_up(player: &mut Player, unlocks: &UnlockIndex) {
    let opened: Vec<String> = unlocks
        .opened_by_all(player.progression.obtained())
        .map(str::to_owned)
        .collect();
    for key in opened {
        player.progression.unlock(&key);
    }
}

impl ServerGame {
    /// Emit `item_obtained` for every item kind that entered a session's
    /// inventory for the first time. Gated on the inventory revision, so an
    /// unchanged tick costs one comparison per session.
    ///
    /// Runs inside the last stage so the events drain at that stage's
    /// boundary: a recipe unlocked by a pickup is craftable on the same tick.
    pub fn detect_obtained_items(&mut self) {
        let mut fresh: Vec<(crate::player::PlayerId, petramond_world::item::ItemType)> = Vec::new();
        for sess in &mut self.sessions {
            let revision = sess.player.inventory.revision();
            if sess.last_obtained_scan == Some(revision) {
                continue;
            }
            sess.last_obtained_scan = Some(revision);
            let held = sess
                .player
                .inventory
                .raw_slots()
                .iter()
                .chain(std::iter::once(&sess.player.inventory.cursor().copied()))
                .flatten()
                .map(|stack| stack.item)
                .collect::<Vec<_>>();
            for item in held {
                if sess.player.progression.obtain(item) {
                    fresh.push((sess.id, item));
                }
            }
        }
        for (player, item) in fresh {
            self.bus.emit(PostEvent::ItemObtained { player, item });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::events::{PostEvent, PostEventKind};
    use petramond_world::item::{ItemStack, ItemType};

    /// `item_obtained` is a FIRST-EVER transition, not "an item moved": it
    /// fires once per kind however the item arrived, never again for that
    /// kind, and a second kind is its own event. The whole progression
    /// surface leans on that (handlers keep no memory of their own), and the
    /// scan is revision-gated, which is exactly what could silently drop one.
    #[test]
    fn an_item_kind_announces_itself_once_and_only_once() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let seen = Arc::new(AtomicUsize::new(0));
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let (seen, logs) = (seen.clone(), logs.clone());
            server
                .bus
                .on_post(PostEventKind::ItemObtained, 0, move |_, ev| {
                    if let PostEvent::ItemObtained { item, .. } = ev {
                        seen.fetch_add(1, Ordering::Relaxed);
                        logs.lock().unwrap().push(*item);
                    }
                });
        }
        // Whatever the fresh session already holds is not this test's subject.
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        seen.store(0, Ordering::Relaxed);
        logs.lock().unwrap().clear();

        server.sessions[0]
            .player
            .inventory
            .add(ItemStack::new(ItemType::Coal, 1));
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        assert_eq!(seen.load(Ordering::Relaxed), 1, "the first coal announces");
        assert_eq!(logs.lock().unwrap().as_slice(), &[ItemType::Coal]);

        // More of the same kind, and a plain re-pump, say nothing further.
        server.sessions[0]
            .player
            .inventory
            .add(ItemStack::new(ItemType::Coal, 4));
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        assert_eq!(
            seen.load(Ordering::Relaxed),
            1,
            "a kind already held never announces again"
        );

        // A different kind is its own first time.
        server.sessions[0]
            .player
            .inventory
            .add(ItemStack::new(ItemType::Dirt, 1));
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        assert_eq!(seen.load(Ordering::Relaxed), 2);
        assert_eq!(logs.lock().unwrap().last(), Some(&ItemType::Dirt));
    }
}
