use petramond_math::math::IVec3;
use petramond_world::fluid::FluidDef;

use super::{block_at, fluid_of, World, CARDINALS, DOWN, UP};

/// Above/side quench contact around `pos`: the cell solidifies when its
/// quencher sits above or beside it, and a quenching neighbour below or beside
/// solidifies when this cell is its quencher. Downward pours wait for
/// [`react_to_downward_flow`].
pub(super) fn react(world: &mut World, pos: IVec3, fluid: &'static FluidDef) {
    if let Some(q) = fluid.quench {
        if [UP]
            .into_iter()
            .chain(CARDINALS)
            .any(|d| block_at(world, pos + d) == q.by)
        {
            world.set_block_world(pos.x, pos.y, pos.z, q.result);
            return;
        }
    }
    // A fluid above must enter this cell on its own downward flow step.
    for d in [DOWN].into_iter().chain(CARDINALS) {
        let target = pos + d;
        if let Some(q) = fluid_of(block_at(world, target)).and_then(|f| f.quench) {
            if q.by == fluid.block {
                world.set_block_world(target.x, target.y, target.z, q.result);
            }
        }
    }
}

/// What a fluid's downward flow step did at the cell below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DownwardContact {
    /// The cell below does not quench this fluid: flow as usual.
    None,
    /// The receiving cell solidified; the flow step is spent.
    Reacted,
    /// The receiving cell quenches this fluid but refused the write.
    Refused,
}

pub(super) fn react_to_downward_flow(
    world: &mut World,
    target: IVec3,
    fluid: &'static FluidDef,
) -> DownwardContact {
    let Some(q) = fluid.quench else {
        return DownwardContact::None;
    };
    let Some(receiving) = fluid_of(block_at(world, target)) else {
        return DownwardContact::None;
    };
    if receiving.block != q.by {
        return DownwardContact::None;
    }
    // Consuming a fluid must not erase its other contacts before their updates.
    react(world, target, receiving);
    if world.set_block_world(target.x, target.y, target.z, q.result) {
        DownwardContact::Reacted
    } else {
        DownwardContact::Refused
    }
}
