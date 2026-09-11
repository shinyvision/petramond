//! Template inspection and pure connector composition; placement policy is mod-owned.

use crate::__rt::host_fn;
use crate::{StructureInfoData, StructurePlacement};

#[cfg(test)]
mod tests;

/// Expand a template's authored terrain requirements at a candidate origin.
/// The caller batches these probes with other pieces and evaluates them before
/// clipping writes. Invalid rotations, overflow and excessive work return None.
pub fn structure_probes(
    placement: &StructurePlacement,
    info: &StructureInfoData,
) -> Option<Vec<([i32; 3], crate::TerrainSpace)>> {
    rotate([0; 3], placement.turn)?;
    let count = info.requirements.iter().try_fold(0usize, |total, region| {
        let count = total.checked_add(region.cell_count()?)?;
        (count <= crate::STRUCTURE_PROBES_MAX).then_some(count)
    })?;
    let mut probes = Vec::with_capacity(count);
    for region in &info.requirements {
        for y in region.min[1]..=region.max[1] {
            for z in region.min[2]..=region.max[2] {
                for x in region.min[0]..=region.max[0] {
                    let mut pos = rotate([x, y, z], placement.turn)?;
                    for (axis, value) in pos.iter_mut().enumerate() {
                        *value = value.checked_add(placement.origin[axis])?;
                    }
                    probes.push((pos, region.space));
                }
            }
        }
    }
    Some(probes)
}

host_fn! {
    /// Inspect a compiled structure once during initialization.
    pub fn structure_info(key: &str) -> Option<Box<StructureInfoData>>
        => StructureInfo { key: key.into() } => StructureInfo
}

fn rotate([x, y, z]: [i32; 3], turn: u8) -> Option<[i32; 3]> {
    match turn {
        0 => Some([x, y, z]),
        1 => Some([z.checked_neg()?, y, x]),
        2 => Some([x.checked_neg()?, y, z.checked_neg()?]),
        3 => Some([z, y, x.checked_neg()?]),
        _ => None,
    }
}

/// Mate compatible connectors face to face. The caller chooses pieces and
/// rejects terrain or footprint collisions before returning the assembled plan.
pub fn connect_structure(
    parent: &StructurePlacement,
    parent_info: &StructureInfoData,
    socket: &str,
    child: &str,
    child_info: &StructureInfoData,
    plug: &str,
) -> Option<StructurePlacement> {
    let socket = parent_info.connectors.iter().find(|c| c.name == socket)?;
    let plug = child_info
        .connectors
        .iter()
        .find(|c| c.name == plug && c.kind == socket.kind)?;
    let normal = rotate(socket.normal, parent.turn)?;
    let offset = rotate(socket.pos, parent.turn)?;
    for turn in 0..4 {
        if rotate(plug.normal, turn)? != normal.map(|n| -n) {
            continue;
        }
        let plug_pos = rotate(plug.pos, turn)?;
        let mut origin = [0; 3];
        for axis in 0..3 {
            origin[axis] = parent.origin[axis]
                .checked_add(offset[axis])?
                .checked_add(normal[axis])?
                .checked_sub(plug_pos[axis])?;
        }
        return Some(StructurePlacement {
            template: child.into(),
            origin,
            turn,
        });
    }
    None
}
