use super::*;
use petramond_math::{facing::Facing, math::IVec3};
use petramond_world::{
    block::{Block, CellCodec, CellView, ShapeState},
    block_state::{SlabSplit, SlabState},
    container::Container,
    furnace::Furnace,
    item::{variant, ItemStack, ItemType},
};
use std::collections::BTreeMap;

fn cell(block: Block) -> ResolvedCell {
    ResolvedCell {
        block,
        state: ShapeState::NONE,
        fluid: 0,
        kv: Default::default(),
        container: None,
        furnace: None,
    }
}
fn schematic(cells: Vec<(IVec3, ResolvedCell)>, size: [i32; 3]) -> Schematic {
    Schematic::from_cells(
        "Round trip".into(),
        size,
        cells.into_iter().map(|(p, d)| SchematicCell {
            pos: p.to_array(),
            data: CellData::capture(&d),
        }),
    )
    .unwrap()
}

#[test]
fn selection_unions_overlap_and_subtraction_without_expanding_holes() {
    let mut s = Selection::default();
    s.region([2, 0, 0], [0, 0, 0], false).unwrap();
    s.region([1, 0, 0], [3, 0, 0], false).unwrap();
    s.region([10, 1, 0], [10, 1, 0], false).unwrap();
    s.region([1, 0, 0], [2, 0, 0], true).unwrap();
    assert_eq!(
        s.cells().collect::<Vec<_>>(),
        vec![[0, 0, 0], [3, 0, 0], [10, 1, 0]]
    );
    let before = s.cells().collect::<Vec<_>>();
    assert!(s.region([i32::MIN; 3], [i32::MAX; 3], false).is_err());
    assert_eq!(s.cells().collect::<Vec<_>>(), before);
}

#[test]
fn portable_cell_round_trip_keeps_shape_references_and_all_instance_stores() {
    let mut d = cell(Block::Stone);
    d.state = SlabState {
        split: SlabSplit::Z,
        layers: [Block::Stone, Block::OakPlanks],
    }
    .to_cell();
    d.fluid = 7;
    d.kv.insert("fixture:opaque".into(), vec![0, 255, 1, 9]);
    let variant = variant::intern(&BTreeMap::from([(
        "fixture:label".into(),
        b"schematic stack".to_vec(),
    )]))
    .unwrap();
    d.container = Some(Container {
        slots: vec![
            Some(ItemStack::with_variant(ItemType::Stone, 12, variant)),
            None,
        ],
    });
    d.furnace = Some(Furnace {
        cook_progress: 12,
        burn_remaining: 30,
        burn_max: 60,
    });
    let saved = CellData::capture(&d);
    assert!(!saved.state_ids.is_empty());
    assert!(saved.container.as_ref().unwrap()[0]
        .as_ref()
        .unwrap()
        .item
        .contains(':'));
    let json = serde_json::to_string(&saved).unwrap();
    assert_eq!(
        serde_json::from_str::<CellData>(&json)
            .unwrap()
            .resolve()
            .unwrap(),
        d
    );
    let mut missing = saved;
    missing.block = "missing:unknown_block".into();
    assert!(missing.resolve().is_err());
}

#[test]
fn slab_rotation_moves_both_material_and_part_data() {
    let block = *Block::all()
        .iter()
        .find(|b| b.shape_family() == petramond_world::block::ShapeFamily::Slab)
        .unwrap();
    let mut d = cell(block);
    d.state = SlabState {
        split: SlabSplit::Z,
        layers: [Block::Stone, Block::OakPlanks],
    }
    .to_cell();
    let k0 = petramond_world::block::part_kv_key("fixture:part", 0);
    let k1 = petramond_world::block::part_kv_key("fixture:part", 1);
    d.kv.insert(k0.clone(), vec![11]);
    d.kv.insert(k1.clone(), vec![22]);
    let s = schematic(vec![(IVec3::ZERO, d.clone())], [1; 3]);
    let rotated = s.placed_cells(IVec3::ZERO, 1).unwrap();
    let slab = SlabState::from_cell(rotated[0].1.state);
    assert_eq!(slab.split, SlabSplit::X);
    assert_eq!(slab.layers, [Block::OakPlanks, Block::Stone]);
    assert_eq!(rotated[0].1.kv[&k0], vec![22]);
    let mut current = s;
    for _ in 0..4 {
        current = schematic(current.placed_cells(IVec3::ZERO, 1).unwrap(), [1; 3]);
    }
    assert_eq!(current.placed_cells(IVec3::ZERO, 0).unwrap()[0].1, d);
}

