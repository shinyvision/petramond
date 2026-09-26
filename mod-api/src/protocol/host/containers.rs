//! Container slots: reads, writes, admission-ruled inserts and takes,
//! transfers, holds, and recipe results.
//!
//! One arm of [`HostCall`](crate::HostCall): each call is declared with its
//! [`Legality`](crate::Legality), which is the only place its side, scope and
//! access are stated.

use crate::data::{ContainerAddress, EntityRef, ItemStackData};
use crate::legality::prelude::*;

host_domain! {
    /// Container slots: reads, writes, admission-ruled inserts and takes,
    /// transfers, holds, and recipe results.
    ContainerCall {
        /// Read every slot of the mod container at `pos` (the engine-backed item
        /// storage behind a mod GUI document's `container` role slots; multi-cell
        /// model blocks key it at the group's base cell — the `block_placed`
        /// anchor). `None` when the section is unloaded or no container exists
        /// there yet (one is created when the GUI first opens, or by the first
        /// `ContainerSet`). → [`HostRet::ContainerSlots`](crate::HostRet::ContainerSlots).
        ContainerGet {
            at: ContainerAddress,
        } => legal(SERVER, Sim, Read),
        /// Write container slots at `pos` as `(slot index, stack)` entries, batched
        /// per the message-level ABI rule. Creates/grows the container as needed
        /// (never shrinks; slot indices past the engine cap are rejected). The
        /// block at `pos` must be registered to THIS mod's namespace — a mod owns
        /// only its own blocks' containers, ANY of them, decorative or not (reads
        /// may cross namespaces). Multi-cell model blocks canonicalize to the
        /// group anchor, so writing through any footprint cell edits the one
        /// container the GUI shows. Counts past the item's stack cap are CLAMPED
        /// to it — size against `ItemInfo.max_stack` if the overflow matters.
        /// At most 4096 slot entries per call (the sim batch cap); more is
        /// [`HostRet::Err`](crate::HostRet::Err). →
        /// [`HostRet::Bool`](crate::HostRet::Bool) (`false` = unloaded, storage that is not this
        /// mod's, or an unknown item name — the batch is not applied).
        ContainerSet {
            at: ContainerAddress,
            slots: Vec<(u32, Option<ItemStackData>)>,
        } => legal(SERVER, Sim, Write),
        /// The loaded machine-processing result for one input item (by registry
        /// NAME) under a recipe `class` (`"petramond:smelting"` = the furnace's
        /// table; a mod machine names its own, e.g. `"kitchen:cooking"`), from the
        /// same layered `recipes.json` catalog engine machines cook from — any
        /// pack's rows for that class included. `None` = no recipe. →
        /// [`HostRet::ItemStack`](crate::HostRet::ItemStack).
        RecipeResult {
            class: String,
            item: String,
        } => legal(SERVER, Any, Read),
        /// Batched [`ContainerGet`](Self::ContainerGet): every listed position's
        /// container slots in ONE crossing. A machine mod's tick loop MUST read
        /// its placed machines through this (like `GetBlocks`), never loop
        /// `ContainerGet` per machine — the per-block-per-tick hot-loop rule.
        /// At most 4096 positions per call (the sim batch cap); more is
        /// [`HostRet::Err`](crate::HostRet::Err). → [`HostRet::Containers`](crate::HostRet::Containers), parallel to the
        /// positions.
        ContainerGetMany {
            addresses: Vec<ContainerAddress>,
        } => legal(SERVER, Sim, Read),
        /// Insert through the target container's slot admission rules; returns the remainder.
        ContainerInsert {
            at: ContainerAddress,
            stack: ItemStackData,
        } => legal(SERVER, Sim, Write),
        /// Take at most count from one slot of any container; returns the taken stack.
        ContainerTake {
            at: ContainerAddress,
            slot: u32,
            count: u8,
        } => legal(SERVER, Sim, Write),
        /// Move up to `count` items out of slot `slot` of `from` into `to`,
        /// through `to`'s own slot admission (filter-matching slots first,
        /// merging before filling) — a hopper step as ONE atomic move: whatever
        /// `to` refuses stays in the source slot, so no item exists in both or
        /// neither. Neither side is namespace-guarded, exactly like
        /// [`ContainerTake`](Self::ContainerTake) + [`ContainerInsert`](Self::ContainerInsert).
        /// Both containers must be readable (stream-final, the mob alive).
        /// → [`HostRet::ItemStack`](crate::HostRet::ItemStack): what actually moved (`None` = nothing).
        ContainerTransfer {
            from: ContainerAddress,
            slot: u32,
            to: ContainerAddress,
            count: u8,
        } => legal(SERVER, Sim, Write),
        /// Hold the container at `at` open (`open`) or let it go, on behalf of a
        /// live mob, exactly as a player's open screen does: what the container
        /// shows while viewed (a chest's lid lifting, with its sound) it shows
        /// while anyone holds it. A mob's holds end when it leaves the world.
        /// → [`HostRet::Bool`](crate::HostRet::Bool): `false` = no slot storage there (a carried
        /// container has nothing to show) or no such mob. Server only.
        ContainerHold {
            at: ContainerAddress,
            actor: EntityRef,
            open: bool,
        } => legal(SERVER, Sim, Write),
    }
}
