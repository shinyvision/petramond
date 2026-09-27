use std::sync::Arc;

use mod_api::ContainerAddress;
use petramond_math::math::IVec3;
use petramond_world::container::{Container, SlotSpec};

use crate::events::SimCtx;

#[derive(Clone, Copy)]
pub(super) enum Target {
    Block(IVec3),
    Mob(u64),
}

pub(super) fn resolve_read(ctx: &SimCtx<'_>, at: ContainerAddress) -> Option<Target> {
    match at {
        ContainerAddress::Block(pos) => Some(Target::Block(
            ctx.world.container_anchor(IVec3::from_array(pos)),
        )),
        ContainerAddress::Mob(id) => {
            super::super::guards::live_mob(ctx, id).map(|_| Target::Mob(id))
        }
    }
}

pub(super) fn resolve_write(ctx: &SimCtx<'_>, at: ContainerAddress) -> Option<Target> {
    let target = resolve_read(ctx, at)?;
    let cell = match target {
        Target::Block(p) => p,
        Target::Mob(id) => ctx.world.mobs().get(id)?.pos.block(),
    };
    ctx.world
        .physics_cell_final_at(cell.x, cell.y, cell.z)
        .then_some(target)
}

pub(super) fn slots<'a>(ctx: &'a SimCtx<'_>, target: Target) -> Option<&'a Container> {
    match target {
        Target::Block(p) => ctx.world.container_at(p),
        Target::Mob(id) => ctx.world.mobs().get(id).map(|m| m.container()),
    }
}

pub(super) fn slots_mut<'a>(ctx: &'a mut SimCtx<'_>, target: Target) -> Option<&'a mut Container> {
    match target {
        Target::Block(p) => ctx.world.container_at_mut(p),
        Target::Mob(id) => ctx.world.mobs_mut().container_mut(id),
    }
}

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
        Target::Mob(id) => {
            let len = ctx.world.mobs().get(id)?.container().slots.len();
            (len > 0).then(|| Arc::new(vec![SlotSpec::default(); len]))
        }
    }
}

pub(super) fn touched(ctx: &mut SimCtx<'_>, target: Target) {
    if let Target::Block(p) = target {
        ctx.world.mark_chunk_modified(p);
    }
}

pub(super) fn owner_name(ctx: &SimCtx<'_>, target: Target) -> Option<&'static str> {
    match target {
        Target::Block(p) => {
            let block = ctx.world.block_if_stream_final(p.x, p.y, p.z)?;
            petramond_world::registry::names().blocks.name(block.id())
        }
        Target::Mob(id) => ctx
            .world
            .mobs()
            .get(id)
            .map(|m| crate::mob::def(m.kind).name),
    }
}
