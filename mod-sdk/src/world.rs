use mod_api::{
    BlockId, CellGroupData, CellRecord, CollisionShape, LightData, RayFilter, RaycastHitData,
    RecordStatus,
};

use crate::__rt::host_fn;

host_fn! {
    pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => GetBlocks { positions } => Blocks
}

pub fn get_block(pos: [i32; 3]) -> Option<BlockId> {
    get_blocks(vec![pos]).pop().flatten()
}

host_fn! {
    /// Each cell whole (block, state, fluid level, data, container), ready to write back with
    /// [`set_cells`] here or anywhere else.
    pub fn get_cells(positions: Vec<[i32; 3]>) -> Vec<Option<CellRecord>>
        => GetCells { positions } => Cells
}

pub fn get_cell(pos: [i32; 3]) -> Option<CellRecord> {
    get_cells(vec![pos]).pop().flatten()
}

host_fn! {
    /// Writes whole cells as one edit; a compound is listed whole. `false` = nothing was
    /// written (an unloaded cell, an incomplete compound, an unknown name).
    pub fn set_cells(cells: Vec<([i32; 3], CellRecord)>) -> bool => SetCells { cells } => Bool
}

host_fn! {
    pub fn block_changes_since(since: Option<u64>) -> mod_api::BlockChanges
        => BlockChangesSince { since } => BlockChanges
}

host_fn! {
    /// Finds every cell in the inclusive box `min..=max` holding one of `blocks`, in one host-side
    /// scan. Don't page a box through [`get_blocks`] to search it.
    ///
    /// Results come in scan order (`y`, then `z`, then `x`); fold for the nearest match yourself.
    /// The box is capped at [`crate::FIND_BLOCKS_VOLUME_MAX`] cells. If any cell isn't
    /// stream-final the whole reply is `None`, so retry later.
    pub fn find_blocks(min: [i32; 3], max: [i32; 3], blocks: Vec<BlockId>)
        -> Option<Vec<[i32; 3]>>
        => FindBlocks { min, max, blocks } => FoundBlocks
}

host_fn! {
    pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> Vec<bool> => SetBlocks { blocks } => Bools
}

pub fn set_block(pos: [i32; 3], block: BlockId) -> bool {
    set_blocks(vec![(pos, block)]).pop().unwrap_or(false)
}

host_fn! {
    pub fn swap_blocks(swaps: Vec<([i32; 3], BlockId)>) -> Vec<bool> => SwapBlocks { swaps } => Bools
}

pub fn swap_block(pos: [i32; 3], block: BlockId) -> bool {
    swap_blocks(vec![(pos, block)]).pop().unwrap_or(false)
}

host_fn! {
    /// Set the PER-INSTANCE presentation of model blocks (any of their footprint cells): `parts`
    /// is a bitmask over the row's declared optional `parts` — bit `i` shows `parts[i]` — and
    /// `tint` multiplies the row's `tint_parts` cubes. `None` means "I am not tinting", NOT "clear
    /// the tint": the colour rides the cell's shared dye key, so a machine that never tints must
    /// not erase what a dye put there.
    ///
    /// Reach for this instead of a block row per combination whenever a machine has more than ONE
    /// independently varying visual. Swap the ROW when the block's identity changes (a lit furnace
    /// has its own emission and drops); set PARTS when the same placed machine is merely showing
    /// something different. Render only — the hitbox never moves.
    ///
    /// Per entry `false` = not a model block, or a footprint cell is unloaded.
    pub fn set_model_parts_many(sets: Vec<([i32; 3], u32, Option<[u8; 3]>)>) -> Vec<bool>
        => SetModelParts { sets } => Bools
}

pub fn set_model_parts(pos: [i32; 3], parts: u32, tint: Option<[u8; 3]>) -> bool {
    set_model_parts_many(vec![(pos, parts, tint)])
        .pop()
        .unwrap_or(false)
}

host_fn! {
    pub fn set_block_draws(sets: Vec<([i32; 3], Vec<mod_api::DrawPrim>)>) -> Vec<bool>
        => SetBlockDraws { sets } => Bools
}

pub fn set_block_draw(pos: [i32; 3], prims: Vec<mod_api::DrawPrim>) -> bool {
    set_block_draws(vec![(pos, prims)]).pop().unwrap_or(false)
}

host_fn! {
    /// Carries `points` from the block's own space at `pos` (the space [`set_block_draw`] prims
    /// use) into world coordinates, in request order. `None` means the cell is unloaded or its
    /// streamed content isn't final yet, so retry later.
    ///
    /// Use this for a world position off the block's model: where a product pops out, where a
    /// spout points, where a seat sits. Anchor plus offset only works at one facing and puts the
    /// thing inside the machine at the other three. Re-deriving the placement transform mod-side
    /// writes the same rule twice.
    ///
    /// Max [`crate::SIM_BATCH_MAX`] points per call.
    pub fn block_local_to_world(pos: [i32; 3], points: Vec<[f32; 3]>) -> Option<Vec<[f64; 3]>>
        => BlockLocalToWorld { pos, points } => Points
}

