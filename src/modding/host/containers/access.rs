//! Resolving a [`ContainerAddress`] to the storage behind it: a block's slots
//! at its anchor, or a live mob's carried slots. Every container call reads
//! and writes through here, so a block and a mob answer the same finality,
//! ownership and admission rules.

use std::sync::Arc;

use mod_api::ContainerAddress;
use petramond_math::math::IVec3;
use petramond_world::container::{Container, SlotSpec};

use crate::events::SimCtx;

/// A resolved container target, valid within the current handler.
#[derive(Clone, Copy)]
pub(super) enum Target {
    /// The block container's anchor cell.
    Block(IVec3),
    /// The live mob's list index.
    Mob(usize),
}

/// Resolve `at` for a READ: the block anchor (a loaded section), or a live
/// mob. `None` answers "nothing readable there".
pub(super) fn resolve_read(ctx: &SimCtx<'_>, at: ContainerAddress) -> Option<Target> {
    match at {
        ContainerAddress::Block(pos) => Some(Target::Block(
            ctx.world.container_anchor(IVec3::from_array(pos)),
        )),
        ContainerAddress::Mob(id) => super::super::guards::live_mob(ctx, id).map(Target::Mob),
    }
}

/// Resolve `at` for a WRITE: like [`resolve_read`], but the storage must sit
/// in stream-final terrain — a block's cell, or the cell under a mob's feet —
/// so an overlay about to land can neither clobber nor be clobbered by it.
pub(super) fn resolve_write(ctx: &SimCtx<'_>, at: ContainerAddress) -> Option<Target> {
    let target = resolve_read(ctx, at)?;
    let cell = match target {
        Target::Block(p) => p,
        Target::Mob(i) => ctx.world.mobs().instances()[i].pos.block(),
    };
    ctx.world
        .physics_cell_final_at(cell.x, cell.y, cell.z)
        .then_some(target)
}

pub(super) fn slots<'a>(ctx: &'a SimCtx<'_>, target: Target) -> Option<&'a Container> {
    match target {
        Target::Block(p) => ctx.world.container_at(p),
        Target::Mob(i) => Some(ctx.world.mobs().instances()[i].container()),
    }
}

pub(super) fn slots_mut<'a>(ctx: &'a mut SimCtx<'_>, target: Target) -> Option<&'a mut Container> {
    match target {
        Target::Block(p) => ctx.world.container_at_mut(p),
        Target::Mob(i) => ctx.world.mobs_mut().container_mut(i),
    }
}

/// The admission rules an insert routes through: a block's GUI document (or
/// the engine furnace's machine filters), sized to it and creating the
/// container on first use; plain cells for a mob. `None` = the target offers
/// no slots to insert into.
pub(super) fn insert_specs(ctx: &mut SimCtx<'_>, target: Target) -> Option<Arc<Vec<SlotSpec>>> {
    match target {
        Target::Block(p) => {
            let block = ctx.world.block_if_stream_final(p.x, p.y, p.z)?;
            let petramond_world::block::BlockInteraction::OpenGui(kind) = block.interaction()
            else {
                return None;
            };
            let specs = crate::menu::slot_specs_for_kind(kind);
            (!specs.is_empty() && ctx.world.ensure_container(p, specs.len())).then_some(specs)
        }
        Target::Mob(i) => {
            let len = ctx.world.mobs().instances()[i].container().slots.len();
            (len > 0).then(|| Arc::new(vec![SlotSpec::default(); len]))
        }
    }
}

/// Record that a target's contents changed: block containers persist with
/// their section, so the section is marked; a mob's slots ride its own record.
pub(super) fn touched(ctx: &mut SimCtx<'_>, target: Target) {
    if let Target::Block(p) = target {
        ctx.world.mark_chunk_modified(p);
    }
}

/// The registry name that owns the storage, for the write ownership guard:
/// the block row at the anchor, or the mob's species.
pub(super) fn owner_name(ctx: &SimCtx<'_>, target: Target) -> Option<&'static str> {
    match target {
        Target::Block(p) => {
            let block = ctx.world.block_if_stream_final(p.x, p.y, p.z)?;
            petramond_world::registry::names().blocks.name(block.id())
        }
        Target::Mob(i) => Some(crate::mob::def(ctx.world.mobs().instances()[i].kind).name),
    }
}
