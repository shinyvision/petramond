use crate::block::Block;
use crate::chunk::{
    section_idx, Chunk, CHUNK_SX, CHUNK_SY, CHUNK_SZ, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::column::Column;
use crate::section::Section;

/// Split a whole-column [`Chunk`] (a 0..256 `generate_chunk` output, or a hand-built
/// fixture) into cubic [`Section`]s plus its [`Column`] data, adding solid-stone
/// sections for the range below y=0. All-air sections are skipped (absent reads as
/// air). TEST/FIXTURE helper only: the live streamer generates per section
/// (`ChunkGenerator::generate_section`), never via a 256-tall intermediate. Retained so
/// the many column-era test fixtures (`insert_chunk_for_test`) keep working.
#[cfg(test)]
pub(crate) fn split_generated_column(chunk: &Chunk) -> (Column, Vec<(i32, Section)>) {
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

    // Surface column: the generator's 0..256 output → sections cy 0..15.
    let surface_sections = (CHUNK_SY / SECTION_SIZE) as i32;
    for cy in 0..surface_sections {
        let mut section = Section::new(cx, cy, cz);
        let mut any = false;
        {
            let dst = section.blocks_slice_mut();
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
        }
        if !any {
            continue; // all-air section: absent reads as air.
        }
        copy_generated_water(chunk, cy, &mut section);
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    // Expanded range below y=0: solid stone, so caves have somewhere to carve.
    for cy in SECTION_MIN_CY..0 {
        let mut section = Section::new(cx, cy, cz);
        {
            let dst = section.blocks_slice_mut();
            for d in dst.iter_mut() {
                *d = Block::Stone.id();
            }
        }
        section.recompute_opaque_count();
        out.push((cy, section));
    }

    (column, out)
}

/// Carry the generated column's water-flow metadata for section `cy` into `section`,
/// so generated rivers/pools keep their source/falloff state through the split.
#[cfg(test)]
fn copy_generated_water(chunk: &Chunk, cy: i32, section: &mut Section) {
    let water = Block::Water.id();
    for ly in 0..SECTION_SIZE {
        let wy = cy as usize * SECTION_SIZE + ly;
        for z in 0..CHUNK_SZ {
            for x in 0..CHUNK_SX {
                if chunk.block_raw(x, wy, z) == water {
                    section.set_water(x, ly, z, Block::Water, chunk.water_meta(x, wy, z));
                }
            }
        }
    }
}
