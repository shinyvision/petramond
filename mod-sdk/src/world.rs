//! Sim-scoped world reads and writes: blocks, light, scheduled ticks,
//! model-block swaps, and spawn-support queries.

use mod_api::{BlockId, CollisionShape, LightData, ModelGroupData, RayFilter, RaycastHitData};

use crate::__rt::host_fn;

host_fn! {
    /// The block at a world cell, or `None` when its section is unloaded, still
    /// STREAMING IN (a gen job or the player's saved record has not finished
    /// landing — reading the half-streamed content would lie), or the cell is
    /// outside the world's vertical range. Treat `None` as "state frozen, retry
    /// later"; never as evidence about what the cell holds. Air is
    /// `Some(BlockId(0))`.
    pub fn get_block(pos: [i32; 3]) -> Option<BlockId> => GetBlock { pos } => Block
}

host_fn! {
    /// Batched [`get_block`]: one result per position, in order. At most
    /// [`crate::SIM_BATCH_MAX`] positions per call; more disables the mod —
    /// chunk larger sweeps across ticks.
    pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => GetBlocks { positions } => Blocks
}

host_fn! {
    /// Every cell in the INCLUSIVE box `min..=max` currently holding one of
    /// `blocks`, resolved host-side in one scan — the neighbourhood-search
    /// primitive ("is there a filled trough near this mob", "where is the
    /// nearest ore/plant"); never page a box through [`get_blocks`] to search
    /// it. Positions arrive in scan order (ascending `y`, then `z`, then `x`);
    /// fold for the nearest match yourself. The box is capped at [`crate::FIND_BLOCKS_VOLUME_MAX`] cells and
    /// reads are stream-final: ANY unreadable cell makes the whole reply
    /// `None` ("state frozen, retry later").
    pub fn find_blocks(min: [i32; 3], max: [i32; 3], blocks: Vec<BlockId>)
        -> Option<Vec<[i32; 3]>>
        => FindBlocks { min, max, blocks } => FoundBlocks
}

host_fn! {
    /// Set one block through the engine's full edit path (relight, neighbour
    /// updates). Returns `false` when the cell is unloaded / out of range.
    pub fn set_block(pos: [i32; 3], block: BlockId) -> bool => SetBlock { pos, block } => Bool
}

host_fn! {
    /// Swap the placed multi-cell MODEL block group at `pos` (any of its cells) to
    /// `block` — another model block sharing the exact same oriented footprint
    /// (e.g. a machine's lit/unlit variants). Its container, facing, and section
    /// cell KV survive; the region relights (emission differences glow). Both
    /// blocks must be this mod's own. `false` = no model group there, footprint
    /// mismatch, or unloaded.
    pub fn swap_model_block(pos: [i32; 3], block: BlockId) -> bool
        => SwapModelBlock { pos, block } => Bool
}

host_fn! {
    /// Set the PER-INSTANCE presentation of the model block at `pos` (any of
    /// its footprint cells): `parts` is a bitmask over the row's declared
    /// optional `parts` — bit `i` shows `parts[i]` — and `tint` multiplies the
    /// row's `tint_parts` cubes. `None` means "I am not tinting", NOT "clear
    /// the tint": the colour rides the cell's shared dye key, so a machine
    /// that never tints must not erase what a dye put there.
    ///
    /// Reach for this instead of a block row per combination whenever a
    /// machine has more than ONE independently varying visual. Swap the ROW
    /// when the block's identity changes (a lit furnace has its own emission
    /// and drops); set PARTS when the same placed machine is merely showing
    /// something different. Render only — the hitbox never moves.
    ///
    /// `false` = not a model block, or a footprint cell is unloaded.
    pub fn set_model_parts(pos: [i32; 3], parts: u32, tint: Option<[u8; 3]>) -> bool
        => SetModelParts { pos, parts, tint } => Bool
}

