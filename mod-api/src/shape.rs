//! Procedural block-shape ABI (Layer 3): the vocabulary a mod's WASM uses to
//! BAKE a custom block shape's geometry — the deterministic sim side (collision
//! boxes + a light aperture, cross-checked server↔replica) and the client
//! render side (the drawn boxes) — plus the per-interaction placement plan. The
//! host batches all cells of one shape kind in a section into a single bake call
//! and caches the result; the hot paths (mesher, physics, light) read the cache,
//! never the guest per cell/frame.
//!
//! Positions are raw `[i32; 3]` and boxes raw `[f32; 3]` pairs, matching the
//! rest of this crate (there is no `Aabb`/`IVec3` type on the wire).
//!
//! # Per-cell purity (HARD requirement — a violation is a multiplayer desync)
//!
//! Every bake reply MUST be a pure function of that cell's [`CellInput`]
//! (`block_id` + the six `neighbor_ids`) and the `shape_kind`. The SIM bake runs
//! on the server AND is re-run against each client's replica for prediction; the
//! host groups a section's cells and iterates them in a defined order, but a bake
//! that reads instance state (an `RngU64` stream, a counter, an arena bump) or
//! the surrounding batch would diverge server↔client with no reproducibility.
//! There is no per-cell state on the wire: the input is the cell's block and
//! neighbours, nothing else.
//!
//! # Shared bake crate (recommendation)
//!
//! A pack that ships both a server `wasm` and a client `client_wasm` bakes the
//! same shape in two binaries; the two MUST agree byte-for-byte on the SIM side
//! (collision + aperture) or a shape desyncs silently. Put the bake in ONE crate
//! both binaries depend on (the bundled `furniture` pack does this) so the two
//! sides cannot drift.

use serde::{Deserialize, Serialize};

use crate::ids::BlockId;

/// One cell-local axis-aligned box (`0.0..1.0` per axis), the wire form of the
/// engine's collision/selection `Aabb`. The host SANITIZES every baked box at
/// ingest (finite, `min <= max` per axis, clamped to the cell with a small
/// margin, count-capped); a breach freezes the shape to its static fallback, the
/// same policy as a wrong-length reply.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ShapeAabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// How a custom shape's cell participates in light propagation — the sim bake's
/// per-cell "opaque to light" decision. Only the two coarse states exist: a cell
/// either blocks light like a full cube or passes it like open air. (There is no
/// partial/octant aperture — the light flood is per-cell.)
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub enum LightAperture {
    /// Blocks light like a full cube.
    Opaque,
    /// Passes light like open air.
    Open,
}

/// The neighbourhood context of one cell handed to a bake — the same on the sim
/// and render sides so a shape gets identical inputs each way. `neighbor_ids`
/// are in `-x,+x,-y,+y,-z,+z` order. This is the ENTIRE bake input: a bake must
/// be a pure function of it (see the module purity note).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CellInput {
    pub world_pos: [i32; 3],
    pub block_id: BlockId,
    pub neighbor_ids: [BlockId; 6],
}

/// One baked SIM cell (deterministic): the authoritative collision boxes the
/// physics sweeps and the light aperture the flood reads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedSimCell {
    pub collision_boxes: Vec<ShapeAabb>,
    pub light_aperture: LightAperture,
}

/// One baked RENDER cell (client presentation): the axis-aligned boxes the
/// mesher draws (each textured face-by-face from the block's own
/// `[top, bottom, side]` tiles, carved-from-the-block like a stair, so a shape
/// reuses its block's textures with no per-quad atlas reference on the wire).
/// Voxel furniture is boxes; the box form gets correct lighting/AO/UV for free
/// from the engine's shared plane-quad emitter. The selection/target box is the
/// union of these boxes (engine-derived), not a wire field.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedRenderCell {
    pub boxes: Vec<ShapeAabb>,
}

/// The read-only placement context a custom shape's `ShapePlacementPlan`
/// dispatch validates against (it also reads the world through the ordinary
/// `GetBlock` host calls — mutating host calls error during this dispatch).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct PlaceInputsView {
    pub hit: [i32; 3],
    pub normal: [i32; 3],
    pub place_pos: [i32; 3],
    /// The placing player's facing (`Facing` discriminant: N, S, W, E).
    pub player_facing: u8,
}

/// A custom shape's placement plan: whether it accepts the click, the anchor
/// cell it writes, and which block row lands there. Placement is SINGLE-CELL
/// and stateless: the host requires an accepted plan to write exactly one cell
/// (`cells` empty, or exactly `[anchor]`) within a small radius of
/// `place_pos`. `cells` is retained for a future multi-cell layer but the host
/// rejects a plan with a wider footprint.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShapePlacementResult {
    pub accepted: bool,
    pub anchor: [i32; 3],
    pub cells: Vec<[i32; 3]>,
    /// The row written at the anchor: `None` writes the held block (the
    /// ordinary case); `Some(row)` writes a SIBLING row of the same shape
    /// kind instead — orientation as block identity (the ladder-row
    /// pattern), how a shape with directional variants (a chain's three
    /// axes) lets the plan pick the variant from `PlaceInputsView::normal`.
    /// The host refuses any row that does not share the placed shape kind
    /// (a kind belongs to one pack, so a plan can never reach across packs).
    pub block: Option<BlockId>,
}

/// The item geometry a shape bakes once (load-time), reused for its icon,
/// dropped entity, and in-hand form: the axis-aligned boxes drawn as textured
/// cuboids of the block's tiles. Sanitized at ingest like the render/sim boxes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedItemGeometry {
    pub boxes: Vec<ShapeAabb>,
}