host_fn! {
    /// Runs each cell's block behaviors' scheduled tick after `delay` ticks; pending ticks are
    /// saved with their section and pause while unloaded. The first pending tick wins; replacing
    /// the block cancels it, while `SwapBlock` preserves it.
    pub fn schedule_ticks(ticks: Vec<([i32; 3], u64)>) => ScheduleTicks { ticks }
}

pub fn schedule_tick(pos: [i32; 3], delay: u64) {
    schedule_ticks(vec![(pos, delay)]);
}

host_fn! {
    pub fn cell_groups(positions: Vec<[i32; 3]>) -> Vec<Option<CellGroupData>>
        => CellGroup { positions } => CellGroups
}

pub fn cell_group(pos: [i32; 3]) -> Option<CellGroupData> {
    cell_groups(vec![pos]).pop().flatten()
}

host_fn! {
    pub fn loaded_many(positions: Vec<[i32; 3]>) -> Vec<bool> => IsLoaded { positions } => Bools
}

pub fn is_loaded(pos: [i32; 3]) -> bool {
    loaded_many(vec![pos]).pop().unwrap_or(false)
}

host_fn! {
    pub fn light_at_many(positions: Vec<[i32; 3]>) -> Vec<Option<LightData>>
        => LightAt { positions } => Lights
}

pub fn light_at(pos: [i32; 3]) -> Option<LightData> {
    light_at_many(vec![pos]).pop().flatten()
}

host_fn! {
    /// Whether a block would be held at each cell by the world's support rule: `None` asks about
    /// the block there now, `Some` about that block standing there instead.
    pub fn supported_at(cells: Vec<([i32; 3], Option<BlockId>)>) -> Vec<bool>
        => SupportedAt { cells } => Bools
}

host_fn! {
    pub fn fluid_surface_at(positions: Vec<[i32; 3]>) -> Vec<Option<(BlockId, f64)>>
        => FluidSurfaceAt { positions } => FluidSurfaces
}

host_fn! {
    /// Breaks each cell as the world does when a block loses its support (bare-hand drops,
    /// container spilled, residue left in place).
    pub fn break_blocks(positions: Vec<[i32; 3]>) -> Vec<bool> => BreakBlocks { positions } => Bools
}

pub fn break_block(pos: [i32; 3]) -> bool {
    break_blocks(vec![pos]).pop().unwrap_or(false)
}

host_fn! {
    pub fn collision_shapes_at(positions: Vec<[i32; 3]>) -> Vec<Option<CollisionShape>>
        => CollisionShapeAt { positions } => CollisionShapes
}

pub fn collision_shape_at(pos: [i32; 3]) -> Option<CollisionShape> {
    collision_shapes_at(vec![pos]).pop().flatten()
}

host_fn! {
    pub fn biomes_at(columns: Vec<[i32; 2]>) -> Vec<Option<u8>> => BiomeAt { columns } => Biomes
}

pub fn biome_at(column: [i32; 2]) -> Option<u8> {
    biomes_at(vec![column]).pop().flatten()
}

host_fn! {
    pub fn surface_ys_at(columns: Vec<[i32; 2]>) -> Vec<Option<i32>>
        => SurfaceYAt { columns } => MaybeI32s
}

pub fn surface_y_at(column: [i32; 2]) -> Option<i32> {
    surface_ys_at(vec![column]).pop().flatten()
}

host_fn! {
    pub fn raycast(from: [f64; 3], dir: [f32; 3], max: f32, filter: RayFilter)
        -> Option<RaycastHitData>
        => Raycast { from, dir, max, filter } => Raycast
}

host_fn! {
    /// How each cell compares to the record wanted there.
    pub fn record_statuses(cells: Vec<([i32; 3], CellRecord)>) -> Vec<RecordStatus>
        => RecordStatuses { cells } => RecordStatuses
}

host_fn! {
    /// Grows a `features.json` feature at `pos` the way a sapling grows its tree; see
    /// [`mod_api::BlockCall::PlaceFeature`].
    pub fn place_feature(feature: &str, pos: [i32; 3], seed: u64, over: Vec<BlockId>, keep: Vec<BlockId>) -> bool
        => PlaceFeature { feature: feature.into(), pos, seed, over, keep } => Bool
}

host_fn! {
    /// Whether each world-space box is clear of collidable terrain; unreadable cells count as
    /// blocked.
    pub fn boxes_clear(boxes: Vec<([f64; 3], [f64; 3])>) -> Vec<bool> => BoxesClear { boxes } => Bools
}
