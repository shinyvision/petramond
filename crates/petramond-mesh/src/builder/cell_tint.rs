use std::collections::HashMap;

use petramond_world::block::CellPart;
use petramond_world::section::Section;
use petramond_world::tile::TileTint;

use super::super::boxset::ShapeBox;
use super::super::tint::{BiomeTints, NO_TINT};

pub(super) struct CellTinting {
    biome: Option<BiomeTints>,
    cell_tints: HashMap<u16, Vec<(CellPart, [f32; 3])>>,
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

    #[inline]
    pub(super) fn tile(&self, kind: Option<TileTint>, ci: usize) -> [f32; 3] {
        match kind {
            Some(TileTint::Fixed(rgb)) => rgb.map(|c| f32::from(c) / 255.0),
            _ => self.biome.as_ref().map_or(NO_TINT, |t| t.tile(kind, ci)),
        }
    }

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

    #[inline]
    pub(super) fn cube(&self, cell: usize, tint: [f32; 3]) -> [f32; 3] {
        match self.part(cell, 0) {
            Some(m) => [tint[0] * m[0], tint[1] * m[1], tint[2] * m[2]],
            None => tint,
        }
    }

    #[inline]
    pub(super) fn tinted(&self, cell: usize) -> bool {
        self.part(cell, 0).is_some()
    }

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

    pub(super) fn model_tint(&self, cell: usize) -> u32 {
        match self.part(cell, 0) {
            Some(m) => {
                let ch = |v: f32| ((v * 255.0).round() as u32).min(255);
                (ch(m[0]) << 16) | (ch(m[1]) << 8) | ch(m[2])
            }
            None => super::super::vertex::MODEL_TINT_NONE,
        }
    }

    pub(super) fn model_parts(&self, cell: usize) -> u32 {
        self.cell_parts.get(&(cell as u16)).copied().unwrap_or(0)
    }
}
