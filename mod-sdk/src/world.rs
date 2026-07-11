//! Sim-scoped world reads and writes: blocks, light, scheduled ticks,
//! model-block swaps, and spawn-support queries.

use mod_api::{BlockId, HostCall, HostRet};

use crate::__rt;

/// The block at a world cell, or `None` when its section is unloaded, still
/// STREAMING IN (a gen job or the player's saved record has not finished
/// landing — reading the half-streamed content would lie), or the cell is
/// outside the world's vertical range. Treat `None` as "state frozen, retry
/// later"; never as evidence about what the cell holds. Air is
/// `Some(BlockId(0))`.
pub fn get_block(pos: [i32; 3]) -> Option<BlockId> {
    match __rt::host_call(&HostCall::GetBlock { pos }) {
        HostRet::Block(b) => b,
        other => panic!("GetBlock returned {other:?}"),
    }
}

/// Batched [`get_block`]: one result per position, in order.
pub fn get_blocks(positions: Vec<[i32; 3]>) -> Vec<Option<BlockId>> {
    match __rt::host_call(&HostCall::GetBlocks { positions }) {
        HostRet::Blocks(b) => b,
        other => panic!("GetBlocks returned {other:?}"),
    }
}

/// Set one block through the engine's full edit path (relight, neighbour
/// updates). Returns `false` when the cell is unloaded / out of range.
pub fn set_block(pos: [i32; 3], block: BlockId) -> bool {
    match __rt::host_call(&HostCall::SetBlock { pos, block }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SetBlock returned {other:?}"),
    }
}

/// Swap the placed multi-cell MODEL block group at `pos` (any of its cells) to
/// `block` — another model block sharing the exact same oriented footprint
/// (e.g. a machine's lit/unlit variants). Its container, facing, and section
/// cell KV survive; the region relights (emission differences glow). Both
/// blocks must be this mod's own. `false` = no model group there, footprint
/// mismatch, or unloaded.
pub fn swap_model_block(pos: [i32; 3], block: BlockId) -> bool {
    match __rt::host_call(&HostCall::SwapModelBlock { pos, block }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SwapModelBlock returned {other:?}"),
    }
}

/// Batched [`set_block`]; returns how many cells were actually set. Each write
/// still pays its own relight/remesh — batch the ABI crossing, not a floodfill.
pub fn set_blocks(blocks: Vec<([i32; 3], BlockId)>) -> u64 {
    match __rt::host_call(&HostCall::SetBlocks { blocks }) {
        HostRet::U64(n) => n,
        other => panic!("SetBlocks returned {other:?}"),
    }
}

/// Run the cell's block behavior `scheduled_tick` in `delay` game ticks (first
/// schedule per cell wins).
pub fn schedule_tick(pos: [i32; 3], delay: u64) {
    __rt::expect_unit(
        "ScheduleTick",
        __rt::host_call(&HostCall::ScheduleTick { pos, delay }),
    );
}

/// Whether the section owning the cell is currently loaded AND its streamed
/// content is final (see [`get_block`] — a section mid-stream reads as not
/// loaded).
pub fn is_loaded(pos: [i32; 3]) -> bool {
    match __rt::host_call(&HostCall::IsLoaded { pos }) {
        HostRet::Bool(loaded) => loaded,
        other => panic!("IsLoaded returned {other:?}"),
    }
}

/// Cached light at a cell as `(combined, sky, block)` on the 6-bit `0..=63`
/// scale (combined = max of the two channels).
pub fn light_at(pos: [i32; 3]) -> (u8, u8, u8) {
    match __rt::host_call(&HostCall::LightAt { pos }) {
        HostRet::Light {
            combined,
            sky,
            block,
        } => (combined, sky, block),
        other => panic!("LightAt returned {other:?}"),
    }
}

/// Whether the loaded block at `pos` is valid full-cube support for
/// programmatic mob spawns. Rejects unloaded cells, water, leaves, and partial
/// collision shapes such as stairs.
pub fn block_is_full_spawn_support(pos: [i32; 3]) -> bool {
    match __rt::host_call(&HostCall::BlockIsFullSpawnSupport { pos }) {
        HostRet::Bool(ok) => ok,
        other => panic!("BlockIsFullSpawnSupport returned {other:?}"),
    }
}
