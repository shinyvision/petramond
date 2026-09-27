use std::sync::Arc;

use crate::events::{EventBus, PostEvent, PostEventKind};
use crate::player::Player;
use petramond_world::crafting::{Recipes, UnlockIndex};

use super::mod_runtime::ModRuntime;
use super::sessions::SessionRegistry;

pub struct RecipeCatalog {
    recipes: Recipes,
    unlocks: Arc<UnlockIndex>,
}

impl RecipeCatalog {
    pub fn new(recipes: Recipes) -> Self {
        let unlocks = Arc::new(UnlockIndex::build(recipes.crafting()));
        Self { recipes, unlocks }
    }

    pub fn recipes(&self) -> &Recipes {
        &self.recipes
    }

    pub fn unlocks(&self) -> &Arc<UnlockIndex> {
        &self.unlocks
    }
}

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

pub fn catch_up(player: &mut Player, unlocks: &UnlockIndex) {
    let opened: Vec<String> = unlocks
        .opened_by_all(player.progression.obtained())
        .map(str::to_owned)
        .collect();
    for key in opened {
        player.progression.unlock(&key);
    }
}

pub fn detect_obtained_items(sessions: &mut SessionRegistry, mods: &mut ModRuntime) {
    let mut fresh: Vec<(crate::player::PlayerId, petramond_world::item::ItemType)> = Vec::new();
    for sess in sessions {
        let revision = sess.player.inventory.revision();
        if sess.replication.last_obtained_scan == Some(revision) {
            continue;
        }
        sess.replication.last_obtained_scan = Some(revision);
        let held = sess
            .player
            .inventory
            .raw_slots()
            .iter()
            .chain(std::iter::once(&sess.player.inventory.cursor().copied()))
            .chain(std::iter::once(&sess.player.inventory.off_hand().copied()))
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
        mods.emit(PostEvent::ItemObtained { player, item });
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use crate::events::{PostEvent, PostEventKind};
    use petramond_world::item::{ItemStack, ItemType};

    #[test]
    fn an_item_kind_announces_itself_once_and_only_once() {
        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let seen = Arc::new(AtomicUsize::new(0));
        let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let (seen, logs) = (seen.clone(), logs.clone());
            server
                .mods
                .bus_mut()
                .on_post(PostEventKind::ItemObtained, 0, move |_, ev| {
                    if let PostEvent::ItemObtained { item, .. } = ev {
                        seen.fetch_add(1, Ordering::Relaxed);
                        logs.lock().unwrap().push(*item);
                    }
                });
        }
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

        server.sessions[0]
            .player
            .inventory
            .add(ItemStack::new(ItemType::Dirt, 1));
        server.pump_tagged(0.06, &mut Vec::new(), &[]);
        assert_eq!(seen.load(Ordering::Relaxed), 2);
        assert_eq!(logs.lock().unwrap().last(), Some(&ItemType::Dirt));
    }
}
