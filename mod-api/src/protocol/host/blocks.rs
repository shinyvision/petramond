//! The live world's cells: block reads and writes, light, columns, collision,
//! placed-block presentation, raycasts, and the block change feed.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::RayFilter;
use crate::ids::BlockId;
use crate::legality::prelude::*;

host_domain! {
    /// The live world's cells: block reads and writes, light, columns, collision,
    /// placed-block presentation, raycasts, and the block change feed.
    BlockCall {
        /// The block at a world cell: `Some` (air included) when its section is
        /// loaded, `None` when unloaded / outside the vertical range.
        /// → [`HostRet::Block`](crate::HostRet::Block).
        GetBlock {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Batched [`BlockCall::GetBlock`], one result per position in order.
        /// At most 4096 positions per call (the sim batch cap — see "Batch
        /// bounds" on [`HostCall`](crate::HostCall)); more is [`HostRet::Err`](crate::HostRet::Err).
        /// → [`HostRet::Blocks`](crate::HostRet::Blocks).
        GetBlocks {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
        /// Set one block through the engine's full edit path (relight, neighbour
        /// updates, `block` events' world state all hold). `false` = the cell is
        /// unloaded / out of range. → [`HostRet::Bool`](crate::HostRet::Bool).
        SetBlock {
            pos: [i32; 3],
            block: BlockId,
        } => legal(SERVER, Sim, Write),
        /// Batched [`BlockCall::SetBlock`]; applied in order, each through the full
        /// edit path. NOTE: every write pays its own relight/remesh of the 3×3×3
        /// section neighbourhood — huge batches are expensive; this batches the
        /// ABI crossing, not the world work. At most 4096 writes per call (the
        /// sim batch cap); more is [`HostRet::Err`](crate::HostRet::Err).
        /// → [`HostRet::U64`](crate::HostRet::U64) (cells actually set).
        SetBlocks {
            blocks: Vec<([i32; 3], BlockId)>,
        } => legal(SERVER, Sim, Write),
        /// Run the cell's block behavior `scheduled_tick` in `delay` game ticks
        /// (first schedule per cell wins, like water's flow checks).
        /// → [`HostRet::Unit`](crate::HostRet::Unit).
        ScheduleTick {
            pos: [i32; 3],
            delay: u64,
        } => legal(SERVER, Sim, Write),
        /// Whether the section owning the cell is loaded. → [`HostRet::Bool`](crate::HostRet::Bool).
        IsLoaded {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Cached light at a cell on the renderer's 6-bit scale (`0..=63`):
        /// combined = max(sky, block). `None` = section unloaded / streamed
        /// content not yet final — the [`BlockCall::GetBlock`] contract ("state
        /// frozen, retry later"), never a fabricated open-sky fallback, so
        /// light-driven policy can never act on values the world does not hold.
        /// → [`HostRet::Light`](crate::HostRet::Light).
        LightAt {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Swap the placed block at `pos` to the row `block` IN PLACE — the same
        /// placed thing changing costume (a machine's lit/unlit variants, a rail
        /// turning to meet a neighbour). Everything the cell owns survives: the
        /// engine-backed container, per-cell state and facing, section cell KV,
        /// and for a multi-cell MODEL group (any of its cells) the whole placed
        /// footprint, which the new row must share exactly. The region relights
        /// (an emission difference glows like a furnace lighting); no placement
        /// event fires — this is not a placement. BOTH blocks must be registered
        /// to THIS mod's namespace. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = unloaded,
        /// a model group swapped to a non-model row or a footprint mismatch).
        SwapBlock {
            pos: [i32; 3],
            block: BlockId,
        } => legal(SERVER, Sim, Write),
        /// The loaded column's biome id at world `pos = [x, z]` (vocabulary:
        /// [`crate::biome`]). `None` = column unloaded. → [`HostRet::MaybeByte`](crate::HostRet::MaybeByte).
        BiomeAt {
            pos: [i32; 2],
        } => legal(SERVER, Sim, Read),
        /// The Y of the topmost movement-blocking block of the loaded column at
        /// world `pos = [x, z]` — real footing; anything without collision boxes
        /// (tall grass, any fluid) is skipped. `None` = unloaded, all-air column, or
        /// the found footing is not yet STREAM-FINAL (retry later, like a block
        /// read). Caveat: finality is checked at the found cell — a saved build
        /// HIGHER in the column that has not streamed in yet is not visible to
        /// this scan, so treat the answer as provisional during join streaming.
        /// → [`HostRet::MaybeI32`](crate::HostRet::MaybeI32).
        SurfaceYAt {
            pos: [i32; 2],
        } => legal(SERVER, Sim, Read),
        /// The collision-shape CLASS of the cell at `pos` — generic physics, no
        /// gameplay policy: [`CollisionShape::Full`](crate::CollisionShape::Full) = exactly one collision box
        /// spanning the whole unit cell, [`CollisionShape::Partial`](crate::CollisionShape::Partial) = any other
        /// non-empty box set (stairs, slabs, doors, snow layers, model blocks),
        /// [`CollisionShape::Empty`](crate::CollisionShape::Empty) = no collision boxes (air, any fluid, tall
        /// grass). `None` = section unloaded / streamed content not yet final
        /// (the [`BlockCall::GetBlock`] contract: state frozen, retry later).
        /// Spawn/placement rules compose on top in mod code — e.g. "full solid
        /// footing" = `Full` + the block is not in
        /// [`RegistryCall::BlocksByTag`](crate::RegistryCall::BlocksByTag)`("petramond:leaves")`.
        /// → [`HostRet::CollisionShape`](crate::HostRet::CollisionShape).
        CollisionShapeAt {
            pos: [i32; 3],
        } => legal(SERVER, Sim, Read),
        /// Every cell in the INCLUSIVE box `min..=max` currently holding one of
        /// `blocks`, resolved host-side in one scan (never page a box through
        /// [`BlockCall::GetBlocks`] to search it). Positions come back in scan
        /// order — ascending `y`, then `z`, then `x` — so "the nearest match" is
        /// the caller's own fold over a deterministic list. The box is capped at
        /// 32768 cells (32³) and `blocks` at the sim batch cap; an inverted box
        /// (`min > max` on any axis) is an error. Reads are stream-final like
        /// [`BlockCall::GetBlock`]: ANY unreadable cell in the box makes the whole
        /// reply `None` (state frozen, retry later) — a partial search would let
        /// policy act on terrain a saved overlay is about to replace. The one
        /// exception: cells OUTSIDE the world's vertical range are definitionally
        /// empty, so the scan clamps to it instead of gating (a box poking past
        /// the world's top must not starve a search forever).
        /// → [`HostRet::FoundBlocks`](crate::HostRet::FoundBlocks).
        FindBlocks {
            min: [i32; 3],
            max: [i32; 3],
            blocks: Vec<BlockId>,
        } => legal(SERVER, Sim, Read),
        /// Set the PER-INSTANCE presentation state of the model block at `pos`
        /// (any of its footprint cells): `parts` is a bitmask over the row's
        /// declared optional `parts` list — bit `i` shows `parts[i]` — and `tint`
        /// is the multiply colour the row's `tint_parts` cubes take. `None` means
        /// "I am not tinting", NOT "clear the tint": the colour rides the cell's
        /// shared dye key, so a machine that never tints must not erase what a dye
        /// put there. → [`HostRet::Bool`](crate::HostRet::Bool) (`false` = not a model block, or a
        /// footprint cell is unloaded).
        ///
        /// This is the fine-grained sibling of
        /// [`SwapBlock`](Self::SwapBlock), and it exists because
        /// enumerating rows does not scale past ONE varying thing. A machine with
        /// several INDEPENDENT visual states — the forge's basin holds any of five
        /// moulds, with or without metal in it, while its fire is lit or not — is
        /// 48 block rows enumerated and one row with a mask. Swap the ROW when the
        /// block's identity changes (a lit furnace is a different row: different
        /// emission, different drops); set PARTS when the same placed machine is
        /// merely showing something different.
        ///
        /// RENDER ONLY: collision, selection and light stay the row's, so a
        /// machine's hitbox never changes under the player. State rides the cell
        /// KV lane, so it replicates, persists, and dies with the block.
        SetModelParts {
            pos: [i32; 3],
            parts: u32,
            tint: Option<[u8; 3]>,
        } => legal(SERVER, Sim, Write),
        /// Replace what this mod DRAWS on the block at `pos` with `prims`
        /// ([`DrawPrim`](crate::DrawPrim), in the block's own space). An empty
        /// list clears it. → [`HostRet::Bool`](crate::HostRet::Bool), where `false` means the cell is
        /// UNLOADED (or not stream-final) — a clear is an accepted submission and
        /// answers `true`. A block that is not this mod's answers `false` too,
        /// here and per entry in the batched form: what stands at a position is
        /// the world's to say and changes under a mod (a machine someone just
        /// broke, a position remembered from a save), so it is never an error.
        ///
        /// The set is RETAINED and redrawn every frame from the replica, and it
        /// costs NO re-mesh — which is the whole point. A block row swap or a
        /// parts mask stages a picture; this draws one, so a mod can SIMULATE
        /// what it shows (liquid running down a channel, a level rising) and
        /// submit the result at tick rate without touching chunk geometry.
        ///
        /// Bounded at [`DRAW_PRIMS_MAX`](crate::DRAW_PRIMS_MAX) prims per block,
        /// and every coordinate must be FINITE — a NaN draws nothing and defeats
        /// the engine's unchanged-submission gate (`NaN != NaN`), so it is an
        /// error rather than a quietly dropped box.
        SetBlockDraw {
            pos: [i32; 3],
            prims: Vec<crate::DrawPrim>,
        } => legal(SERVER, Sim, Write),
        /// Carry `points`, in the BLOCK'S OWN space at `pos`, into WORLD
        /// coordinates — reply parallel to the request. `None` = the cell is
        /// unloaded or its streamed content is not final (retry later), the same
        /// gate every other mod read answers on.
        ///
        /// It is the same space [`SetBlockDraw`](Self::SetBlockDraw) prims are
        /// authored in: for a model block its FOOTPRINT space (16 authored px =
        /// 1.0, origin at the footprint base, turned by the placed facing), and
        /// otherwise the cell's `0..1`. A mod that already computes geometry
        /// against its model — a spout, a ledge, the point a product pops out of —
        /// asks HERE rather than re-deriving the placement transform, which is a
        /// rule that then exists twice and only agrees at one of four facings.
        ///
        /// Bounded batch ([`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX) points per
        /// call). → [`HostRet::Points`](crate::HostRet::Points).
        BlockLocalToWorld {
            pos: [i32; 3],
            points: Vec<[f32; 3]>,
        } => legal(SERVER, Sim, Read),
        /// [`SetBlockDraw`](Self::SetBlockDraw) for MANY blocks in one crossing —
        /// the form a mod with more than one placed machine wants. Reply parallel
        /// to `sets`, each entry as the single call's ([`HostRet::Bools`](crate::HostRet::Bools)).
        ///
        /// This exists because the per-block call makes a mod's cost per TICK
        /// proportional to how much of it the player has built: a hundred machines
        /// is a hundred wasm→host crossings, every tick, for a submission the
        /// engine usually drops as unchanged. Presentation is exactly the kind of
        /// work that should cost one crossing however much of it there is.
        ///
        /// Bounded batch (4096 sets), each set bounded and finite-checked like the
        /// single call. One bad set is an error for the WHOLE call, like every
        /// other batched write.
        ///
        /// [`SetBlockDraw`]: Self::SetBlockDraw
        SetBlockDraws {
            sets: Vec<([i32; 3], Vec<crate::DrawPrim>)>,
        } => legal(SERVER, Sim, Write),
        /// [`SetModelParts`](Self::SetModelParts) for many blocks in one crossing —
        /// `(pos, parts, tint)` per entry. Reply parallel to `sets`
        /// ([`HostRet::Bools`](crate::HostRet::Bools)). Bounded batch (4096).
        SetModelPartsMany {
            sets: Vec<([i32; 3], u32, Option<[u8; 3]>)>,
        } => legal(SERVER, Sim, Write),
        /// The first block along the ray from `from` in direction `dir`
        /// (any length, normalised host-side) within `max` blocks (finite,
        /// `0 < max <= 64`), stopping on what `filter` says — the crosshair's
        /// selection rule or a body's collision rule ([`RayFilter`]). Unloaded
        /// cells read as air, like the crosshair's own ray. The line-of-sight
        /// primitive: a swung weapon reaching for a body, a projectile's flight,
        /// an AI's sightline. `None` = nothing within `max`.
        /// → [`HostRet::Raycast`](crate::HostRet::Raycast).
        Raycast {
            from: [f64; 3],
            dir: [f32; 3],
            max: f32,
            filter: RayFilter,
        } => legal(SERVER, Sim, Read),
        /// The world's change log: every cell announced changed — a block, a
        /// fluid, a door's swing — from entry `since` on. `None` asks only where
        /// the log stands now. A mod keeping something derived from the world's
        /// cells (a survey, a route answer) follows the log instead of reading
        /// the cells again: pass the reply's `next` as the following `since`.
        /// Streaming is not a change (a section loading or leaving is not
        /// logged), and the numbering is a session's: never save it. Server
        /// only. → [`HostRet::BlockChanges`](crate::HostRet::BlockChanges).
        BlockChangesSince {
            since: Option<u64>,
        } => legal(SERVER, Sim, Read),
        /// [`BlockCall::LightAt`] for many cells in one crossing, parallel to
        /// `positions` (same gate per entry: `None` = unloaded or not yet
        /// stream-final). At most [`SIM_BATCH_MAX`](crate::SIM_BATCH_MAX)
        /// positions. → [`HostRet::Lights`](crate::HostRet::Lights).
        LightAtMany {
            positions: Vec<[i32; 3]>,
        } => legal(SERVER, Sim, Read),
    }
}
