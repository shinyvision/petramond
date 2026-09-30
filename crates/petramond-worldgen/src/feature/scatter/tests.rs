use super::*;
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
