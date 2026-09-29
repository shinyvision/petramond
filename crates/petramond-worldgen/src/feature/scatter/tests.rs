use super::*;
use crate::data::ores::tests::legacy_veins;
use crate::data::ores::OreVein;
use petramond_world::chunk::SECTION_SIZE;

fn stone_section(cx: i32, cy: i32, cz: i32) -> Section {
    let mut section = Section::new(cx, cy, cz);
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                section.set_block(x, y, z, Block::Stone);
            }
        }
    }
    section
}

fn cells(section: &Section) -> Vec<u16> {
    section.blocks_iter().collect()
}

#[test]
fn loaded_table_places_the_same_veins_as_the_compiled_table() {
    let compiled = OreTable::new(legacy_veins().leak());
    let mut ores_seen = 0;
    for seed in [1u32, 42, 0x1234_5678] {
        for &(cx, cz) in &[(0, 0), (-3, 5), (17, -9), (-120, -64)] {
            for cy in [-4, -2, -1, 0, 1, 3, 6, 8, 9] {
                let mut data = stone_section(cx, cy, cz);
                let mut code = stone_section(cx, cy, cz);
                place_underground_section(&mut data, seed, &crate::cache::installed());
                place_table_section(&compiled, &mut code, seed, &crate::cache::installed());
                let (data, code) = (cells(&data), cells(&code));
                assert_eq!(data, code, "seed {seed} section ({cx},{cy},{cz})");
                ores_seen += data.iter().filter(|&&id| id != Block::Stone.id()).count();
            }
        }
    }
    assert!(ores_seen > 0, "the sampled sections must hold veins");
}

#[test]
fn veins_overwrite_only_their_hosts() {
    const TUFF: &[Block] = &[Block::Tuff];
    let veins = vec![OreVein {
        block: Block::GoldOre,
        salt: 99,
        count: 64,
        shape: VeinShape::Blob { size: 20 },
        y_min: 0,
        y_max: 15,
        depth_ramp: None,
        hosts: TUFF,
    }];
    let table = OreTable::new(veins.leak());
    let mut section = stone_section(0, 0, 0);
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in SECTION_SIZE / 2..SECTION_SIZE {
                section.set_block(x, y, z, Block::Tuff);
            }
        }
    }
    place_table_section(&table, &mut section, 7, &crate::cache::installed());
    let mut gold = 0;
    for y in 0..SECTION_SIZE {
        for z in 0..SECTION_SIZE {
            for x in 0..SECTION_SIZE {
                let block = section.block(x, y, z);
                if x < SECTION_SIZE / 2 {
                    assert_eq!(block, Block::Stone, "stone at ({x},{y},{z}) was not a host");
                } else if block == Block::GoldOre {
                    gold += 1;
                }
            }
        }
    }
    assert!(gold > 0, "the tuff half must take gold veins");
}
