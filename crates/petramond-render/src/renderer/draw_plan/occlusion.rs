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
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::VecDeque;

struct ColumnVisibility {
    min_cy: i32,
    first: u32,
    len: u32,
}

const ANY_ENTRY: u8 = 1 << 6;

pub(crate) struct SectionOcclusion {
    columns: FxHashMap<ChunkPos, ColumnVisibility>,
    visibility: Vec<SectionVisibility>,
    cy_bounds: (i32, i32),
    visible: FxHashSet<SectionPos>,
    entered: FxHashMap<SectionPos, u8>,
    queue: VecDeque<(SectionPos, Option<Face>)>,
}

impl Default for SectionOcclusion {
    fn default() -> Self {
        Self {
            columns: FxHashMap::default(),
            visibility: Vec::new(),
            cy_bounds: (i32::MAX, i32::MIN),
            visible: FxHashSet::default(),
            entered: FxHashMap::default(),
            queue: VecDeque::new(),
        }
    }
}

fn opposite(face: Face) -> Face {
    match face {
        Face::PosX => Face::NegX,
        Face::NegX => Face::PosX,
        Face::PosY => Face::NegY,
        Face::NegY => Face::PosY,
        Face::PosZ => Face::NegZ,
        Face::NegZ => Face::PosZ,
    }
}

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
        self.visible.clear();
        self.entered.clear();
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
    }

    fn visibility_at(&self, pos: SectionPos) -> SectionVisibility {
        let Some(column) = self.columns.get(&ChunkPos::new(pos.cx, pos.cz)) else {
            return SectionVisibility::ALL;
        };
        let offset = pos.cy - column.min_cy;
        if offset < 0 || offset as u32 >= column.len {
            return SectionVisibility::ALL;
        }
        self.visibility[(column.first + offset as u32) as usize]
    }

    pub fn flood(&mut self, camera: SectionPos, admit: impl Fn(SectionPos) -> bool) -> bool {
        self.visible.clear();
        self.entered.clear();
        self.queue.clear();
        if self.columns.is_empty() {
            return false;
        }
        let lo = self.cy_bounds.0.min(camera.cy);
        let hi = self.cy_bounds.1.max(camera.cy);
        self.visible.insert(camera);
        self.queue.push_back((camera, None));
        while let Some((pos, entry)) = self.queue.pop_front() {
            let here = self.visibility_at(pos);
            for exit in Face::ALL {
                if !moves_away(camera, pos, exit) {
                    continue;
                }
                if entry.is_some_and(|entry| !here.connects(entry, exit)) {
                    continue;
                }
                let glam::IVec3 {
                    x: dx,
                    y: dy,
                    z: dz,
                } = exit.dir();
                let next = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if next.cy < lo || next.cy > hi || !admit(next) {
                    continue;
                }
                let entry_face = opposite(exit);
                let bit = if self.visibility_at(next) == SectionVisibility::ALL {
                    ANY_ENTRY
                } else {
                    1 << entry_face as u8
                };
                let seen = self.entered.entry(next).or_insert(0);
                if *seen & bit != 0 {
                    continue;
                }
                *seen |= bit;
                self.visible.insert(next);
                self.queue.push_back((next, Some(entry_face)));
            }
        }
        true
    }

    #[inline]
    pub fn is_visible(&self, pos: SectionPos) -> bool {
        self.visible.contains(&pos)
    }
}

#[cfg(test)]
mod tests;
