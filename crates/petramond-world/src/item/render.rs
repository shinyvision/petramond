use crate::block::Block;
use crate::tile::Tile;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemRenderKind {
    BlockCube(Block),
    Sprite(Tile),
    Model(crate::block_model::BlockModelKind),
}

impl ItemRenderKind {
    pub const NAMES: [&'static str; 3] = ["block", "sprite", "model"];

    pub fn name(self) -> &'static str {
        match self {
            Self::BlockCube(_) => "block",
            Self::Sprite(_) => "sprite",
            Self::Model(_) => "model",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeldPose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

pub const DEFAULT_SPRITE_AXIS_DEGREES: f32 = 45.0;

impl HeldPose {
    pub const DEFAULT: HeldPose = HeldPose {
        pitch: 0.0,
        yaw: 1.8,
        roll: 0.0,
    };
}
