//! Section occlusion culling: a flood over the section visibility graph from
//! the camera's section, on top of frustum and fog culling.
//!
//! Each section carries which of its faces see each other through it
//! ([`SectionVisibility`], computed on the mesh worker). A sight line from the
//! camera crosses a chain of face-adjacent sections, entering and leaving each
//! through connected faces, and moves away from the camera along every axis it
//! moves on. The flood follows exactly those chains — per section AND entry
//! face, so no path is lost to an earlier arrival — through sections inside
//! the view volume; a section it never reaches is behind rock from every
//! sight line and is not drawn. Sections without a record (air, unloaded,
//! unmeshed) are fully connected, which can only draw more.

use petramond_mesh::face::Face;
use petramond_mesh::SectionVisibility;
use petramond_world::chunk::{ChunkPos, SectionPos};
use rustc_hash::FxHashMap;

struct ColumnVisibility {
    min_cy: i32,
    first: u32,
    len: u32,
}

const ANY_ENTRY: u8 = 1 << 6;
const NO_ENTRY: u8 = u8::MAX;
const CAMERA: u8 = 1 << 7;
const ADMIT_KNOWN: u16 = 1 << 8;
const ADMITTED: u16 = 1 << 9;

/// The flood's working set: a dense box of sections around the camera (x/z within the caller's
/// reach, y across the loaded span), so a visit is array indexing instead of hash probes. `state`
/// is only meaningful where `stamp` equals the current flood's `generation`, which clears the
/// whole box in O(1) per flood.
#[derive(Default)]
struct FloodGrid {
    min: [i32; 3],
    size: [i32; 3],
    visibility: Vec<SectionVisibility>,
    stamp: Vec<u32>,
    state: Vec<u16>,
    generation: u32,
}

impl FloodGrid {
    #[inline]
    fn index(&self, pos: SectionPos) -> Option<usize> {
        let x = pos.cx.wrapping_sub(self.min[0]) as u32;
        let y = pos.cy.wrapping_sub(self.min[1]) as u32;
        let z = pos.cz.wrapping_sub(self.min[2]) as u32;
        let [sx, sy, sz] = self.size.map(|s| s as u32);
        (x < sx && y < sy && z < sz).then(|| ((z * sx + x) * sy + y) as usize)
    }

    #[inline]
    fn state(&self, index: usize) -> u16 {
        if self.stamp[index] == self.generation {
            self.state[index]
        } else {
            0
        }
    }

    /// Invalidates every cell's state at once.
    fn next_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.stamp.fill(0);
            self.generation = 1;
        }
    }

    #[inline]
    fn set_state(&mut self, index: usize, state: u16) {
        self.stamp[index] = self.generation;
        self.state[index] = state;
    }
}

pub(crate) struct SectionOcclusion {
    columns: FxHashMap<ChunkPos, ColumnVisibility>,
    visibility: Vec<SectionVisibility>,
    cy_bounds: (i32, i32),
    grid: FloodGrid,
    grid_stale: bool,
    /// Reached (section, entry face) states still to expand. Reachability does not depend on
    /// the visiting order, so this is a stack.
    queue: Vec<(u32, [i32; 3], u8)>,
}

impl Default for SectionOcclusion {
    fn default() -> Self {
        Self {
            columns: FxHashMap::default(),
            visibility: Vec::new(),
            cy_bounds: (i32::MAX, i32::MIN),
            grid: FloodGrid::default(),
            grid_stale: true,
            queue: Vec::new(),
        }
    }
}

/// The exit faces (bit `Face as u8`) of the box cell at `pos` that keep moving away from the camera
/// on every axis — see [`moves_away`] — and stay inside the `size` box.
#[inline]
fn exits_away_inside(pos: [i32; 3], camera: [i32; 3], size: [i32; 3]) -> u8 {
    let mut exits = 0u8;
    for axis in 0..3 {
        let (positive, negative) = (1u8 << (axis * 2), 1u8 << (axis * 2 + 1));
        if pos[axis] >= camera[axis] && pos[axis] + 1 < size[axis] {
            exits |= positive;
        }
        if pos[axis] <= camera[axis] && pos[axis] > 0 {
            exits |= negative;
        }
    }
    exits
}

#[cfg(test)]
fn moves_away(camera: SectionPos, pos: SectionPos, exit: Face) -> bool {
    let glam::IVec3 {
        x: dx,
        y: dy,
        z: dz,
    } = exit.dir();
    (pos.cx - camera.cx) * dx >= 0
        && (pos.cy - camera.cy) * dy >= 0
        && (pos.cz - camera.cz) * dz >= 0
}

impl SectionOcclusion {
    pub fn clear(&mut self) {
        self.columns.clear();
        self.visibility.clear();
        self.cy_bounds = (i32::MAX, i32::MIN);
        self.grid_stale = true;
        self.grid.next_generation();
    }

