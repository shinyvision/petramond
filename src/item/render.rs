use crate::atlas::Tile;
use crate::block::Block;

/// How an item is drawn in inventory slots and in-hand.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ItemRenderKind {
    /// A full-cube block-item: render isometric 3D in slots, held as a 3D cube.
    BlockCube(Block),
    /// A flat sprite (cross-plant blocks like flowers/grass, and future tools):
    /// render the tile flat in slots; held and dropped it extrudes into a
    /// pixel-perfect 3D slab one texel (1/16 block) deep.
    Sprite(Tile),
    /// A data-driven bbmodel block: render its actual baked model (cubes + the model
    /// atlas) in slots / in-hand / dropped, not a stand-in cube. See `crate::block_model`.
    Model(crate::block_model::BlockModelKind),
}

/// First-person hold orientation for a [`Sprite`](ItemRenderKind::Sprite) item:
/// the Euler tilt (radians) applied to the upright, origin-centred extruded slab
/// before it's seated in the hand (see [`crate::render`]'s `held_sprite`). A long
/// tool is laid diagonally like a swung handle (`roll != 0`); a small item stands
/// upright (`roll == 0`). Per-item so each item can declare how it's held.
#[derive(Copy, Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeldPose {
    pub pitch: f32,
    pub yaw: f32,
    pub roll: f32,
}

impl HeldPose {
    /// Upright hold for an ordinary sprite item (flowers, raw drops): no roll, so
    /// it stands straight up in the hand. The shared default carried by every
    /// item that isn't a tool with its own pose.
    pub const DEFAULT: HeldPose = HeldPose {
        pitch: 0.0,
        yaw: 1.8,
        roll: 0.0,
    };
}
