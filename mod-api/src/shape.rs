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
//! # Per-cell purity (HARD requirement on the SIM side — a violation is a
//! # multiplayer desync)
//!
//! Every SIM bake reply MUST be a pure function of that cell's [`CellInput`]
//! and the `shape_kind`. The SIM bake runs on the server AND is re-run against
//! each client's replica for prediction; the host groups a section's cells and
//! iterates them in a defined order, but a bake that reads instance state (an
//! `RngU64` stream, a counter, an arena bump) or the surrounding batch would
//! diverge server↔client with no reproducibility.
//!
//! ## Purity is about server/replica AGREEMENT, not "block ids only"
//!
//! [`CellInput`] carries the cell's own [`state`](CellInput::state) and its six
//! [`neighbor_states`](CellInput::neighbor_states) — the opaque bytes of the
//! shape kind's declared state key (`shapes.json` `"state_key"`), read from
//! REPLICATED per-cell KV. A stateful family (a stair reading its neighbours'
//! facings to resolve a corner) is therefore expressible: the state is
//! replicated, applied before the bake pump runs, and a change to any cell
//! re-bakes it AND its six neighbours, so the server and every replica bake
//! from identical inputs. What purity forbids is reading UNreplicated or
//! ambient state (an RNG, a wall clock, the batch) — never replicated per-cell
//! state, which is exactly what the widened input now provides.
//!
//! The RENDER bake, being presentation-only, may additionally batch-read any
//! replicated KV via `ClientCellKvAt` (e.g. tinting from a dye color); the sim
//! bake sees only its declared state key, kept identical on both sides.
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
/// and render sides so a shape gets identical inputs each way. This is the
/// ENTIRE bake input: a bake must be a pure function of it (see the module
/// purity note).
///
/// `neighbor_ids` / `neighbor_states` are in `-x,+x,-y,+y,-z,+z` order.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CellInput {
    pub world_pos: [i32; 3],
    pub block_id: BlockId,
    pub neighbor_ids: [BlockId; 6],
    /// This cell's per-cell shape state — the opaque bytes of the shape kind's
    /// declared `state_key` (replicated cell KV), or `None` when the shape
    /// declares no state key or the cell carries no value. What makes a
    /// STATEFUL family (a stair's facing/half) expressible through the ABI.
    pub state: Option<Vec<u8>>,
    /// The same state for the six neighbours — a stair reads these to resolve
    /// its corner shape. `None` per neighbour when absent.
    pub neighbor_states: [Option<Vec<u8>>; 6],
}

/// One baked SIM cell (deterministic): the authoritative collision boxes the
/// physics sweeps and the light aperture the flood reads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedSimCell {
    pub collision_boxes: Vec<ShapeAabb>,
    pub light_aperture: LightAperture,
}

/// One drawn box of a render bake: its geometry plus an optional RGB multiply
/// TINT (linear, `[r, g, b]` unorm8; `None` = untinted). The tint multiplies
/// the box's sampled texels per vertex — the same lane biome grass/water
/// tinting uses — so a shape can colorize part of itself from replicated
/// state (a dye vat's fluid, stained panes) while its other boxes keep their
/// authored tiles. `ao` scales how strongly ambient occlusion darkens this
/// box's faces, in PERCENT (`None` = 100, the ordinary full effect; `0` =
/// AO-immune) — the darkening is scaled within the engine's vertex-AO steps,
/// so a fluid surface sitting inside a pot can stay bright while the pot
/// keeps its creases. `dyed` picks the SAMPLING BASE the tint lands on:
/// `false` = the box's authored tiles (a plain hue-preserving multiply —
/// shading a colored texture darker); `true` = the tiles' dye-base twins
/// (desaturated, brightness-normalized), so the tint can both dye and
/// whiten — set it whenever the tint IS a dye color. Presentation-only:
/// none of these fields exist on the sim side.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ShapeRenderBox {
    pub aabb: ShapeAabb,
    pub tint: Option<[u8; 3]>,
    pub ao: Option<u8>,
    pub dyed: bool,
}

impl From<ShapeAabb> for ShapeRenderBox {
    /// An untinted, full-AO, undyed box — the ordinary case.
    fn from(aabb: ShapeAabb) -> Self {
        ShapeRenderBox {
            aabb,
            tint: None,
            ao: None,
            dyed: false,
        }
    }
}

/// One baked RENDER cell (client presentation): the axis-aligned boxes the
/// mesher draws (each textured face-by-face from the block's own
/// `[top, bottom, side]` tiles, carved-from-the-block like a stair, so a shape
/// reuses its block's textures with no per-quad atlas reference on the wire),
/// each optionally tinted ([`ShapeRenderBox`]).
/// Voxel furniture is boxes; the box form gets correct lighting/AO/UV for free
/// from the engine's shared plane-quad emitter. The selection/target box is the
/// union of these boxes (engine-derived), not a wire field.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BakedRenderCell {
    pub boxes: Vec<ShapeRenderBox>,
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
