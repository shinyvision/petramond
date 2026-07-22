use serde::{Deserialize, Serialize};

pub use super::host::HostCall;
use crate::client::{ClientCanvasEvent, ClientFrameData, ClientUiEvent};
use crate::data::{AiNodeCtx, AiNodeDecision, BlockHookKind, HostileSpawnCandidate};
use crate::events::{EventPayload, Outcome};
use crate::ids::BlockId;
use crate::sched::WorldgenStage;
use crate::shape::{
    BakedItemGeometry, BakedRenderCell, BakedSimCell, CellInput, PlaceInputsView,
    ShapePlacementResult,
};

/// One worldgen block write: `(world position, block)`. Applied by the engine
/// through a section-clipping sink — writes outside the dispatched section are
/// dropped (that clipping IS the seam mechanism, see [`GuestCall::GenFeature`]).
pub type GenWrite = ([i32; 3], BlockId);

/// Host → guest: what the engine asks a mod to run through `mod_dispatch`.
/// (`mod_init` is its own export and carries no payload.)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestCall {
    /// Run the tick system the mod registered under `id`.
    TickSystem {
        id: u32,
    },
    /// Handle one event with the handler registered under `id`. The guest
    /// returns the (possibly mutated) payload in [`GuestRet::Event`]. The
    /// event's kind is the payload's variant ([`EventPayload::kind`]) — this
    /// hot dispatch deliberately carries no redundant kind lane.
    HandleEvent {
        id: u32,
        payload: EventPayload,
    },

    // --- worldgen hooks --------------------------------------------------------
    /// Generate one registered feature's writes for one 16³ section.
    /// → [`GuestRet::GenWrites`].
    ///
    /// DETERMINISM CONTRACT (binding — a violation shows up as world seams):
    /// the reply must be a pure function of this call's fields. Worldgen
    /// instances are SEPARATE wasm instances per worker thread sharing NOTHING
    /// with the tick instance; no sim-scoped host call works here, and any
    /// state carried between calls breaks (seed, section) reproducibility.
    /// A feature spanning section boundaries must derive identical per-origin
    /// decisions in EVERY section its writes touch (positional RNG over
    /// `(seed, origin)` + the column data below); the engine clips each call's
    /// writes to its own section, which makes consistent emission seamless.
    GenFeature {
        feature_id: u32,
        /// Section coordinates (16³ units; world origin = `pos * 16`).
        section_pos: [i32; 3],
        /// The world seed — feed it to the SDK's positional RNG.
        seed: u32,
        /// 4096-byte snapshot of the section as of this attach point (engine
        /// stages + earlier hooks applied), layout `y*256 + z*16 + x`.
        blocks: Vec<u8>,
        /// 256 entries (`z*16 + x`), the column's post-cave bare-ground top
        /// (world Y, before vegetation/trees; below `sea_level` = submerged
        /// or floorless). Identical for every section of one column.
        surface_heights: Vec<i32>,
        /// 256 biome ids (`z*16 + x`), identical for every section of a column.
        biomes: Vec<u8>,
        sea_level: i32,
    },
    /// Run a registered stage REPLACEMENT. Same field meanings and determinism
    /// contract as [`GuestCall::GenFeature`]. Expected reply by stage:
    /// `Climate` → [`GuestRet::GenBiomes`] (256 ids; `section_pos` is
    /// `[cx, 0, cz]`, `blocks` empty, `biomes` = the engine's proposal),
    /// `Terrain` → [`GuestRet::GenBlocks`] (the full 4096 fill; `blocks`
    /// empty), others → [`GuestRet::GenWrites`]. A wrong-shape reply disables
    /// the mod; the engine stage then runs as the fallback.
    GenStage {
        callback_id: u32,
        stage: WorldgenStage,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u8>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },

    // --- mod GUIs ---------------------------------------------------------------
    /// A button of the mod's own GUI was clicked (dispatched on the tick, in
    /// click order, to the mod whose namespace `kind_key` carries). `pos` is
    /// the block the GUI was opened from (`None` for a programmatic
    /// [`HostCall::GuiOpen`]). → [`GuestRet::Unit`].
    GuiClick {
        kind_key: String,
        widget_id: String,
        pos: Option<[i32; 3]>,
    },

    // --- Hostile spawning -------------------------------------------------
    /// Ask a registered hostile spawner whether this candidate should produce
    /// a hostile species. → [`GuestRet::HostileSpawn`].
    HostileSpawnCandidate {
        callback_id: u32,
        candidate: HostileSpawnCandidate,
    },

    // --- block behaviors --------------------------------------------------------
    /// A hook fired on a block whose row's `behavior` the mod registered via
    /// [`HostCall::RegisterBlockBehavior`]. Dispatched on the game tick, in
    /// hook-fire order, right after the world's own scheduled/random ticks —
    /// so a handler edits the world through sim host calls one dispatch step
    /// later than an engine-compiled behavior would. → [`GuestRet::Unit`].
    BlockBehavior {
        callback_id: u32,
        kind: BlockHookKind,
        pos: [i32; 3],
    },

    // --- Scripted AI nodes (landed 2026-07-06) ------------------------------
    /// One AI decision for one mob, this tick — the node the mod registered
    /// via [`HostCall::RegisterAiNode`]. DECISION-ONLY: the dispatch runs
    /// inside the mob tick with NO simulation scope, so sim host calls
    /// (world edits, spawns, player state) error here; the core calls
    /// (`CurrentTick`, RNG, log) work, and `ctx.tick` already carries the
    /// tick without a host call. Return desires in
    /// [`GuestRet::AiDecision`]; the engine's brain arbitration merges them
    /// by the brain row's priority.
    /// → [`GuestRet::AiDecision`].
    AiNode {
        callback_id: u32,
        ctx: AiNodeCtx,
    },

    // --- Presentation-only client module ----------------------------------
    ClientFrame {
        frame: ClientFrameData,
    },
    ClientKey {
        action_id: u32,
        pressed: bool,
    },
    ClientUi {
        kind_key: String,
        event: ClientUiEvent,
    },
    ClientCanvas {
        canvas_key: String,
        event: ClientCanvasEvent,
    },
    /// Mouse-wheel travel over this module's open modal canvas. `x`/`y` are
    /// canvas-local logical pixels (the cursor position), `delta` is in wheel
    /// notches with positive = scrolled up / away from the user. The host
    /// coalesces wheel events to at most one call per app frame.
    ClientCanvasScroll {
        canvas_key: String,
        x: f32,
        y: f32,
        delta: f32,
    },

    // --- Procedural shape bakes (Layer 3) ---------------------------------------
    /// Bake the DETERMINISTIC sim geometry (collision boxes + light aperture) for
    /// every cell of one custom shape kind in a section, in one batch. Run on the
    /// server `wasm` AND re-run against the client replica for prediction — each
    /// cell's reply MUST be a pure function of that cell's [`CellInput`] (same
    /// determinism contract as [`GuestCall::GenFeature`]; see the per-cell purity
    /// note on the [`crate::shape`] module). → [`GuestRet::BakedSim`].
    BakeShapeSim {
        shape_kind: u8,
        cells: Vec<CellInput>,
    },
    /// Bake the client RENDER geometry (the drawn boxes) for every cell of one
    /// custom shape kind in a section. Client `client_wasm` only; no determinism
    /// requirement. → [`GuestRet::BakedRender`].
    BakeShapeRender {
        shape_kind: u8,
        cells: Vec<CellInput>,
    },
    /// Bake one block's ITEM geometry (icon / dropped / in-hand), once at load.
    /// → [`GuestRet::BakedItem`].
    BakeShapeItem {
        shape_kind: u8,
        block_id: BlockId,
    },
    /// Compute a custom shape's placement plan for one click — read-only world
    /// access through the ordinary `GetBlock` host calls (mutating host calls
    /// error during this dispatch). Placement is single-cell: the host accepts a
    /// plan writing exactly one cell near the click. → [`GuestRet::ShapePlacement`].
    ShapePlacementPlan {
        shape_kind: u8,
        block_id: BlockId,
        inputs: PlaceInputsView,
    },
}