#[test]
fn model_rotation_reanchors_machine_data_and_keeps_every_footprint_cell() {
    let block = Block::Bed;
    let kind = block.model_kind().unwrap();
    let facing = Facing::South;
    let cells: Vec<_> =
        petramond_world::block_model::oriented_footprint_cells(IVec3::ZERO, kind, facing)
            .into_iter()
            .map(|(p, offset)| {
                let mut d = cell(block);
                d.state = petramond_world::block_model::ModelCellState { offset, facing }.to_cell();
                if p == IVec3::ZERO {
                    d.container = Some(Container {
                        slots: vec![Some(ItemStack::new(ItemType::Stone, 2))],
                    });
                    d.kv.insert("fixture:machine".into(), vec![42]);
                }
                (p, d)
            })
            .collect();
    let size = std::array::from_fn(|i| cells.iter().map(|(p, _)| p[i]).max().unwrap() + 1);
    let original = schematic(cells, size);
    for turn in 0..4 {
        let rotated = original.placed_cells(IVec3::ZERO, turn).unwrap();
        let anchor = rotated.iter().find(|(_, d)| d.container.is_some()).unwrap();
        assert_eq!(anchor.0, IVec3::ZERO);
        assert_eq!(anchor.1.kv["fixture:machine"], vec![42]);
        let state = petramond_world::block_model::ModelCellState::from_cell(anchor.1.state);
        for (p, offset) in
            petramond_world::block_model::oriented_footprint_cells(IVec3::ZERO, kind, state.facing)
        {
            assert!(rotated.iter().any(|(at, d)| *at == p
                && petramond_world::block_model::ModelCellState::from_cell(d.state).offset
                    == offset));
        }
    }
    assert!(original.placed_cells(IVec3::splat(i32::MAX), 1).is_err());
}

#[test]
fn creative_modes_survive_player_save_restore() {
    for mode in [
        crate::player::PlayerMode::Creative,
        crate::player::PlayerMode::CreativeFlying,
    ] {
        let mut p =
            crate::player::Player::new(petramond_math::world_pos::WorldPos::new(1.0, 64.0, 1.0));
        p.set_mode(mode);
        let bytes = crate::save::player::encode(&p);
        assert_eq!(
            crate::save::player::decode(&bytes)
                .unwrap()
                .restore()
                .mode(),
            mode
        );
    }
}

#[test]
fn every_registered_block_is_available_without_changing_survival_drop_mapping() {
    for &block in Block::all().iter().filter(|b| **b != Block::Air) {
        let name = petramond_world::registry::names()
            .blocks
            .name(block.id())
            .unwrap();
        let ordinary = ItemType::from_block(block);
        assert!(!ordinary.creative_only());
        let item = if ordinary == ItemType::Air {
            ItemType::by_name(&format!("petramond:creative/{name}")).unwrap()
        } else {
            ordinary
        };
        assert_eq!(item.as_block(), Some(block));
    }
}

#[test]
fn a_full_world_height_schematic_has_a_valid_preview_and_placement() {
    let min = petramond_world::chunk::WORLD_MIN_Y;
    let height = petramond_world::chunk::WORLD_MAX_Y - min;
    let schematic = schematic(
        vec![
            (IVec3::ZERO, cell(Block::Stone)),
            (IVec3::new(0, height - 1, 0), cell(Block::Stone)),
        ],
        [1, height, 1],
    );
    let scene = Scene::prepare(&schematic, 0, |_| {}).unwrap();
    assert_eq!(scene.size, [1, height, 1]);
    assert_eq!(scene.cells.len(), 2);
    assert!(schematic.placed_cells(IVec3::new(0, min, 0), 0).is_ok());
    assert!(schematic
        .placed_cells(IVec3::new(0, min + 1, 0), 0)
        .is_err());
}
