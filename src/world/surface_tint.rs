//! Presentation-only top-down surface sampling: per-column height + color
//! grids with lazily built 5×5 biome-blended tints (the map/minimap feed).

use std::sync::Arc;

use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use crate::column::{Column, NO_SURFACE};
use crate::section::Section;

use super::store::World;

impl World {
    /// Whether the column is loaded, and if so its payload revision — the
    /// change-detection half of [`client_surface_column`](Self::client_surface_column).
    pub(crate) fn client_surface_column_revision(&self, pos: ChunkPos) -> Option<u64> {
        self.columns
            .contains_key(&pos)
            .then(|| self.column_payload_revision(pos))
    }

    /// Final top-down surface samples for one whole chunk column, for
    /// presentation-only client modules: per cell `(height, rgb)`, or `None`
    /// where the cell is unknown (missing data or a surface section still in
    /// flight — never guessed from generation; callers retain prior explored
    /// samples). Returns `false` when the column itself is not loaded.
    ///
    /// Column, section finality, and the 5×5 biome tint blend are resolved
    /// once per column / per section / per tint kind, not per cell — this is
    /// the sampling hot path.
    pub(crate) fn client_surface_column(
        &self,
        pos: ChunkPos,
        out: &mut [Option<(i16, [u8; 3])>; 256],
    ) -> bool {
        let Some(column) = self.columns.get(&pos) else {
            return false;
        };
        let mut tints = SurfaceTintGrids::new(self, pos, column);
        // Surface heights cluster in a handful of sections per column.
        let mut sections: Vec<(i32, Option<&Section>)> = Vec::new();
        for lz in 0..16usize {
            for lx in 0..16usize {
                let i = lz * 16 + lx;
                out[i] = None;
                let height = column.surface_y(lx, lz);
                if height == NO_SURFACE {
                    continue;
                }
                let cy = height.div_euclid(SECTION_SIZE as i32);
                let section = match sections.iter().find(|(known, _)| *known == cy) {
                    Some((_, section)) => *section,
                    None => {
                        let sp = SectionPos::new(pos.cx, cy, pos.cz);
                        let section = (SectionPos::cy_in_range(cy) && self.stream_writable(sp))
                            .then(|| self.sections.get(&sp).map(Arc::as_ref))
                            .flatten();
                        sections.push((cy, section));
                        section
                    }
                };
                let Some(section) = section else {
                    continue;
                };
                let block = section.block(lx, height.rem_euclid(SECTION_SIZE as i32) as usize, lz);
                let tile = block.tiles()[0];
                let base = tile.map_rgb();
                let rgb = match tile.world_tint() {
                    None => base,
                    Some(kind) => {
                        let tint = tints.at(kind, lx, lz);
                        std::array::from_fn(|channel| {
                            (base[channel] as f32 * tint[channel])
                                .round()
                                .clamp(0.0, 255.0) as u8
                        })
                    }
                };
                out[i] = Some((height as i16, rgb));
            }
        }
        true
    }
}

/// Lazy per-column 5×5 biome-blended tint grids for surface sampling: one
/// 16×16 grid per [`TileTint`] kind, built on first use. With a 20×20 biome
/// halo the blend is a separable box sum (each halo biome color decodes once);
/// without one it falls back to the column's own unblended biome colors.
struct SurfaceTintGrids<'a> {
    halo: Option<&'a [u8]>,
    column: &'a Column,
    grids: [Option<Box<[[f32; 3]; 256]>>; 3],
}

impl<'a> SurfaceTintGrids<'a> {
    fn new(world: &'a World, pos: ChunkPos, column: &'a Column) -> Self {
        let halo = world
            .column_gen
            .get(&pos)
            .map(|column| column.mesh_biome_slice())
            .or_else(|| world.column_biome_halos.get(&pos).map(|halo| halo.as_ref()))
            .filter(|halo| halo.len() == 20 * 20);
        Self {
            halo,
            column,
            grids: [None, None, None],
        }
    }

    fn at(&mut self, kind: crate::atlas::TileTint, lx: usize, lz: usize) -> [f32; 3] {
        let slot = match kind {
            crate::atlas::TileTint::Grass => &mut self.grids[0],
            crate::atlas::TileTint::Foliage => &mut self.grids[1],
            crate::atlas::TileTint::Water => &mut self.grids[2],
        };
        slot.get_or_insert_with(|| Self::build(self.halo, self.column, kind))[lz * 16 + lx]
    }

    fn build(
        halo: Option<&[u8]>,
        column: &Column,
        kind: crate::atlas::TileTint,
    ) -> Box<[[f32; 3]; 256]> {
        let color_of = |id: u8| {
            let biome = crate::biome::Biome::from_id(id);
            match kind {
                crate::atlas::TileTint::Grass => biome.grass_color(),
                crate::atlas::TileTint::Foliage => biome.foliage_color(),
                crate::atlas::TileTint::Water => biome.water_color(),
            }
        };
        let mut out = Box::new([[0.0f32; 3]; 256]);
        let Some(halo) = halo else {
            for lz in 0..16 {
                for lx in 0..16 {
                    out[lz * 16 + lx] = color_of(column.biome_at(lx, lz));
                }
            }
            return out;
        };
        let mut colors = [[0.0f32; 3]; 400];
        for (color, &id) in colors.iter_mut().zip(halo) {
            *color = color_of(id);
        }
        // The halo starts two cells before the column, so the 5x5 blend window
        // for local (x,z) occupies [x..x+5, z..z+5] directly.
        let mut rows = [[0.0f32; 3]; 20 * 16];
        for z in 0..20 {
            for x in 0..16 {
                let mut sum = [0.0f32; 3];
                for cell in &colors[z * 20 + x..z * 20 + x + 5] {
                    for channel in 0..3 {
                        sum[channel] += cell[channel];
                    }
                }
                rows[z * 16 + x] = sum;
            }
        }
        for z in 0..16 {
            for x in 0..16 {
                let mut sum = [0.0f32; 3];
                for row in 0..5 {
                    for channel in 0..3 {
                        sum[channel] += rows[(z + row) * 16 + x][channel];
                    }
                }
                out[z * 16 + x] = sum.map(|channel| channel / 25.0);
            }
        }
        out
    }
}
