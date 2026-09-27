//! Procedural block-shape ABI: what a mod's WASM uses to bake a shape's
//! geometry — sim side (collision boxes + light aperture, checked server vs
//! replica) and client render side (drawn boxes) — plus placement plan. Host
//! bakes each shape kind once per section and caches it; mesher/physics/light
//! read the cache, never call the guest per cell or per frame.
//!
//! Positions are raw `[i32; 3]`, boxes raw `[f32; 3]` pairs — no `Aabb`/`IVec3`
//! on the wire.
//!
//! # Per-cell purity on the SIM side (hard requirement, violation = desync)
//!
//! SIM bake reply must be a pure function of [`CellInput`] and `shape_kind`.
//! It runs on the server and again on each client's replica for prediction.
//! Reading instance state (RNG, a counter, arena bump) or the surrounding
//! batch will diverge server and client.
//!
//! ## Purity means server/replica agreement, not "block ids only"
//!
//! [`CellInput`] includes the cell's own [`state`](CellInput::state) and its
//! six [`neighbor_states`](CellInput::neighbor_states) — opaque bytes for the
//! shape kind's declared `state_key`, from replicated per-cell KV. So stateful
//! families work (a stair reading neighbor facings to resolve a corner): state
//! is replicated and applied before baking, and changing a cell re-bakes it
//! plus its six neighbors, so server and replicas bake from identical inputs.
//! What's forbidden is unreplicated/ambient state (RNG, wall clock, batch) —
//! not replicated per-cell state, which is exactly what this input provides.
//!
//! RENDER bake is presentation-only and may also batch-read any replicated KV
//! via `ClientCellKvAt` (e.g. tint from a dye color); sim bake only ever sees
//! its declared state key, identical on both sides.
//!
//! # Shared bake crate (recommended)
//!
//! A pack shipping both server `wasm` and client `client_wasm` bakes the same
//! shape twice — the two must agree byte-for-byte on the sim side (collision +
//! aperture) or the shape desyncs silently. Put the bake logic in one shared
//! crate both binaries depend on (as the bundled `furniture` pack does) so
//! they can't drift apart.

use serde::{Deserialize, Serialize};

use crate::ids::BlockId;

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ShapeAabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum LightAperture {
    Opaque,
    Open,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CellInput {
    pub world_pos: [i32; 3],
    pub block_id: BlockId,
    pub neighbor_ids: [BlockId; 6],
    pub state: Option<Vec<u8>>,
    pub neighbor_states: [Option<Vec<u8>>; 6],
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedSimCell {
    pub collision_boxes: Vec<ShapeAabb>,
    pub light_aperture: LightAperture,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ShapeRenderBox {
    pub aabb: ShapeAabb,
    pub tint: Option<[u8; 3]>,
    pub ao: Option<u8>,
    pub dyed: bool,
}

impl From<ShapeAabb> for ShapeRenderBox {
    fn from(aabb: ShapeAabb) -> Self {
        ShapeRenderBox {
            aabb,
            tint: None,
            ao: None,
            dyed: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedRenderCell {
    pub boxes: Vec<ShapeRenderBox>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct PlaceInputsView {
    pub hit: [i32; 3],
    pub normal: [i32; 3],
    pub place_pos: [i32; 3],
    pub player_facing: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShapePlacementResult {
    pub accepted: bool,
    pub anchor: [i32; 3],
    pub cells: Vec<[i32; 3]>,
    pub block: Option<BlockId>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedItemGeometry {
    pub boxes: Vec<ShapeAabb>,
}
