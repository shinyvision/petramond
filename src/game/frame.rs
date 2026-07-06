//! Neutral app/client frame state read from [`Game`].
//!
//! This is the game-side boundary for App update/render: camera, environment,
//! and target/held-item state. It intentionally does not contain renderer DTOs
//! or terrain upload handles.

use crate::block::Block;
use crate::block_state::HeldBlockState;
use crate::camera::Camera;
use crate::item::ItemType;
use crate::mathh::SelectionShape;

use super::{Game, GameEnvironment};

pub(crate) struct ClientFrame<'a> {
    pub(crate) camera: &'a Camera,
    pub(crate) environment: GameEnvironment,
    pub(crate) selection: Option<SelectionShape>,
    pub(crate) held_item: ClientHeldItem,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientHeldItem {
    pub(crate) item: Option<ItemType>,
    pub(crate) block_state: HeldBlockState,
    pub(crate) mining: bool,
    pub(crate) mining_block: Option<Block>,
}

impl Game {
    /// Coherent neutral app-facing state for update/render after the game tick.
    pub(crate) fn client_frame(&self, now: f64) -> ClientFrame<'_> {
        let mining = self.mining.is_mining();
        ClientFrame {
            // The third-person boom camera when active; the first-person eye
            // otherwise. Sim consumers keep reading `self.cam` directly.
            camera: self.render_camera(),
            environment: self.environment(now),
            selection: self.look.map(|h| h.outline),
            held_item: ClientHeldItem {
                item: self.player.inventory.selected().map(|s| s.item),
                block_state: self.held_block_state(),
                mining,
                mining_block: mining.then(|| self.mining.block()).flatten(),
            },
        }
    }
}