    pub fn insert_column(
        &mut self,
        pos: ChunkPos,
        cy_span: (i32, i32),
        sections: impl Iterator<Item = (i32, SectionVisibility)>,
    ) {
        let (min_cy, max_cy) = cy_span;
        if min_cy > max_cy {
            return;
        }
        let first = self.visibility.len() as u32;
        let len = (max_cy - min_cy + 1) as u32;
        self.visibility
            .resize((first + len) as usize, SectionVisibility::ALL);
        for (cy, vis) in sections {
            if (min_cy..=max_cy).contains(&cy) {
                self.visibility[(first + (cy - min_cy) as u32) as usize] = vis;
            }
        }
        self.columns
            .insert(pos, ColumnVisibility { min_cy, first, len });
        self.cy_bounds = (self.cy_bounds.0.min(min_cy), self.cy_bounds.1.max(max_cy));
        self.grid_stale = true;
    }

    /// Lays the recorded visibilities into the flood box; unrecorded sections stay open.
    fn fill_grid(&mut self, min: [i32; 3], size: [i32; 3]) {
        let grid = &mut self.grid;
        let cells = (size[0] * size[1] * size[2]) as usize;
        grid.min = min;
        grid.size = size;
        grid.visibility.clear();
        grid.visibility.resize(cells, SectionVisibility::ALL);
        if grid.stamp.len() < cells {
            grid.stamp.resize(cells, 0);
            grid.state.resize(cells, 0);
        }
        // Stamps written under an earlier box layout must never read as current.
        grid.stamp[..cells].fill(0);
        grid.generation = 1;
        for (pos, column) in &self.columns {
            let (x, z) = (pos.cx - min[0], pos.cz - min[2]);
            if !(0..size[0]).contains(&x) || !(0..size[2]).contains(&z) {
                continue;
            }
            let base = ((z * size[0] + x) * size[1]) as usize;
            for offset in 0..column.len as i32 {
                let y = column.min_cy + offset - min[1];
                if (0..size[1]).contains(&y) {
                    grid.visibility[base + y as usize] =
                        self.visibility[(column.first + offset as u32) as usize];
                }
            }
        }
        self.grid_stale = false;
    }

    /// Floods from `camera` through sections `admit` accepts. Every admitted section must lie
    /// within `reach` columns of the camera on x and z; nothing beyond it is visited.
    pub fn flood(
        &mut self,
        camera: SectionPos,
        reach: i32,
        admit: impl Fn(SectionPos) -> bool,
    ) -> bool {
        self.queue.clear();
        if self.columns.is_empty() {
            self.grid.next_generation();
            return false;
        }
        let lo = self.cy_bounds.0.min(camera.cy);
        let hi = self.cy_bounds.1.max(camera.cy);
        let min = [camera.cx - reach, lo, camera.cz - reach];
        let size = [2 * reach + 1, hi - lo + 1, 2 * reach + 1];
        if self.grid_stale || self.grid.min != min || self.grid.size != size {
            self.fill_grid(min, size);
        } else {
            self.grid.next_generation();
        }
        let grid = &mut self.grid;
        let [sx, sy, _] = grid.size;
        let camera_local = [camera.cx - min[0], camera.cy - min[1], camera.cz - min[2]];
        let camera_index = grid.index(camera).expect("the flood box holds the camera");
        grid.set_state(camera_index, u16::from(CAMERA));
        // Index step of each exit face, in `Face` order.
        let step = [sy, -sy, 1, -1, sx * sy, -sx * sy];
        let dirs = Face::ALL.map(|face| face.dir());
        self.queue.clear();
        self.queue
            .push((camera_index as u32, camera_local, NO_ENTRY));
        while let Some((index, [x, y, z], entry)) = self.queue.pop() {
            let here = grid.visibility[index as usize];
            let mut exits = exits_away_inside([x, y, z], camera_local, grid.size);
            if entry != NO_ENTRY && here != SectionVisibility::ALL {
                let entry = Face::ALL[entry as usize];
                let mut candidates = exits;
                exits = 0;
                while candidates != 0 {
                    let exit = candidates.trailing_zeros() as usize;
                    candidates &= candidates - 1;
                    if here.connects(entry, Face::ALL[exit]) {
                        exits |= 1 << exit;
                    }
                }
            }
            while exits != 0 {
                let exit = exits.trailing_zeros() as usize;
                exits &= exits - 1;
                let next_index = (index as i32 + step[exit]) as usize;
                let d = dirs[exit];
                let next_local = [x + d.x, y + d.y, z + d.z];
                let mut state = grid.state(next_index);
                if state & ADMIT_KNOWN == 0 {
                    let next = SectionPos::new(
                        min[0] + next_local[0],
                        min[1] + next_local[1],
                        min[2] + next_local[2],
                    );
                    state |= ADMIT_KNOWN | if admit(next) { ADMITTED } else { 0 };
                    grid.set_state(next_index, state);
                }
                if state & ADMITTED == 0 {
                    continue;
                }
                // The entry face is the exit's opposite: faces pair up as (2k, 2k + 1).
                let entry_face = (exit ^ 1) as u8;
                let bit = if grid.visibility[next_index] == SectionVisibility::ALL {
                    ANY_ENTRY
                } else {
                    1 << entry_face
                };
                if state as u8 & bit != 0 {
                    continue;
                }
                grid.set_state(next_index, state | u16::from(bit));
                self.queue.push((next_index as u32, next_local, entry_face));
            }
        }
        true
    }

    #[inline]
    pub fn is_visible(&self, pos: SectionPos) -> bool {
        self.grid
            .index(pos)
            .is_some_and(|index| self.grid.state(index) as u8 != 0)
    }
}

#[cfg(test)]
mod tests;
