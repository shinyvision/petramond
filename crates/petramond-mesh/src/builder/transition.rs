use crate::{face::Face, vertex::transition::Transition};
use glam::{IVec3, Vec3};
use petramond_world::{
    block::{Block, SNOW_BEDDING_REACH, SNOW_COVER_REACH},
    chunk::SECTION_SIZE,
    section::Section,
    texture_transition::Rules,
};

/// How far outside a section its mesh reads, in cells. The one-cell pad
/// covers face culling, AO, smooth light and every in-plane donor; a donor's
/// exclusion then asks for the snow cover above it and that cover's bedding
/// neighbours sideways. An edit inside these margins of a neighbouring
/// section must remesh that section too.
pub struct SamplingHalo {
    pub horizontal: usize,
    pub up: usize,
    pub down: usize,
}

const PAD_REACH: i32 = 1;

pub const SAMPLING_HALO: SamplingHalo = SamplingHalo {
    horizontal: (PAD_REACH + SNOW_BEDDING_REACH) as usize,
    up: (PAD_REACH + SNOW_COVER_REACH) as usize,
    down: PAD_REACH as usize,
};

const _: () = assert!(SAMPLING_HALO.horizontal < SECTION_SIZE / 2);

pub(super) fn needs_tint(section: &Section, rules: &Rules) -> bool {
    section.has_biome_tint_blocks() || section.blocks_iter().any(|id| rules.is_tinted_material(id))
}

pub(super) fn axes(face: Face) -> (IVec3, IVec3) {
    let q = face.quad_box([0.0; 3], [1.0; 3]).map(Vec3::from_array);
    ((q[2] - q[3]).as_ivec3(), (q[0] - q[3]).as_ivec3())
}

const NEIGHBOURS: [(i32, i32); 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (-1, 0),
    (1, 0),
    (-1, 1),
    (0, 1),
    (1, 1),
];
const CORNERS: [(usize, usize, usize); 4] = [(0, 1, 3), (2, 1, 4), (5, 3, 6), (7, 4, 6)];

pub(super) struct Context<'a> {
    pub rules: &'a Rules,
    pub block: &'a dyn Fn(i32, i32, i32) -> Block,
    pub known: &'a dyn Fn(i32, i32, i32) -> bool,
    pub blocked: &'a dyn Fn(i32, i32, i32) -> bool,
    pub covered: &'a dyn Fn(IVec3, Face) -> bool,
}

impl Context<'_> {
    pub(super) fn plan(&self, pos: IVec3, face: Face, block: u16) -> Option<Transition> {
        let memberships = self.rules.memberships(block);
        if memberships.is_empty() || (self.blocked)(pos.x, pos.y, pos.z) {
            return None;
        }
        let (u, v) = axes(face);
        let cells = NEIGHBOURS.map(|(dx, dy)| pos + u * dx + v * dy);
        let blocks = cells.map(|p| (self.block)(p.x, p.y, p.z).id());
        if blocks
            .iter()
            .all(|&b| b == block || !self.rules.is_material(b))
        {
            return None;
        }
        let normal = face.dir();
        for m in memberships {
            let set = &self.rules.sets[m.set as usize];
            let locals = blocks.map(|b| self.rules.local(m.set, b));
            if !locals
                .iter()
                .any(|&l| l != 0 && l != m.local && set.allows(m.local, l))
            {
                continue;
            }
            let mut grid = [0u8; 8];
            for (i, &local) in locals.iter().enumerate() {
                if local == 0 || (local != m.local && !set.allows(m.local, local)) {
                    continue;
                }
                let p = cells[i];
                let front = p + normal;
                if !(self.known)(p.x, p.y, p.z)
                    || !(self.known)(front.x, front.y, front.z)
                    || (self.blocked)(p.x, p.y, p.z)
                    || (self.covered)(front, face)
                {
                    continue;
                }
                grid[i] = local;
            }
            for (corner, a, b) in CORNERS {
                if grid[a] == 0 || grid[b] == 0 {
                    grid[corner] = 0;
                }
            }
            if grid.iter().any(|&l| l != 0 && l != m.local) {
                let mut slots = [m.local; 9];
                slots[1..].copy_from_slice(&grid);
                return Some(Transition {
                    set: m.set,
                    grid: slots,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests;
