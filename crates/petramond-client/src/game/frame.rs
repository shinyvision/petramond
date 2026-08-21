//! Neutral app/client frame state read from [`Game`].
//!
//! This is the game-side boundary for App update/render: camera, environment,
//! and target/held-item state. It intentionally does not contain renderer DTOs
//! or terrain upload handles.

use petramond_math::math::SelectionShape;
use petramond_render::camera::Camera;
use petramond_world::block::Block;
use petramond_world::block_state::HeldBlockState;
use petramond_world::item::ItemType;

use super::{Game, GameEnvironment};

pub struct ClientFrame<'a> {
    pub camera: &'a Camera,
    pub environment: GameEnvironment,
    pub selection: Option<SelectionShape>,
    pub held_item: ClientHeldItem,
    /// The OFF-hand (left) item's frame state. `item == None` = empty slot —
    /// no left hand renders. Mining/rotation stay main-hand-only; an off-hand
    /// eat carries its progress here instead of on `held_item`.
    pub off_hand_item: ClientHeldItem,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ClientHeldItem {
    pub item: Option<ItemType>,
    pub variant: petramond_world::item::VariantId,
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
        // The one in-progress eat belongs to a HAND: its progress animates the
        // hand that is carrying the food, and only that one.
        let eating = self.eating_progress();
        let (eat_main, eat_off) = if view.eating_off_hand {
            (None, eating)
        } else {
            (eating, None)
        };
        let bob = self.view_bob.offset();
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
                eating: eat_main,
                bob,
            },
            off_hand_item: ClientHeldItem {
                item: view.inventory.off_hand().map(|s| s.item),
                variant: view
                    .inventory
                    .off_hand()
                    .map(|s| s.variant)
                    .unwrap_or_default(),
                // The R-rotation preview arms on the SELECTED item only.
                block_state: Default::default(),
                mining: false,
                mining_block: None,
                eating: eat_off,
                bob,
            },
        }
    }
}
