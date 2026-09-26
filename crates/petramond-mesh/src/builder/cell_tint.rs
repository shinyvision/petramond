//! Every colour multiply a section's faces take: the biome tint of a tile's
//! tint class, and the per-cell `petramond:tint` presentation entries
//! (replicated cell KV) that multiply into the vertex tint lane.

use std::collections::HashMap;

use petramond_world::block::CellPart;
use petramond_world::section::Section;
use petramond_world::tile::TileTint;

use super::super::boxset::ShapeBox;
use super::super::tint::{BiomeTints, NO_TINT};

pub(super) struct CellTinting {
    /// Present when the section holds anything biome- or set-tinted.
    biome: Option<BiomeTints>,
    /// Sparse — empty on almost every section, so the fast path of every
    /// per-cell query is one `is_empty` test.
    cell_tints: HashMap<u16, Vec<(CellPart, [f32; 3])>>,
    /// Per-cell model part masks (same lane, same sparsity): which of a model
    /// row's OPTIONAL parts this placed instance shows.
    cell_parts: HashMap<u16, u32>,
}

impl CellTinting {
    pub(super) fn new(section: &Section, biome: Option<BiomeTints>) -> Self {
        Self {
            biome,
            cell_tints: section.cell_tint_map(),
            cell_parts: section.cell_parts_map(),
        }
    }

    /// The world tint of a tile of tint class `kind` in column `ci`.
    #[inline]
    pub(super) fn tile(&self, kind: Option<TileTint>, ci: usize) -> [f32; 3] {
        match kind {
            Some(TileTint::Fixed(rgb)) => rgb.map(|c| f32::from(c) / 255.0),
            _ => self.biome.as_ref().map_or(NO_TINT, |t| t.tile(kind, ci)),
        }
    }

    /// One cell's tint for one of its parts. A single-part cell (every cube,
    /// stair, chair — anything but a stacked slab) carries only part 0, so the
    /// scan ends on the first entry.
    #[inline]
    pub(super) fn part(&self, cell: usize, part: CellPart) -> Option<[f32; 3]> {
        if self.cell_tints.is_empty() {
            return None;
        }
        self.cell_tints
            .get(&(cell as u16))?
            .iter()
            .find(|&&(p, _)| p == part)
            .map(|&(_, m)| m)
    }

    /// `tint` under the cell's whole-cell (part 0) multiply — the cube path is
    /// whole-cell by nature.
    #[inline]
    pub(super) fn cube(&self, cell: usize, tint: [f32; 3]) -> [f32; 3] {
        match self.part(cell, 0) {
            Some(m) => [tint[0] * m[0], tint[1] * m[1], tint[2] * m[2]],
            None => tint,
        }
    }

    /// Whether the cell carries a tint at all — tinted cells set the vertex
    /// dyed flag so faces sample their tiles' dye-base twins and the multiply
    /// lands on a desaturated, peak-white base.
    #[inline]
    pub(super) fn tinted(&self, cell: usize) -> bool {
        self.part(cell, 0).is_some()
    }

    /// The ONE place a cell's `petramond:tint` reaches box geometry: multiply
    /// it into every emitted face AND mark the box dyed (so the multiply lands
    /// on the tile's dye-base twin). Every box family gets both halves by
    /// construction. Threading the multiply through each family's own tint
    /// closure instead left four of six families flagging the dye base without
    /// ever multiplying, which rendered them whitened and untinted.
    ///
    /// Each box takes ITS OWN part's tint, so one cell can hold a dyed layer
    /// and a plain one (a white slab under an orange one) and each draws right.
    pub(super) fn apply_to_boxes(&self, boxes: &mut [ShapeBox], cell: usize) {
        if self.cell_tints.is_empty() {
            return;
        }
        for b in boxes.iter_mut() {
            if let Some(m) = self.part(cell, b.part) {
                b.apply_tint(m);
            }
        }
    }

    /// A model cell's packed tint word, or `MODEL_TINT_NONE` when untinted.
    pub(super) fn model_tint(&self, cell: usize) -> u32 {
        match self.part(cell, 0) {
            Some(m) => {
                let ch = |v: f32| ((v * 255.0).round() as u32).min(255);
                (ch(m[0]) << 16) | (ch(m[1]) << 8) | ch(m[2])
            }
            None => super::super::vertex::MODEL_TINT_NONE,
        }
    }

    /// A model cell's optional-part mask (0 = the row's defaults).
    pub(super) fn model_parts(&self, cell: usize) -> u32 {
        self.cell_parts.get(&(cell as u16)).copied().unwrap_or(0)
    }
}
