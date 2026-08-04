//! The ABI's numeric bounds, in the crate BOTH sides depend on.
//!
//! Every one of these is a contract a mod has to obey and the host enforces
//! with a [`HostRet::Error`](crate::HostRet::Error) — which the SDK turns into
//! a guest panic and the host into a disabled mod. So a mod that wants to stay
//! inside a bound has to know its value, and the only way it can know it
//! without guessing is to read it from here.
//!
//! They live in this crate rather than beside their host-side guards because
//! the alternative is what this replaced: the number spelled as a literal in
//! the engine's guard, again in every SDK doc comment that mentioned it, and
//! again as a private `const` in each mod crate that pages a batch — under a
//! different name each time (`PAGE`, `ABI_BATCH_MAX`). Raising a bound then
//! meant finding every copy, and the copy nobody found is a mod that keeps
//! chunking at the old number, or worse, one that no longer does.

/// Element cap for every batched sim/registry call (`GetBlocks`, `SetBlocks`,
/// `ContainerGetMany`, `ContainerSet` slots, `SectionKvGetMany`/`SetMany`,
/// `SetModelPartsMany`, `SetBlockDraws`, the `*Names` reverse resolvers,
/// `ChatSend` targets).
///
/// A mod SPLITS at this rather than truncating: over-cap is an error, not a
/// short reply, so "the player built more machines than the cap" must never be
/// a way to lose a pack.
pub const SIM_BATCH_MAX: usize = 4096;

/// Cell cap for the `FindBlocks` box scan — the same bounded-host-work
/// doctrine as [`SIM_BATCH_MAX`], but a VOLUME bound: the scan pays per cell,
/// not per element. 32³ comfortably covers radius-8..15 neighbourhood searches
/// (17³ = 4913).
pub const FIND_BLOCKS_VOLUME_MAX: i64 = 32 * 32 * 32;

/// Per-entry limits for the mod KV surfaces (world / section-cell / mob).
pub const KV_MAX_KEY_BYTES: usize = 256;
/// A single KV value's byte cap. A mod storing a list longer than this must
/// SHARD it across keys — the cap is per value, not per key.
pub const KV_MAX_VALUE_BYTES: usize = 64 * 1024;

/// Distinct KV keys ON ONE CELL. Every `BlockDelta` (including per-recipient
/// corrective syncs) ships the cell's WHOLE KV map, so an unbounded key count
/// would make each delta of that cell arbitrarily expensive. Overwrites of an
/// existing key always pass; only a NEW key beyond the cap errors.
pub const CELL_KV_MAX_KEYS: usize = 16;

/// A mod event's payload bound (`EmitEvent`). Same order as a KV value: the
/// post queue holds these until the next drain point, and an event is a
/// notification, not a transport.
pub const EVENT_MAX_DATA_BYTES: usize = 8 * 1024;

/// Prims one block's draw set may hold. Small on purpose: this is a machine's
/// moving parts, not a mesh format — a mod that needs hundreds of boxes wants
/// a `.bbmodel`, which is baked once instead of shipped every change.
pub const DRAW_PRIMS_MAX: usize = 32;

/// Largest width or height (in pixels) of a pack GUI image sheet — the
/// document-local art a `*.gui.json` names on its `image`/image-backed
/// `button` nodes. The same ceiling the client image calls enforce: a sheet
/// uploads as ONE texture and the GPU slot budget is shared, so GUI art obeys
/// the client image side cap rather than growing a second limit for the same
/// budget.
pub const GUI_IMAGE_MAX_SIDE: u32 = 640;

/// Frame cap for one pack GUI image sheet's `frames` grid (`cols * rows`).
/// A sheet is a strip of states — a smelting flame, a charge meter — indexed
/// by `bind.frame` or cycled by `fps`, not a general animation format; a mod
/// that needs more frames wants a second sheet.
pub const GUI_IMAGE_MAX_FRAMES: u32 = 64;
