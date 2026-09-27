use crate::data::RayFilter;
use crate::ids::BlockId;
use crate::legality::prelude::*;

host_domain! {
    BlockCall {
        GetBlock {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        GetBlocks {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        SetBlock {
            pos: [i32; 3],
            block: BlockId,
        } => legal(SERVER, Sim, Write),
        SetBlocks {
            blocks: Vec<([i32; 3], BlockId)>,
        } => legal(SERVER, Sim, Write),
        ScheduleTick {
            pos: [i32; 3],
            delay: u64,
        } => legal(SERVER, Sim, Write),
        IsLoaded {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        LightAt {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        SwapBlock {
            pos: [i32; 3],
            block: BlockId,
        } => legal(SERVER, Sim, Write),
        BiomeAt {
            pos: [i32; 2],
        } => legal(SERVER, Sim, Read),
        SurfaceYAt {
            pos: [i32; 2],
        } => legal(SERVER, Sim, Read),
        CollisionShapeAt {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Finds every cell in the inclusive box `min..=max` matching one of `blocks`, scanned
        /// host-side in one pass (don't page through [`BlockCall::GetBlocks`] to search).
        /// Order is ascending y, then z, then x, so "nearest match" is just the caller folding over
        /// that list.
        /// Box capped at 32768 cells (32³), `blocks` at the sim batch cap. Inverted box is an
        /// error.
        /// Same stream-final rule as [`BlockCall::GetBlock`]: any unreadable cell makes the whole
        /// reply `None`, so a partial search can't race a saved overlay.
        /// Exception: cells outside the world's vertical range count as empty and get clamped, not
        /// gated, or a box poking past the top would stall forever.
        /// → [`HostRet::FoundBlocks`](crate::HostRet::FoundBlocks).
        FindBlocks {
            min: [i32; 3],
            max: [i32; 3],
            blocks: Vec<BlockId>,
        } => legal(SERVER, Sim, Read),
        SetModelParts {
            pos: [i32; 3],
            parts: u32,
            tint: Option<[u8; 3]>,
        } => legal(SERVER, Sim, Write),
        SetBlockDraw {
            pos: [i32; 3],
            prims: Vec<crate::DrawPrim>,
        } => legal(SERVER, Sim, Write),
        BlockLocalToWorld {
            pos: [i32; 3],
            points: Vec<[f32; 3]>,
        } => legal(SERVER, Sim, Read),
        SetBlockDraws {
            sets: Vec<([i32; 3], Vec<crate::DrawPrim>)>,
        } => legal(SERVER, Sim, Write),
        SetModelPartsMany {
            sets: Vec<([i32; 3], u32, Option<[u8; 3]>)>,
        } => legal(SERVER, Sim, Write),
        Raycast {
            from: [f64; 3],
            dir: [f32; 3],
            max: f32,
            filter: RayFilter,
        } => legal(SERVER_CLIENT, Sim, Read),
        BlockChangesSince {
            since: Option<u64>,
        } => legal(SERVER, Sim, Read),
        LightAtMany {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
    }
}
