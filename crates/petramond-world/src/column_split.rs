use crate::block::Block;
use crate::chunk::{
    section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::column::Column;
use crate::section::Section;

pub fn split_generated_column(chunk: &Chunk) -> (Column, Vec<(i32, Section)>) {
    let cx = chunk.cx;
    let cz = chunk.cz;
    let mut column = Column::new();
    for z in 0..CHUNK_SZ {
        for x in 0..CHUNK_SX {
            column.set_biome(x, z, chunk.biome_at(x, z));
            column.set_surface_y(x, z, chunk.surface_y(x, z));
            let mut sky_cover = -1;
            for y in (0..CHUNK_SY).rev() {
                let block = Block::from_id(chunk.block_raw(x, y, z));
                if !block.transmits_direct_skylight() {
                    sky_cover = y as i32;
                    break;
                }
            }
            column.set_sky_cover_y(x, z, sky_cover);
        }
    }

    let mut out: Vec<(i32, Section)> = Vec::new();

    let surface_sections = (CHUNK_SY / SECTION_SIZE) as i32;
    for cy in 0..surface_sections {
        let mut section = Section::new(cx, cy, cz);
        let mut any = false;
        section.edit_ids_bulk(|dst| {
            for ly in 0..SECTION_SIZE {
                let wy = cy as usize * SECTION_SIZE + ly;
                for z in 0..CHUNK_SZ {
                    for x in 0..CHUNK_SX {
                        let id = chunk.block_raw(x, wy, z);
                        if id != 0 {
                            dst[section_idx(x, ly, z)] = id;
                            any = true;
                        }
                    }
                }
            }
        });
        if !any {
            continue;
        }
        copy_generated_fluid_meta(chunk, cy, &mut section);
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    for cy in SECTION_MIN_CY..0 {
        let mut section = Section::new(cx, cy, cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    (column, out)
}

fn copy_generated_fluid_meta(chunk: &Chunk, cy: i32, section: &mut Section) {
    for ly in 0..SECTION_SIZE {
        let wy = cy as usize * SECTION_SIZE + ly;
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                let block = Block::from_id(chunk.block_raw(x, wy, z));
                if block.is_fluid() {
                    section.set_fluid(x, ly, z, block, chunk.fluid_meta(x, wy, z));
                }
            }
        }
    }
}
