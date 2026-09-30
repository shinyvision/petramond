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
    pub grip: Option<[f32; 3]>,
    pub third_person: Option<SpriteHeldPose>,
}

/// A sprite's authored third-person seat, rotating and scaling about its grip.
/// Angles are radians; the grip is in the centred unit sprite's coordinates.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SpriteHeldPose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
    pub scale: f32,
    pub grip: [f32; 3],
}

impl Default for SpriteHeldPose {
    fn default() -> Self {
        Self {
            pitch: 0.0,
            yaw: 0.0,
            roll: 0.0,
            scale: 1.0,
            grip: [0.0; 3],
        }
    }
}

pub const DEFAULT_SPRITE_AXIS_DEGREES: f32 = 45.0;

impl HeldPose {
    pub const DEFAULT: HeldPose = HeldPose {
        pitch: 0.0,
        yaw: 1.8,
        roll: 0.0,
        grip: None,
        third_person: None,
    };
}
