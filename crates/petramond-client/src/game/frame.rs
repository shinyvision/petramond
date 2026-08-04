//! Neutral app/client frame state read from [`Game`].
//!
//! This is the game-side boundary for App update/render: camera, environment,
//! and target/held-item state. It intentionally does not contain renderer DTOs
//! or terrain upload handles.

use petramond::block::Block;
use petramond::block_state::HeldBlockState;
use petramond_render::camera::Camera;
use petramond::item::ItemType;
use petramond::mathh::SelectionShape;

use super::{Game, GameEnvironment};

pub struct ClientFrame<'a> {
    pub camera: &'a Camera,
    pub environment: GameEnvironment,
    pub selection: Option<SelectionShape>,
    pub held_item: ClientHeldItem,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClientHeldItem {
    pub item: Option<ItemType>,
    pub variant: petramond::item::VariantId,
    pub block_state: HeldBlockState,
    pub mining: bool,
    pub mining_block: Option<Block>,
    /// A food item is mid-eat (held secondary button): the eat's progress in
    /// `[0, 1)` — the animation carries the food deeper toward the mouth as it
    /// advances. `None` on ordinary frames.
    pub eating: Option<f32>,
    /// The camera's normalized walk sway this frame — the hand follows a
    /// LAGGED copy of it (see `game::view_bob` and `HeldItemAnimator`).
    pub bob: [f32; 2],
}

impl Game {
    /// Coherent neutral app-facing state for update/render after the game
    /// tick. Held-item/mining/eating state reads the REPLICATED self view,
    /// never the server session.
    pub fn client_frame(&self, now: f64) -> ClientFrame<'_> {
        let view = &self.self_view;
        let mining = view.mining.is_some();
        // The mined block is re-read from the REPLICA at the replicated
        // target cell — it feeds the dig-sound pick.
        let mining_block = view
            .mining
            .map(|(p, _)| Block::from_id(self.replica.chunk_block(p.x, p.y, p.z)));
        ClientFrame {
            // The third-person boom camera when active; the first-person eye
            // otherwise. Sim consumers keep reading `self.cam` directly.
            camera: self.render_camera(),
            environment: self.environment(now),
            selection: self.look.map(|h| h.outline),
            held_item: ClientHeldItem {
                item: view.inventory.selected().map(|s| s.item),
                variant: view
                    .inventory
                    .selected()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                block_state: self.held_block_state(),
                mining,
                mining_block,
                eating: self.eating_progress(),
                bob: self.view_bob.offset(),
            },
        }
    }
}
