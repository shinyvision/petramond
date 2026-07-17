//! Sim-scoped world reads and writes: blocks, light, scheduled ticks,
//! model-block swaps, and spawn-support queries.

use mod_api::BlockId;

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
    /// Batched [`get_block`]: one result per position, in order.
    pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>>
        => GetBlocks { positions } => Blocks
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
    /// Batched [`set_block`]; returns how many cells were actually set. Each write
    /// still pays its own relight/remesh — batch the ABI crossing, not a floodfill.
    pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> u64 => SetBlocks { blocks } => U64
}

host_fn! {
    /// Run the cell's block behavior `scheduled_tick` in `delay` game ticks (first
    /// schedule per cell wins).
    pub fn schedule_tick(pos: [i32; 3], delay: u64) => ScheduleTick { pos, delay }
}

host_fn! {
    /// Whether the section owning the cell is currently loaded AND its streamed
    /// content is final (see [`get_block`] — a section mid-stream reads as not
    /// loaded).
    pub fn is_loaded(pos: [i32; 3]) -> bool => IsLoaded { pos } => Bool
}

host_fn! {
    /// Cached light at a cell as `(combined, sky, block)` on the 6-bit `0..=63`
    /// scale (combined = max of the two channels).
    pub fn light_at(pos: [i32; 3]) -> (u8, u8, u8)
        => LightAt { pos }
        => crate::HostRet::Light { combined, sky, block } => (combined, sky, block)
}

host_fn! {
    /// Whether the loaded block at `pos` is valid full-cube support for
    /// programmatic mob spawns. Rejects unloaded cells, water, leaves, and partial
    /// collision shapes such as stairs.
    pub fn block_is_full_spawn_support(pos: [i32; 3]) -> bool
        => BlockIsFullSpawnSupport { pos } => Bool
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
