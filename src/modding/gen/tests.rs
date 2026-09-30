use super::*;
use petramond_world::chunk::SectionPos;
use petramond_worldgen::ChunkGenerator;

#[test]
fn invalid_plan_members_reject_the_whole_generation_output() {
    let good = ([0, 0, 0], mod_api::BlockId(Block::Stone.id()));
    assert!(validated_writes(
        mod_api::GenOutput {
            blocks: vec![good, ([1, 0, 0], mod_api::BlockId(u16::MAX))],
            ..Default::default()
        },
        1,
        1
    )
    .is_err());
    assert!(validated_writes(
        mod_api::GenOutput {
            blocks: vec![good],
            structures: vec![mod_api::StructurePlacement {
                template: "fixture:missing".into(),
                origin: [0, 0, 0],
                turn: 0,
            }],
            ..Default::default()
        },
        1,
        1
    )
    .is_err());
}

fn trapping_module() -> Module {
    let wat = format!(
        r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
{}  (func (export "mod_init") (param i32 i64))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64) unreachable))"#,
        crate::modding::instance::wat_abi_exports(mod_api::ABI_VERSION)
    );
    Module::new(crate::modding::host::engine(), wat.as_bytes()).expect("assemble trap guest")
}

#[test]
fn stage_replacement_conflicts_resolve_to_last_in_load_order() {
    let module = trapping_module();
    let mut b = GenHooksBuilder::new(1, ModHealthBoard::default());
    b.add_generator("alpha", &module, 7);
    b.add_stage_replacement("beta", &module, WorldgenStage::Terrain, 9);
    let hooks = b.build().expect("hooks registered");

    for stage in ALL_STAGES {
        assert!(hooks.replaces(stage), "{stage:?} is replaced");
    }
    let terrain = hooks.replacements[stage_index(WorldgenStage::Terrain)]
        .as_ref()
        .unwrap();
    assert_eq!(hooks.mods[terrain.mod_idx].id, "beta", "later mod wins");
    assert_eq!(terrain.callback_id, 9);
    let climate = hooks.replacements[stage_index(WorldgenStage::Climate)]
        .as_ref()
        .unwrap();
    assert_eq!(hooks.mods[climate.mod_idx].id, "alpha");

    assert!(GenHooksBuilder::new(1, ModHealthBoard::default())
        .build()
        .is_none());
}

#[test]
fn trapping_gen_mod_falls_back_to_the_engine_stage() {
    let module = trapping_module();
    let mut b = GenHooksBuilder::new(0x312, ModHealthBoard::default());
    b.add_stage_replacement("hostile", &module, WorldgenStage::Terrain, 1);
    b.add_stage_replacement("hostile", &module, WorldgenStage::Vegetation, 2);
    b.add_feature(
        "hostile",
        &module,
        WorldgenStage::Trees,
        3,
        Default::default(),
    );
    let hooks = b.build().expect("hooks registered");

    let seed = 0x312;
    let hooked = ChunkGenerator::with_hooks(seed, Some(hooks));
    let engine = ChunkGenerator::with_hooks(seed, None);
    for &(cx, cy, cz) in &[(0, 3, 0), (1, 4, -1), (-2, 2, 5)] {
        let col_hooked = hooked.generate_column_gen(cx, cz);
        let col_engine = engine.generate_column_gen(cx, cz);
        let sp = SectionPos::new(cx, cy, cz);
        let a = hooked.generate_section(sp, &col_hooked);
        let b = engine.generate_section(sp, &col_engine);
        assert_eq!(
            a.blocks_iter().collect::<Vec<_>>(),
            b.blocks_iter().collect::<Vec<_>>(),
            "engine fallback must match at ({cx},{cy},{cz})"
        );
    }
}

#[test]
fn a_trap_on_one_worker_disables_the_mod_on_every_worker() {
    let module = trapping_module();
    let board = ModHealthBoard::default();
    let mut b = GenHooksBuilder::new(7, board.clone());
    b.add_stage_replacement("hostile", &module, WorldgenStage::Terrain, 1);
    let hooks = b.build().expect("hooks registered");
    let heights = [64; 256];
    let biomes = [1; 256];
    let inputs = GenInputs {
        seed: 7,
        section_pos: [0, 4, 0],
        blocks: None,
        surface_heights: &heights,
        biomes: &biomes,
    };
    assert!(!board.health("hostile").is_disabled());
    let worker = std::thread::spawn({
        let hooks = Arc::clone(&hooks);
        move || {
            let inputs = GenInputs {
                seed: 7,
                section_pos: [0, 4, 0],
                blocks: None,
                surface_heights: &[64; 256],
                biomes: &[1; 256],
            };
            hooks.replace_terrain(&inputs)
        }
    });
    assert!(worker.join().unwrap().is_none(), "the worker trapped");
    assert!(
        board.health("hostile").is_disabled(),
        "the mod is down session-wide"
    );
    assert_eq!(board.disabled_since(0), vec!["hostile".to_owned()]);
    assert!(hooks.replace_terrain(&inputs).is_none());
    assert_eq!(
        board.disabled_count(),
        1,
        "disabled once, not once per thread"
    );
}

#[test]
fn authored_writes_keep_state_and_data_across_the_sections_they_span() {
    use petramond_world::block::CellView;
    use petramond_world::block_state::StairState;
    use petramond_world::door::DoorState;
    use petramond_world::facing::Facing;
    use petramond_world::section::Section;

    let mut palette = mod_api::AuthoredPalette::default();
    palette.push(
        mod_api::BlockId(Block::OakStairs.id()),
        [("facing", "east"), ("half", "top")],
    );
    palette.push(
        mod_api::BlockId(Block::OakDoor.id()),
        [("facing", "south"), ("open", "true")],
    );
    let authored = mod_api::AuthoredWrites {
        palette,
        cells: [([3, 15, 0], 0), ([5, 15, 0], 1)].into_iter().collect(),
        data: vec![mod_api::AuthoredData {
            pos: [3, 15, 0],
            key: "fixture:once".into(),
            value: b"1".to_vec(),
        }],
    };
    let plan = validated_writes(
        mod_api::GenOutput {
            authored: authored.clone(),
            ..Default::default()
        },
        1,
        1,
    )
    .expect("authored writes validate");
    let mut lower = Section::new(0, 0, 0);
    let mut upper = Section::new(0, 1, 0);
    petramond_worldgen::feature::apply_gen_plan(&mut upper, &plan);
    petramond_worldgen::feature::apply_gen_plan(&mut lower, &plan);

    let stair = StairState::from_cell(lower.cell_state(3, 15, 0));
    assert_eq!(lower.block(3, 15, 0), Block::OakStairs);
    assert_eq!(stair.facing, Facing::East);
    assert_eq!(
        lower.cell_kv_get(3, 15, 0, "fixture:once"),
        Some(b"1".as_slice())
    );
    for (section, y, top) in [(&lower, 15, false), (&upper, 0, true)] {
        assert_eq!(section.block(5, y, 0), Block::OakDoor);
        let door = DoorState::from_cell(section.cell_state(5, y, 0));
        assert_eq!(
            (door.facing, door.open, door.top),
            (Facing::South, true, top)
        );
    }

    let mut cut = authored;
    cut.palette.push(mod_api::BlockId(Block::Stone.id()), []);
    cut.cells.push([5, 16, 0], 2);
    let partial = mod_api::GenOutput {
        authored: cut,
        ..Default::default()
    };
    assert!(
        validated_writes(partial, 1, 1).is_err(),
        "half a door is rejected"
    );
}
