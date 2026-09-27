use super::*;
use petramond_world::chunk::SectionPos;
use petramond_worldgen::ChunkGenerator;

#[test]
fn invalid_plan_members_reject_the_whole_generation_output() {
    let good = ([0, 0, 0], mod_api::BlockId(Block::Stone.id()));
    assert!(validated_writes(
        mod_api::GenOutput {
            features: Vec::new(),
            blocks: vec![good, ([1, 0, 0], mod_api::BlockId(u16::MAX))],
            structures: Vec::new(),
            deferred: false,
        },
        1
    )
    .is_err());
    assert!(validated_writes(
        mod_api::GenOutput {
            features: Vec::new(),
            blocks: vec![good],
            structures: vec![mod_api::StructurePlacement {
                template: "fixture:missing".into(),
                origin: [0, 0, 0],
                turn: 0,
            }],
            deferred: false,
        },
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
    let mut b = GenHooksBuilder::new(1, ModHealthBoard::default(), FuelBudget::DEFAULT);
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

    assert!(
        GenHooksBuilder::new(1, ModHealthBoard::default(), FuelBudget::DEFAULT)
            .build()
            .is_none()
    );
}

#[test]
fn trapping_gen_mod_falls_back_to_the_engine_stage() {
    let module = trapping_module();
    let mut b = GenHooksBuilder::new(0x312, ModHealthBoard::default(), FuelBudget::DEFAULT);
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
            "engine fallback must be byte-identical at ({cx},{cy},{cz})"
        );
    }
}

#[test]
fn a_trap_on_one_worker_disables_the_mod_on_every_worker() {
    let module = trapping_module();
    let board = ModHealthBoard::default();
    let mut b = GenHooksBuilder::new(7, board.clone(), FuelBudget::DEFAULT);
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
