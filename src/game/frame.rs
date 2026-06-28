//! Neutral app/client frame state read from [`Game`].
//!
//! This is the game-side boundary for App update/render cadence: camera pose,
//! environment, target/held-item state, and redraw activity. It intentionally
//! does not contain renderer DTOs or terrain upload handles.

use crate::block::Block;
use crate::camera::Camera;
use crate::item::ItemType;
use crate::mathh::{SelectionShape, Vec3};

use super::{Game, GameEnvironment};

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct CameraPose {
    pub(crate) pos: Vec3,
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
}

impl CameraPose {
    #[inline]
    fn from_camera(camera: &Camera) -> Self {
        Self {
            pos: camera.pos,
            yaw: camera.yaw,
            pitch: camera.pitch,
        }
    }
}

pub(crate) struct ClientFrame<'a> {
    pub(crate) camera: &'a Camera,
    pub(crate) camera_pose: CameraPose,
    pub(crate) environment: GameEnvironment,
    pub(crate) selection: Option<SelectionShape>,
    pub(crate) held_item: ClientHeldItem,
    pub(crate) activity: ClientFrameActivity,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientHeldItem {
    pub(crate) item: Option<ItemType>,
    pub(crate) mining: bool,
    pub(crate) mining_block: Option<Block>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientFrameActivity {
    pub(crate) visually_active: bool,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) struct ClientPreTickFrame {
    pub(crate) mesh_pending: bool,
}

impl Game {
    /// Frame-cadence state that must be sampled before [`Game::tick`] drains mesh work.
    pub(crate) fn client_frame_before_tick(&self) -> ClientPreTickFrame {
        ClientPreTickFrame {
            mesh_pending: self.world.has_dirty_meshes(),
        }
    }

    /// Coherent neutral app-facing state for update/render after the game tick.
    pub(crate) fn client_frame(&self, now: f64) -> ClientFrame<'_> {
        let mining = self.mining.is_mining();
        ClientFrame {
            camera: &self.cam,
            camera_pose: CameraPose::from_camera(&self.cam),
            environment: self.environment(now),
            selection: self.look.map(|h| h.outline),
            held_item: ClientHeldItem {
                item: self.player.inventory.selected().map(|s| s.item),
                mining,
                mining_block: mining.then(|| self.mining.block()).flatten(),
            },
            activity: ClientFrameActivity {
                visually_active: self.is_visually_active(),
            },
        }
    }
}
