use mod_api::{BlockId, CollisionShape, LightData, ModelGroupData, RayFilter, RaycastHitData};

use crate::__rt::host_fn;
use crate::__rt::try_host_fn;

try_host_fn! {
    pub fn try_light_at_many(positions: Vec<[i32; 3]>) -> Vec<Option<LightData>>
        => LightAtMany { positions } => Lights
}

host_fn! {
    pub fn get_block(pos: [i32; 3]) -> Option<BlockId> => GetBlock { pos } => Block
}

host_fn! {
    pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => GetBlocks { positions } => Blocks
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
    pub fn set_block(pos: [i32; 3], block: BlockId) -> bool => SetBlock { pos, block } => Bool
}

host_fn! {
    pub fn swap_block(pos: [i32; 3], block: BlockId) -> bool
        => SwapBlock { pos, block } => Bool
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
    pub fn set_model_parts_many(sets: Vec<([i32; 3], u32, Option<[u8; 3]>)>) -> Vec<bool>
        => SetModelPartsMany { sets } => Bools
}

host_fn! {
    pub fn set_block_draw(pos: [i32; 3], prims: Vec<mod_api::DrawPrim>) -> bool
        => SetBlockDraw { pos, prims } => Bool
}

host_fn! {
    pub fn set_block_draws(sets: Vec<([i32; 3], Vec<mod_api::DrawPrim>)>) -> Vec<bool>
        => SetBlockDraws { sets } => Bools
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
    pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> u64 => SetBlocks { blocks } => U64
}

host_fn! {
    pub fn schedule_tick(pos: [i32; 3], delay: u64) => ScheduleTick { pos, delay }
}

host_fn! {
    pub fn block_model_group(pos: [i32; 3]) -> Option<ModelGroupData>
        => BlockModelGroup { pos } => ModelGroup
}

host_fn! {
    pub fn is_loaded(pos: [i32; 3]) -> bool => IsLoaded { pos } => Bool
}

host_fn! {
    pub fn light_at(pos: [i32; 3]) -> Option<LightData> => LightAt { pos } => Light
}

host_fn! {
    pub fn light_at_many(positions: Vec<[i32; 3]>) -> Vec<Option<LightData>>
        => LightAtMany { positions } => Lights
}

host_fn! {
    pub fn collision_shape_at(pos: [i32; 3]) -> Option<CollisionShape>
        => CollisionShapeAt { pos } => CollisionShape
}

host_fn! {
    pub fn biome_at(pos: [i32; 2]) -> Option<u8> => BiomeAt { pos } => MaybeByte
}

host_fn! {
    pub fn surface_y_at(pos: [i32; 2]) -> Option<i32> => SurfaceYAt { pos } => MaybeI32
}

host_fn! {
    pub fn raycast(from: [f64; 3], dir: [f32; 3], max: f32, filter: RayFilter)
        -> Option<RaycastHitData>
        => Raycast { from, dir, max, filter } => Raycast
}
