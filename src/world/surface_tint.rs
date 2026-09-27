use crate::world::{World, WorldSide};
use std::sync::Arc;

use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
use petramond_world::column::{Column, NO_SURFACE};
use petramond_world::section::Section;

impl<S: WorldSide> World<S> {
    pub fn client_surface_column_revision(&self, pos: ChunkPos) -> Option<u64> {
        self.data
            .columns
            .contains_key(&pos)
            .then(|| self.data.column_payload_revision(pos))
    }

    pub fn client_surface_column(
        &self,
        pos: ChunkPos,
        out: &mut [Option<(i16, [u8; 3])>; 256],
    ) -> bool {
        let Some(column) = self.data.columns.get(&pos) else {
            return false;
        };
        let mut tints = SurfaceTintGrids::new(self, pos, column);
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
                        let section = (SectionPos::cy_in_range(cy)
                            && self.data.stream_writable(sp))
                        .then(|| self.data.sections.get(&sp).map(Arc::as_ref))
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
                let base = petramond_world::tile::map_rgb(tile);
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

/// Tint grids for surface sampling, 16x16 per [`TileTint`] kind, filled in the first time a column
/// asks. If we have a 20x20 biome halo, each cell is a 5×5 box blend (separable, so each halo
/// color only decodes once). Otherwise we just use the column's own biome colors.
struct SurfaceTintGrids<'a> {
    halo: Option<&'a [u8]>,
    column: &'a Column,
    grids: [Option<Box<[[f32; 3]; 256]>>; 3],
}

impl<'a> SurfaceTintGrids<'a> {
    fn new<S: WorldSide>(world: &'a World<S>, pos: ChunkPos, column: &'a Column) -> Self {
        let halo = world
            .column_gen(pos)
            .map(|column| column.mesh_biome_slice())
            .or_else(|| {
                world
                    .data
                    .column_biome_halos
                    .get(&pos)
                    .map(|halo| halo.as_ref())
            })
            .filter(|halo| halo.len() == 20 * 20);
        Self {
            halo,
            column,
            grids: [None, None, None],
        }
    }

    fn at(&mut self, kind: petramond_world::tile::TileTint, lx: usize, lz: usize) -> [f32; 3] {
        let slot = match kind {
            petramond_world::tile::TileTint::Grass => &mut self.grids[0],
            petramond_world::tile::TileTint::Foliage => &mut self.grids[1],
            petramond_world::tile::TileTint::Water => &mut self.grids[2],
            petramond_world::tile::TileTint::Fixed(rgb) => {
                return rgb.map(|c| f32::from(c) / 255.0)
            }
        };
        slot.get_or_insert_with(|| Self::build(self.halo, self.column, kind))[lz * 16 + lx]
    }

    fn build(
        halo: Option<&[u8]>,
        column: &Column,
        kind: petramond_world::tile::TileTint,
    ) -> Box<[[f32; 3]; 256]> {
        let color_of = |id: u8| {
            let biome = petramond_world::biome::Biome::from_id(id);
            match kind {
                petramond_world::tile::TileTint::Grass => biome.grass_color(),
                petramond_world::tile::TileTint::Foliage => biome.foliage_color(),
                petramond_world::tile::TileTint::Water => biome.water_color(),
                petramond_world::tile::TileTint::Fixed(rgb) => rgb.map(|c| f32::from(c) / 255.0),
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
