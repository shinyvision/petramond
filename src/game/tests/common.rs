use super::super::tick::TickEvents;
use super::super::Game;
use crate::camera::Camera;
use crate::inventory::Inventory;
use crate::item::{ItemStack, ItemType};
use crate::mathh::{IVec3, SelectionShape, Vec3};
use crate::player::RaycastHit;

pub(super) fn game() -> Game {
    Game::new(Camera::new(Vec3::new(0.0, 80.0, 0.0), 16.0 / 9.0), "", 1, 1)
}

/// A hotbar slot filled with one full demo stack, for tests that need the
/// player holding something (the real starting inventory is empty).
pub(super) fn filled_inventory() -> Inventory {
    let mut inv = Inventory::new();
    inv.add(ItemStack::new(ItemType::Dirt, 64));
    inv
}

pub(super) fn apply_drop_actions(game: &mut Game) -> TickEvents {
    let mut events = TickEvents::default();
    game.tick_drops(&mut events);
    events
}

pub(super) fn hit(pos: IVec3, normal: IVec3) -> RaycastHit {
    RaycastHit {
        block: pos,
        normal,
        outline: SelectionShape::full_block(pos),
    }
}

pub(super) fn install_empty_chunk(game: &mut Game) {
    let pos = crate::chunk::ChunkPos::new(0, 0);
    game.world.clear_world();
    game.world
        .insert_chunk_for_test(pos, crate::chunk::Chunk::new(0, 0));
}

pub(super) fn count_item(inv: &Inventory, item: ItemType) -> u32 {
    (0..crate::inventory::TOTAL_SLOTS)
        .filter_map(|i| inv.slot(i))
        .filter(|s| s.item == item)
        .map(|s| s.count as u32)
        .sum()
}