host_fn! {
    /// [`set_model_parts`] for many blocks in ONE crossing, `(pos, parts,
    /// tint)` per entry; the reply is parallel to `sets`.
    ///
    /// Presentation should cost one host call however many machines a player
    /// has built. At most [`crate::SIM_BATCH_MAX`] entries.
    pub fn set_model_parts_many(sets: Vec<([i32; 3], u32, Option<[u8; 3]>)>) -> Vec<bool>
        => SetModelPartsMany { sets } => Bools
}

host_fn! {
    /// Replace what this mod DRAWS on the block at `pos` with `prims`, in the
    /// BLOCK'S OWN space — for a model block its FOOTPRINT space (16 authored
    /// px = 1.0, origin at the footprint base, turned by the placed facing);
    /// otherwise the cell's `0..1`. An empty list clears it.
    ///
    /// This is the surface for anything a mod SIMULATES rather than stages.
    /// The set is retained, redrawn every frame from the replica, and costs no
    /// re-mesh, so submitting a new one every tick is the intended use: run
    /// your own liquid, your own needle, your own sliding part, and draw the
    /// result. Reach for a `.bbmodel` for the machine's fixed body, a parts
    /// mask for its discrete states, and this for what actually moves.
    ///
    /// Like a container and a parts mask, a set is keyed at the multi-cell
    /// group's ANCHOR: address the machine by whichever of its cells you have
    /// to hand and one set is stored, replaced and forgotten with the block.
    ///
    /// `false` = the cell is unloaded (or not stream-final). A clear answers
    /// `true` like any other accepted submission — it stores nothing, which is
    /// the point of it. Drawing on a block this mod does NOT own is an error
    /// (mod disabled), not a `false`; the batched form below answers `false`
    /// for that instead, because one machine broken mid-tick must not take the
    /// pack with it.
    ///
    /// At most [`crate::DRAW_PRIMS_MAX`] prims, and every coordinate must be finite;
    /// either is an error (mod disabled). A NaN would draw nothing AND defeat
    /// the engine's unchanged-submission gate, so it is refused rather than
    /// quietly dropped.
    pub fn set_block_draw(pos: [i32; 3], prims: Vec<mod_api::DrawPrim>) -> bool
        => SetBlockDraw { pos, prims } => Bool
}

host_fn! {
    /// [`set_block_draw`] for many blocks in ONE crossing; the reply is
    /// parallel to `sets`. An entry is `false` for an unloaded cell OR a block
    /// that is not this mod's — the one place the two forms differ, and
    /// deliberately: a machine broken between your read and your write is
    /// ordinary, and the single call's error would disable the pack for it.
    ///
    /// Redrawing is the per-tick idiom, so the per-block call makes a mod's
    /// tick cost proportional to how much of it the player has built. Submit
    /// the whole kind at once. At most [`crate::SIM_BATCH_MAX`] sets, each bounded and
    /// finite-checked exactly like [`set_block_draw`] — one bad set is an
    /// error for the whole call.
    pub fn set_block_draws(sets: Vec<([i32; 3], Vec<mod_api::DrawPrim>)>) -> Vec<bool>
        => SetBlockDraws { sets } => Bools
}

host_fn! {
    /// Carry `points` from the BLOCK'S OWN space at `pos` — the very space
    /// [`set_block_draw`] prims are authored in — into world coordinates,
    /// parallel to the request. `None` = the cell is unloaded or its streamed
    /// content is not final (retry later).
    ///
    /// Reach for this the moment a mod needs a WORLD position off its own
    /// model: where a product pops out, where a spout points, where a seat
    /// sits. Deriving it from the anchor plus an offset works at one facing
    /// and puts the thing inside the machine at the other three, and
    /// re-deriving the placement transform mod-side is the same rule written
    /// twice.
    ///
    /// At most [`crate::SIM_BATCH_MAX`] points per call.
    pub fn block_local_to_world(pos: [i32; 3], points: Vec<[f32; 3]>) -> Option<Vec<[f32; 3]>>
        => BlockLocalToWorld { pos, points } => Points
}

host_fn! {
    /// Batched [`set_block`]; returns how many cells were actually set. Each write
    /// still pays its own relight/remesh — batch the ABI crossing, not a floodfill.
    /// At most [`crate::SIM_BATCH_MAX`] writes per call; more disables the mod.
    pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> u64 => SetBlocks { blocks } => U64
}

