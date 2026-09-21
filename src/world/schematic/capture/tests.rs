use super::*;
use petramond_world::{
    block::CellCodec,
    container::Container,
    item::{ItemStack, ItemType},
    section::SectionSummary,
};

fn world() -> World {
    World::new_with_pool(
        0,
        1,
        crate::world::WorldRole::ServerHeadless,
        Arc::new(crate::worker::JobPool::inline()),
    )
}

#[test]
fn sparse_capture_skips_large_air_volume_and_keeps_the_save_time_snapshot() {
    let mut world = world();
    for x in 0..4 {
        for z in 0..4 {
            for y in 4..6 {
                let sp = SectionPos::new(x, y, z);
                world.insert_section_for_test(sp, Section::new(x, y, z));
            }
        }
    }
    let pos = [15, 79, 15];
    let s = world
        .section_at_world_mut_for_test(pos[0], pos[1], pos[2])
        .unwrap();
    s.set_block(15, 15, 15, Block::Chest);
    s.insert_container(
        15,
        15,
        15,
        Container {
            slots: vec![Some(ItemStack::new(ItemType::Stone, 7)), None],
        },
    );
    s.cell_kv_restore(
        15,
        15,
        15,
        std::collections::BTreeMap::from([("fixture:data".into(), vec![0, 1, 255])]),
    );
    let before = CellData::capture(&world.snapshot_cell(IVec3::from_array(pos)).unwrap());
    let region = SelectionBox::between([0, 64, 0], [63, 95, 63]).unwrap();
    assert!(region.volume().unwrap() > 100_000);
    let capture = Capture::new(&world, "Sparse".into(), vec![region], false, false).unwrap();
    world
        .section_at_world_mut_for_test(pos[0], pos[1], pos[2])
        .unwrap()
        .set_block(15, 15, 15, Block::Dirt);
    let saved = capture.run().unwrap();
    assert_eq!(saved.cell_count(), 1);
    assert_eq!(saved.cells().next().unwrap().data, &before);
    assert_eq!(saved.size, [1; 3]);
}

#[test]
fn capture_includes_air_only_on_request_and_deduplicates_overlaps() {
    let mut world = world();
    let mut section = Section::new(-1, 0, -1);
    section.set_block(14, 0, 15, Block::Stone);
    world.insert_section_for_test(SectionPos::new(-1, 0, -1), section);
    let region = SelectionBox::between([-2, 0, -1], [-1, 0, -1]).unwrap();
    let sparse = Capture::new(&world, "Sparse".into(), vec![region], false, false)
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(sparse.cell_count(), 1);
    let air = Capture::new(&world, "Air".into(), vec![region, region], true, false)
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(air.cell_count(), 2);
    assert_eq!(
        air.cells().nth(1).unwrap().data.resolve().unwrap().block,
        Block::Air
    );
}

#[test]
fn capture_expands_compounds_across_section_boundaries() {
    let mut world = world();
    let door = *Block::all()
        .iter()
        .find(|b| b.shape_family() == ShapeFamily::Door)
        .unwrap();
    for (cy, y, top) in [(0, 15, false), (1, 0, true)] {
        let mut section = Section::new(0, cy, 0);
        section.set_block(0, y, 0, door);
        section.set_cell_state(
            0,
            y,
            0,
            petramond_world::door::DoorState {
                facing: petramond_math::facing::Facing::South,
                open: false,
                top,
            }
            .to_cell(),
        );
        world.insert_section_for_test(SectionPos::new(0, cy, 0), section);
    }
    let r = SelectionBox::between([0, 15, 0], [0, 15, 0]).unwrap();
    let saved = Capture::new(&world, "Door".into(), vec![r], false, false)
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(saved.cell_count(), 2);
    assert_eq!(saved.size, [1, 2, 1]);
    assert!(
        !petramond_world::door::DoorState::from_cell(
            saved.cells().next().unwrap().data.resolve().unwrap().state
        )
        .top
    );
    assert!(
        petramond_world::door::DoorState::from_cell(
            saved.cells().nth(1).unwrap().data.resolve().unwrap().state
        )
        .top
    );
}

#[test]
fn missing_and_in_flight_terrain_is_not_saved_as_air_but_uniform_solid_is_preserved() {
    let mut world = world();
    let sp = SectionPos::new(0, 0, 0);
    let region = SelectionBox::between([0; 3], [1; 3]).unwrap();
    let capture = || {
        Capture::new(&world, "Terrain".into(), vec![region], true, false)
            .unwrap()
            .run()
    };
    assert!(capture().is_err());
    world.insert_section_for_test(sp, Section::new(0, 0, 0));
    world.sections.remove(&sp);
    let mut summaries = vec![SectionSummary::Empty; (SECTION_MAX_CY - SECTION_MIN_CY + 1) as usize];
    summaries[(-SECTION_MIN_CY) as usize] = SectionSummary::FullOpaque;
    world
        .column_summaries
        .insert(sp.chunk_pos(), summaries.into_boxed_slice());
    let saved = Capture::new(&world, "Uniform".into(), vec![region], false, false)
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(saved.cell_count(), 8);
    assert!(saved
        .cells()
        .all(|c| c.data.resolve().unwrap().block == SectionSummary::FullOpaque.virtual_block()));
    world.mark_overlay_in_flight_for_test(sp);
    assert!(
        Capture::new(&world, "Pending".into(), vec![region], true, false)
            .unwrap()
            .run()
            .is_err()
    );
}

#[test]
fn capture_large_solid_build_interns_records_by_section() {
    let mut world = world();
    for cx in 0..20 {
        let mut section = Section::new(cx, 4, 0);
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    section.set_block(x, y, z, Block::Stone);
                }
            }
        }
        world.insert_section_for_test(SectionPos::new(cx, 4, 0), section);
    }
    let region = SelectionBox::between([0, 64, 0], [319, 79, 15]).unwrap();
    let saved = Capture::new(&world, "Large".into(), vec![region], false, false)
        .unwrap()
        .run()
        .unwrap();
    assert_eq!(saved.size, [320, 16, 16]);
    assert_eq!(saved.cell_count(), 81920);
    assert!(
        saved
            .cells()
            .map(|c| c.data.validate().unwrap())
            .sum::<usize>()
            > 4 * 1024 * 1024
    );
    assert!(saved
        .sections
        .iter()
        .all(|s| s.palette.len() == 1 && s.cells.len() == 4096));
}
