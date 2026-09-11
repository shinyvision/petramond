use super::*;
use petramond_world::chunk::SectionPos;
use petramond_worldgen::driver::ChunkGenerator;

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

/// A minimal guest whose init succeeds and whose every dispatch traps —
/// the "runaway/broken gen mod" for the fallback contract.
fn trapping_module() -> Module {
    let wat = r#"(module
  (import "env" "host_dispatch" (func $hd (param i32 i32) (result i64)))
  (memory (export "memory") 1)
  (func (export "mod_init"))
  (func (export "mod_alloc") (param i32) (result i32) (i32.const 4096))
  (func (export "mod_free") (param i32 i32))
  (func (export "mod_dispatch") (param i32 i32) (result i64) unreachable))"#;
    Module::new(crate::modding::host::engine(), wat.as_bytes()).expect("assemble trap guest")
}

/// Conflict contract: two mods replacing the same stage → LAST in load
/// order wins; `RegisterGenerator` claims every stage and later
/// stage-specific replacements override it per stage.
#[test]
fn stage_replacement_conflicts_resolve_to_last_in_load_order() {
    let module = trapping_module();
    let mut b = GenHooksBuilder::new(1);
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

    // Nothing registered = no config = the empty fast path.
    assert!(GenHooksBuilder::new(1).build().is_none());
}

/// Failure contract: a trapping replacement falls back to the ENGINE
/// stage and a trapping feature is skipped — the generated section is
/// byte-identical to a hookless generator's.
#[test]
fn trapping_gen_mod_falls_back_to_the_engine_stage() {
    let module = trapping_module();
    let mut b = GenHooksBuilder::new(0x312);
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