/// Guest → host reply for a [`GuestCall`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestRet {
    Unit,
    /// Reply to [`GuestCall::HandleEvent`]: the verdict plus the payload echoed
    /// back so the engine can read the mutable fields.
    Event {
        outcome: Outcome,
        payload: EventPayload,
    },
    /// Reply to [`GuestCall::GenFeature`] and to non-climate/terrain
    /// [`GuestCall::GenStage`]: world-position block writes, applied in order
    /// through the engine's section clip. An unregistered block id disables
    /// the mod (never reaches world storage).
    GenWrites(Vec<GenWrite>),
    /// Reply to a `Terrain` [`GuestCall::GenStage`]: the complete 4096-block
    /// section fill (layout `y*256 + z*16 + x`). Must be exactly 4096
    /// registered ids.
    GenBlocks(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Reply to a `Climate` [`GuestCall::GenStage`]: the 256-entry column
    /// biome map (`z*16 + x`). Must be exactly 256 valid biome ids.
    GenBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    /// Reply to [`GuestCall::HostileSpawnCandidate`]: `Some(registry_key)` to
    /// ask core to spawn that hostile species here, `None` to reject this site.
    HostileSpawn(Option<String>),
    /// Reply to [`GuestCall::AiNode`]: the node's desires for this mob this
    /// tick (`None` = no opinion on anything, same as the default decision).
    AiDecision(Option<AiNodeDecision>),

    // --- Procedural shape bakes (Layer 3) ---------------------------------------
    /// Reply to [`GuestCall::BakeShapeSim`]: one [`BakedSimCell`] per input cell,
    /// in order. An EMPTY reply means "no bake, use the static fallback" (a shape
    /// that declines to bake, or a `client_wasm` implementing only the render
    /// side); a wrong-but-NONZERO length is a protocol break that disables the
    /// mod. Boxes are sanitized at ingest (see [`BakedSimCell`]).
    BakedSim(Vec<BakedSimCell>),
    /// Reply to [`GuestCall::BakeShapeRender`]: one [`BakedRenderCell`] per input
    /// cell, in order. Empty = fallback, wrong-nonzero = protocol break (same
    /// policy as [`GuestRet::BakedSim`]).
    BakedRender(Vec<BakedRenderCell>),
    /// Reply to [`GuestCall::BakeShapeItem`]: the block's item geometry.
    BakedItem(BakedItemGeometry),
    /// Reply to [`GuestCall::ShapePlacementPlan`]: the placement plan.
    ShapePlacement(ShapePlacementResult),
}
