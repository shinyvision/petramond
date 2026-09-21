use super::*;
use petramond_world::chunk::ChunkPos;

#[test]
fn a_floor_pasted_without_air_breaks_litter_that_cannot_live_on_the_new_surface() {
    let mut server = server();
    let origin = IVec3::new(7, 64, 7);
    let litter = [
        Block::ShortGrass,
        Block::PebblesSmall,
        Block::PebblesMedium,
        Block::PebblesLarge,
    ];
    let floor = Schematic::from_cells(
        "Floor".into(),
        [4, 1, 1],
        (0..4).map(|x| SchematicCell {
            pos: [x, 0, 0],
            data: CellData::capture(&data(Block::OakPlanks)),
        }),
    )
    .unwrap();

    for (x, b) in litter.into_iter().enumerate() {
        for z in 0..=1 {
            let p = origin + IVec3::new(x as i32, 0, z);
            server.world.set_block_world(p.x, p.y, p.z, Block::Grass);
            server.world.set_block_world(p.x, p.y + 1, p.z, b);
        }
    }
    server.world.game_tick(&server.recipes);
    assert!(server.world.take_natural_breaks().is_empty());
    server
        .place_schematic(0, &floor, origin.to_array(), 0, &mut TickEvents::default())
        .unwrap();
    server.world.game_tick(&server.recipes);
    let breaks = server.world.take_natural_breaks();
    let remaining: Vec<_> = litter
        .into_iter()
        .enumerate()
        .filter_map(|(x, b)| {
            let p = origin + IVec3::new(x as i32, 1, 0);
            assert_eq!(
                server.world.snapshot_cell(p - IVec3::Y).unwrap().block,
                Block::OakPlanks
            );
            assert_eq!(
                server.world.snapshot_cell(p + IVec3::Z).unwrap().block,
                b,
                "the same litter on untouched soil must survive the border update"
            );
            (server.world.snapshot_cell(p).unwrap().block != Block::Air
                || !breaks.contains(&(p, b)))
            .then_some(b)
        })
        .collect();
    assert!(
        remaining.is_empty(),
        "litter left on the wooden floor: {remaining:?}"
    );
}

#[test]
fn schematic_placement_updates_skipped_interior_cells_and_the_rotated_one_block_border() {
    let mut server = server();
    let mut schematic = plan(data(Block::Stone));
    schematic.size = [7, 4, 9];
    let mut far = schematic.cells().next().unwrap().to_owned();
    far.pos = [6, 3, 8];
    schematic = Schematic::from_cells(
        schematic.name.clone(),
        schematic.size,
        schematic.cells().map(|c| c.to_owned()).chain([far]),
    )
    .unwrap();
    let origin = IVec3::new(3, 65, 3);

    for turns in 0..4 {
        server.world.clear_world();
        for cx in -1..=1 {
            for cz in -1..=1 {
                server
                    .world
                    .insert_empty_column_for_test(ChunkPos::new(cx, cz));
            }
        }
        let [x, y, z] = schematic.rotated_size(turns);
        let unsupported = [
            (IVec3::new(3, 1, 4), Block::ShortGrass),
            (IVec3::new(4, 1, 4), Block::PebblesSmall),
            (IVec3::new(-1, 1, 4), Block::ShortGrass),
            (IVec3::new(x, 1, 4), Block::PebblesSmall),
            (IVec3::new(3, -1, 4), Block::ShortGrass),
            (IVec3::new(3, y, 4), Block::PebblesSmall),
            (IVec3::new(3, 1, -1), Block::ShortGrass),
            (IVec3::new(3, 1, z), Block::PebblesSmall),
            (-IVec3::ONE, Block::ShortGrass),
            (IVec3::new(x, y, z), Block::PebblesSmall),
        ]
        .map(|(p, b)| (origin + p, b));
        let supported = origin + IVec3::new(1, 1, 2);
        let outside = origin + IVec3::new(-2, 1, 2);
        for (p, b) in unsupported.into_iter().chain([
            (supported, Block::ShortGrass),
            (supported - IVec3::Y, Block::Dirt),
            (outside, Block::ShortGrass),
        ]) {
            server.world.set_block_world(p.x, p.y, p.z, b);
        }
        // Generated decorations have no outstanding edit notification to rescue them.
        server.world.sim.update_queue.clear();
        server.world.sim.update_set.clear();
        server
            .place_schematic(
                0,
                &schematic.clone(),
                origin.to_array(),
                turns,
                &mut TickEvents::default(),
            )
            .unwrap();

        server
            .apply_creative(0, CreativeAction::Undo, &mut TickEvents::default())
            .unwrap();
        server.world.sim.update_queue.clear();
        server.world.sim.update_set.clear();
        server
            .apply_creative(0, CreativeAction::Redo, &mut TickEvents::default())
            .unwrap();

        for (p, block) in unsupported {
            assert!(
                server.world.sim.update_set.contains(&p),
                "missing update at {p:?}, turn {turns}"
            );
            assert_eq!(
                server.world.snapshot_cell(p).unwrap().block,
                block,
                "notification must defer to the ordinary behavior dispatch"
            );
        }
        assert!(!server.world.sim.update_set.contains(&outside));
        for (p, _) in schematic.placed_cells(origin, turns).unwrap() {
            assert!(
                server.world.sim.update_set.contains(&p),
                "copied solids also receive updates"
            );
        }
        server.world.game_tick(&server.recipes);
        let breaks = server.world.take_natural_breaks();
        for (p, block) in unsupported {
            assert_eq!(server.world.snapshot_cell(p).unwrap().block, Block::Air);
            assert!(
                breaks.contains(&(p, block)),
                "ordinary break effects and drops must be emitted"
            );
        }
        assert_eq!(
            server.world.snapshot_cell(supported).unwrap().block,
            Block::ShortGrass
        );
        assert_eq!(
            server.world.snapshot_cell(outside).unwrap().block,
            Block::ShortGrass
        );
    }
}