host_fn! {
    /// Run the cell's block behavior `scheduled_tick` in `delay` game ticks (first
    /// schedule per cell wins).
    pub fn schedule_tick(pos: [i32; 3], delay: u64) => ScheduleTick { pos, delay }
}

host_fn! {
    /// The placed model-block GROUP at `pos` (any of its cells): the group's
    /// base cell and placement facing — map your own footprint-space data
    /// (seat layouts, machine fronts) into the world with
    /// [`crate::footprint_local_to_world`]. `None` = no model group there or
    /// the cell is unloaded.
    pub fn block_model_group(pos: [i32; 3]) -> Option<ModelGroupData>
        => BlockModelGroup { pos } => ModelGroup
}

host_fn! {
    /// Whether the section owning the cell is currently loaded AND its streamed
    /// content is final (see [`get_block`] — a section mid-stream reads as not
    /// loaded).
    pub fn is_loaded(pos: [i32; 3]) -> bool => IsLoaded { pos } => Bool
}

host_fn! {
    /// Cached light at a loaded cell on the 6-bit `0..=63` scale
    /// (`combined = max(sky, block)`). `None` = section unloaded or its
    /// streamed content not yet final — the [`get_block`] contract: state
    /// frozen, retry later. Never fabricated open-sky values, so light-driven
    /// policy can trust every `Some`.
    pub fn light_at(pos: [i32; 3]) -> Option<LightData> => LightAt { pos } => Light
}

host_fn! {
    /// The collision-shape CLASS of the cell — generic physics with no
    /// gameplay policy: [`CollisionShape::Full`] = exactly one collision box
    /// spanning the whole unit cell, [`CollisionShape::Partial`] = any other
    /// non-empty box set (stairs, slabs, doors, snow layers, model blocks),
    /// [`CollisionShape::Empty`] = no collision boxes (air, water, tall
    /// grass). `None` = unloaded / streamed content not final (retry later).
    /// Compose spawn/placement rules on top in mod code, e.g. "full solid
    /// footing" = `Full` + the block is not water + not a `petramond:leaves`
    /// tag member ([`crate::blocks_by_tag`]).
    pub fn collision_shape_at(pos: [i32; 3]) -> Option<CollisionShape>
        => CollisionShapeAt { pos } => CollisionShape
}

host_fn! {
    /// The loaded column's biome id at world `pos = [x, z]` (vocabulary:
    /// [`mod_api::biome`]), or `None` when the chunk is unloaded. Biomes are
    /// column-level data fixed at generation.
    pub fn biome_at(pos: [i32; 2]) -> Option<u8> => BiomeAt { pos } => MaybeByte
}

host_fn! {
    /// The Y of the topmost movement-blocking block of the loaded column at
    /// world `pos = [x, z]` — real footing; walk-through cover (tall grass,
    /// snow layers, water) is skipped. `None` = unloaded, all-air, or the
    /// footing is not yet stream-final (treat as "retry later"). A saved
    /// build higher in the column that has not streamed in yet is invisible
    /// to this scan — answers are provisional during join streaming.
    pub fn surface_y_at(pos: [i32; 2]) -> Option<i32> => SurfaceYAt { pos } => MaybeI32
}

host_fn! {
    /// The first block along the ray from `from` in direction `dir` (any
    /// length) within `max` blocks (`0 < max <= 64`), stopping on what
    /// `filter` says: [`RayFilter::Selectable`] is the crosshair's rule
    /// (plants stop it), [`RayFilter::Collidable`] a body's (only cells with
    /// collision boxes). Unloaded cells read as air, like the crosshair's own
    /// ray. THE line-of-sight primitive — a swung weapon reaching for a
    /// body, a projectile's flight, an AI's sightline. `None` = nothing
    /// within `max`.
    pub fn raycast(from: [f32; 3], dir: [f32; 3], max: f32, filter: RayFilter)
        -> Option<RaycastHitData>
        => Raycast { from, dir, max, filter } => Raycast
}
