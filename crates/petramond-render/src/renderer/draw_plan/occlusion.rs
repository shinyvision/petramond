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

/// One loaded column's section connectivity, densely by `cy`.
struct ColumnVisibility {
    min_cy: i32,
    first: u32,
    len: u32,
}

/// The entry-face bit that stands for "any face": a fully connected section
/// is expanded once, whichever face the flood reaches it through.
const ANY_ENTRY: u8 = 1 << 6;

pub(crate) struct SectionOcclusion {
    columns: FxHashMap<ChunkPos, ColumnVisibility>,
    visibility: Vec<SectionVisibility>,
    /// `(min, max)` section `cy` over every loaded column; inverted when none.
    cy_bounds: (i32, i32),
    /// The last flood's reached sections.
    visible: FxHashSet<SectionPos>,
    /// Entry faces each reached section was already expanded through.
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

/// The face a step through `exit` enters the neighbour by.
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

/// Whether a sight line from `camera` can step from `pos` through `exit`: it
/// only moves AWAY from the camera's section along the step's axis (or out of
/// the camera's own slab).
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
    /// Forget every column (the column set is rebuilt next).
    pub fn clear(&mut self) {
        self.columns.clear();
        self.visibility.clear();
        self.cy_bounds = (i32::MAX, i32::MIN);
        self.visible.clear();
        self.entered.clear();
    }

    /// Record one column's installed sections (`(cy, visibility)` pairs within
    /// `cy_span`); sections it lacks inside the span stay fully connected.
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

    /// A section's connectivity; fully connected when nothing is recorded.
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

    /// Flood from `camera` through the sections `admit` lets in (the view
    /// volume). False when there is nothing to cull against — no column is
    /// loaded — and every admitted section should be drawn.
    pub fn flood(&mut self, camera: SectionPos, admit: impl Fn(SectionPos) -> bool) -> bool {
        self.visible.clear();
        self.entered.clear();
        self.queue.clear();
        if self.columns.is_empty() {
            return false;
        }
        // Straight sight lines never leave the loaded band vertically and
        // come back; the camera itself may sit above or below it.
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

    /// Whether the last flood reached `pos`.
    #[inline]
    pub fn is_visible(&self, pos: SectionPos) -> bool {
        self.visible.contains(&pos)
    }
}

#[cfg(test)]
mod tests;
