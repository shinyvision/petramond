use super::*;
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

fn base() -> String {
    let (text, _) =
        petramond_world::assets::read_base_text("mobs.json").expect("assets/mobs.json must ship");
    text
}

#[test]
fn shipped_mobs_json_loads_fully() {
    let defs = parse_layers(&[&base()])
        .unwrap_or_else(|e| panic!("shipped mobs.json: {e}"))
        .defs;
    assert_eq!(
        defs.len(),
        ENGINE_MOB_NAMES.len(),
        "the base table is exactly the engine set"
    );
    for (i, d) in defs.iter().enumerate() {
        assert_eq!(d.mob, Mob(i as u8));
        assert_eq!(d.name, ENGINE_MOB_NAMES[i]);
    }
}

#[test]
fn consumer_data_patches_override_only_their_named_entry() {
    let first = r#"{"mobs":[{"patch":"petramond:owl","data":{"fixture:diet":{"restore":2},"fixture:other":true}}]}"#;
    let second = r#"{"mobs":[{"patch":"petramond:owl","data":{"fixture:diet":{"restore":5}}}]}"#;
    let loaded = parse_layers(&[&base(), first, second]).unwrap();
    let row = loaded.defs.iter().find(|row| row.mob == Mob::Owl).unwrap();
    assert_eq!(row.data_value("fixture:diet"), Some(r#"{"restore":5}"#));
    assert_eq!(row.data_value("fixture:other"), Some("true"));
    assert_eq!(row.data_value("fixture:missing"), None);
    assert!(parse_layers(&[
        &base(),
        r#"{"mobs":[{"patch":"fixture:absent","data":{"fixture:diet":{}}}]}"#
    ])
    .is_err());
    assert!(parse_layers(&[
        &base(),
        r#"{"mobs":[{"patch":"petramond:owl","data":{"bare":{}}}]}"#
    ])
    .is_err());
}

#[test]
fn loader_rejects_geometry_that_is_non_finite_or_unbounded() {
    let invalid = |edit: fn(&mut serde_json::Value)| {
        let mut value: serde_json::Value = serde_json::from_str(&base()).unwrap();
        edit(&mut value["mobs"][0]);
        let text = serde_json::to_string(&value).unwrap();
        parse_layers(&[&text])
            .map(|_| ())
            .expect_err("invalid geometry must fail during catalog load")
    };

    let zero_width = invalid(|row| row["size"]["half_width"] = serde_json::json!(0.0));
    assert!(zero_width.contains("half_width"), "{zero_width}");

    let excessive_segments = invalid(|row| {
        row["size"] = serde_json::json!({
            "half_width": 0.01,
            "height": 1.0,
            "half_length": 1.0
        });
    });
    assert!(
        excessive_segments.contains("segments"),
        "{excessive_segments}"
    );

    let narrowed_infinite_seat = invalid(|row| {
        row["seats"] = serde_json::json!([[1e300, 0.0, 0.0]]);
    });
    assert!(
        narrowed_infinite_seat.contains("finite f32"),
        "{narrowed_infinite_seat}"
    );
}

#[test]
fn pack_layer_overrides_rows_by_mob() {
    let layer = r#"{"mobs": [{
            "mob": "petramond:owl", "key": "petramond:owl", "model": "models/owl.bbmodel", "scale": 0.5,
            "size": {"half_width": 0.3, "height": 0.9}, "tags": {"petramond:health": 6.0},
            "walk_speed": 3.0, "jump_speed": 7.2, "turn_rate": 7.0, "walk_anim_rate": 1.2,
            "category": "passive", "cap": 4,
            "spawn": {"biomes": ["forest"], "ground": ["petramond:grass"]},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": ["forest"]},
            "avoid_fluids": true,
            "brain": [{"node": "wander", "priority": 0}]
        }]}"#;
    let defs = parse_layers(&[&base(), layer])
        .expect("layered table loads")
        .defs;
    assert_eq!(defs.len(), ENGINE_MOB_NAMES.len(), "an override adds no id");
    let owl = &defs[Mob::Owl.0 as usize];
    assert_eq!(owl.scale, 0.5);
    assert_eq!(owl.cap, 4);
    assert_eq!(owl.brain.len(), 1);
}

#[test]
fn brain_extensions_apply_as_a_side_table_and_bad_ones_degrade() {
    let layer = r#"{"mobs": [], "brain_extensions": [
            {"mob": "petramond:sheep", "brain": [{"node": "mymod:lure", "priority": 20, "inputs": ["player_held"]}]},
            {"mob": "gone:species", "brain": [{"node": "wander"}]},
            {"mob": "petramond:owl", "brain": [{"node": "chase_player", "params": {"radius": "not a number"}}]}
        ]}"#;
    let loaded = parse_layers(&[&base(), layer]).expect("extension layer loads");
    assert_eq!(
        loaded.defs.len(),
        ENGINE_MOB_NAMES.len(),
        "extending registers nothing"
    );
    let base_sheep_nodes = parse_layers(&[&base()]).unwrap().defs[Mob::Sheep.0 as usize]
        .brain
        .len();
    assert_eq!(
        loaded.defs[Mob::Sheep.0 as usize].brain.len(),
        base_sheep_nodes,
        "the target row's own brain stays its own"
    );
    assert_eq!(
        loaded.extensions.len(),
        1,
        "the valid extension applies; the unloaded target and the factory-rejected one degrade"
    );
    let (target, nodes) = &loaded.extensions[0];
    assert_eq!(*target, Mob::Sheep);
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].node, "mymod:lure");
    assert_eq!(nodes[0].priority, 20);
}

#[test]
fn namespaced_pack_row_registers_a_hostile_mob_with_a_data_brain() {
    let layer = r#"{"mobs": [{
            "mob": "mymod:zombling", "key": "mymod:zombling", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.8}, "tags": {"petramond:health": 20.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "hostile", "despawn_radius": 64.0, "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "brain": [
                {"node": "wander", "priority": 0},
                {"node": "chase_player", "priority": 20, "params": {"radius": 12.0, "give_up_radius": 18.0}},
                {"node": "melee_attack", "priority": 30, "params": {"reach": 1.2, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20}}
            ]
        }]}"#;
    let defs = parse_layers(&[&base(), layer])
        .expect("dynamic row loads")
        .defs;
    let engine = ENGINE_MOB_NAMES.len();
    assert_eq!(defs.len(), engine + 1, "a fresh id past the engine set");
    let z = &defs[engine];
    assert_eq!(z.mob, Mob(engine as u8));
    assert_eq!(z.name, "mymod:zombling");
    assert_eq!(z.category, MobCategory::Hostile);
    assert_eq!(z.despawn.map(|d| d.radius), Some(64.0));
    assert!(
        !z.spawn.is_spawnable(),
        "an empty spawn rule = programmatic-spawn-only"
    );

    let mut brain = super::super::build_brain(z);
    let world = {
        use petramond_world::block::Block;
        use petramond_world::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
        let mut w = crate::world::ServerWorld::new(0, 1);
        let mut c = Chunk::new(0, 0);
        for zz in 0..CHUNK_SZ {
            for xx in 0..CHUNK_SX {
                c.set_block(xx, 63, zz, Block::Grass);
            }
        }
        w.insert_chunk_for_test(ChunkPos::new(0, 0), c);
        w
    };
    let mut rng = super::super::MobRng::new(1);
    let mob_pos = WorldPos::new(2.5, 64.0, 2.5);
    let player = WorldPos::new(3.7, 64.9, 2.5);
    let players = [crate::mob::PlayerAnchor {
        pos: player,
        ..Default::default()
    }];
    let mut ctx = crate::mob::behavior::test_support::ctx_at(&world, &mut rng, mob_pos);
    ctx.yaw = -std::f32::consts::FRAC_PI_2;
    ctx.head_height = z.size.height;
    ctx.half_width = z.size.half_width;
    ctx.player_pos = player;
    ctx.head = z.size.head_cells();
    let decision = brain.decide(&mut ctx);
    assert_eq!(
        decision.goal,
        Some(petramond_math::math::IVec3::new(3, 64, 2)),
        "chase_player steers navigation at the player's cell"
    );
    assert!(
        decision.target.is_some(),
        "the engaged chase publishes its lock"
    );
    ctx.players = &players;
    ctx.target = decision.target;
    let attack = brain
        .decide(&mut ctx)
        .attack
        .expect("melee_attack strikes the locked player in reach");
    assert_eq!(attack.damage, 2.0);
    assert_eq!(attack.knockback, 5.0);
}

#[test]
fn mob_sound_hooks_resolve_registered_sound_keys() {
    let layer = r#"{"mobs": [{
            "mob": "mymod:caller", "key": "mymod:caller", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "sounds": [
                {"category": "idle", "sound": "petramond:item_pickup", "tick_interval": 40, "tick_interval_variance": 10},
                {"category": "hurt", "sound": "petramond:wood_punch"},
                {"category": "death", "sound": "petramond:wood_break"}
            ],
            "brain": []
        }]}"#;
    let defs = parse_layers(&[&base(), layer])
        .expect("sound hooks load")
        .defs;
    let caller = defs
        .iter()
        .find(|d| d.name == "mymod:caller")
        .expect("dynamic mob registered");
    assert_eq!(
        caller
            .sound_for(super::super::MobSoundCategory::Idle)
            .expect("idle hook")
            .tick_interval,
        Some(40)
    );
    assert!(
        caller
            .sound_for(super::super::MobSoundCategory::Hurt)
            .is_some(),
        "hurt hook resolved"
    );
    assert!(
        caller
            .sound_for(super::super::MobSoundCategory::Death)
            .is_some(),
        "death hook resolved"
    );
}

#[test]
fn empty_damage_feedback_row_resolves_to_default_components() {
    let layer = r#"{"mobs": [{
            "mob": "mymod:dummy", "key": "mymod:dummy", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "damage_feedback": [],
            "brain": []
        }]}"#;
    let defs = parse_layers(&[&base(), layer])
        .expect("damage feedback row loads")
        .defs;
    let dummy = defs
        .iter()
        .find(|d| d.name == "mymod:dummy")
        .expect("dynamic mob registered");
    assert_eq!(dummy.damage_feedback, MobDamageFeedback::default());
}

#[test]
fn step_noise_defaults_on_and_a_row_can_silence_it() {
    let row = |name: &str, extra: &str| {
        format!(
            r#"{{
            "mob": "{name}", "key": "{name}", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {{"half_width": 0.3, "height": 1.0}}, "tags": {{"petramond:health": 4.0}},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {{"biomes": [], "ground": []}},
            "spawn_group": {{"min": 1, "max": 1}},
            "wander": {{"chance_per_tick": 0.0125, "radius": 8}},
            "habitat": {{"avoid": [], "prefer": []}},
            "avoid_fluids": false,{extra}
            "brain": []
        }}"#
        )
    };
    let layer = format!(
        r#"{{"mobs": [{}, {}]}}"#,
        row("mymod:walker", ""),
        row("mymod:cart", r#" "step_noise": false,"#)
    );
    let defs = parse_layers(&[&base(), &layer]).expect("rows load").defs;
    let noisy = |name: &str| defs.iter().find(|d| d.name == name).unwrap().step_noise;
    assert!(noisy("mymod:walker"), "an unspecified row is heard walking");
    assert!(
        !noisy("mymod:cart"),
        "a silenced row never makes step noise"
    );
    assert!(
        defs.iter()
            .filter(|d| (d.mob.0 as usize) < ENGINE_MOB_NAMES.len())
            .all(|d| d.step_noise),
        "the engine species keep their audible footsteps"
    );
}

#[test]
fn damage_feedback_components_parse_from_json_objects() {
    let layer = r#"{"mobs": [{
            "mob": "mymod:dummy", "key": "mymod:dummy", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "damage_feedback": [
                {"component": "petramond:decrease_health"},
                {"component": "petramond:flash", "duration": 0.5},
                {"component": "petramond:knockback", "scale": 1.5, "duration": 0.2},
                {"component": "petramond:sound", "when": "death"},
                {"component": "petramond:ragdoll", "joints": "detached", "impulse_scale": 0.25}
            ],
            "brain": []
        }]}"#;
    let defs = parse_layers(&[&base(), layer])
        .expect("damage feedback row loads")
        .defs;
    let dummy = defs
        .iter()
        .find(|d| d.name == "mymod:dummy")
        .expect("dynamic mob registered");
    assert_eq!(
        dummy.damage_feedback.components,
        vec![
            MobDamageFeedbackComponent::DecreaseHealth,
            MobDamageFeedbackComponent::Flash { duration: 0.5 },
            MobDamageFeedbackComponent::Knockback {
                scale: 1.5,
                duration: 0.2,
            },
            MobDamageFeedbackComponent::Sound {
                category: MobDamageSound::Death,
            },
            MobDamageFeedbackComponent::Ragdoll {
                joints: crate::mob::RagdollJoints::Detached,
                impulse_scale: 0.25
            },
        ]
    );
}

#[test]
fn ragdoll_configuration_defaults_and_rejects_unsafe_impulses() {
    let parse = |json: &str| {
        convert_damage_feedback(serde_json::from_str::<Vec<RawMobDamageFeedback>>(json).unwrap())
    };
    let default = parse(r#"[{"component":"petramond:ragdoll"}]"#).unwrap();
    assert!(matches!(
        default.components[0],
        MobDamageFeedbackComponent::Ragdoll {
            joints: super::super::RagdollJoints::Connected,
            impulse_scale: 1.0,
        }
    ));
    for impulse in [-1.0, 9.0, 1e100] {
        assert!(parse(&format!(
            r#"[{{"component":"petramond:ragdoll","impulse_scale":{impulse}}}]"#
        ))
        .is_err());
    }
    assert!(serde_json::from_str::<RawMobDamageFeedback>(
        r#"{"component":"petramond:ragdoll","joints":"unknown"}"#
    )
    .is_err());
}

#[test]
fn idle_mob_sound_requires_a_positive_interval() {
    let layer = r#"{"mobs": [{
            "mob": "mymod:caller", "key": "mymod:caller", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "sounds": [{"category": "idle", "sound": "petramond:item_pickup"}],
            "brain": []
        }]}"#;
    let err = parse_layers(&[&base(), layer])
        .map(|_| ())
        .expect_err("idle cadence is required");
    assert!(err.contains("tick_interval"), "{err}");
}

#[test]
fn unknown_and_reserved_brain_nodes_are_load_errors() {
    let row = |node: &str| {
        format!(
            r#"{{"mobs": [{{
                    "mob": "mymod:thing", "key": "mymod:thing", "model": "models/owl.bbmodel",
                    "scale": 0.25, "size": {{"half_width": 0.3, "height": 1.0}}, "tags": {{"petramond:health": 4.0}},
                    "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
                    "category": "passive", "cap": 8,
                    "spawn": {{"biomes": [], "ground": []}},
                    "spawn_group": {{"min": 1, "max": 1}},
                    "wander": {{"chance_per_tick": 0.0125, "radius": 8}},
                    "habitat": {{"avoid": [], "prefer": []}},
                    "avoid_fluids": false,
                    "brain": [{{"node": "{node}", "priority": 0}}]
                }}]}}"#
        )
    };
    let err = parse_layers(&[&base(), &row("levitate")])
        .map(|_| ())
        .expect_err("unknown node refused");
    assert!(err.contains("unknown AI node 'levitate'"), "{err}");
    parse_layers(&[&base(), &row("mymod:levitate")]).expect("scripted node key loads");
    let bad = r#"{"mobs": [{
            "mob": "mymod:thing", "key": "mymod:thing", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0},
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_fluids": false,
            "brain": [{"node": "head_look", "priority": 10, "params": {"bogus": 1}}]
        }]}"#;
    let err = parse_layers(&[&base(), bad])
        .map(|_| ())
        .expect_err("stray params refused");
    assert!(err.contains("takes no params"), "{err}");
}

#[test]
fn bare_additions_and_bad_references_are_rejected() {
    let bare = r#"{"mobs": [{"mob": "zombling", "key": "zombling", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0}, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_fluids": false, "brain": []}]}"#;
    let err = parse_layers(&[&base(), bare])
        .map(|_| ())
        .expect_err("bare additions refused");
    assert!(
        err.contains("zombling") && err.contains("namespace"),
        "{err}"
    );

    let bad_biome = r#"{"mobs": [{"mob": "mymod:z", "key": "mymod:z", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0}, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": ["atlantis"], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_fluids": false, "brain": []}]}"#;
    let err = parse_layers(&[&base(), bad_biome])
        .map(|_| ())
        .expect_err("unknown biome refused");
    assert!(err.contains("atlantis"), "{err}");

    let bad_companion = r#"{"mobs": [{"mob": "mymod:z", "key": "mymod:z", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0}, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8,
                       "cohesion": {"companion": "ghost", "search_radius_multiplier": 2}},
            "habitat": {"avoid": [], "prefer": []}, "avoid_fluids": false, "brain": []}]}"#;
    let err = parse_layers(&[&base(), bad_companion])
        .map(|_| ())
        .expect_err("unknown companion refused");
    assert!(err.contains("ghost"), "{err}");
}

#[test]
fn loader_rejects_incomplete_tables_and_duplicate_keys() {
    let (owl_only, _) = {
        let full: serde_json::Value = serde_json::from_str(&base()).unwrap();
        let owl = full["mobs"][0].clone();
        (serde_json::json!({ "mobs": [owl] }).to_string(), ())
    };
    let err = parse_layers(&[&owl_only])
        .map(|_| ())
        .expect_err("partial tables refused");
    assert!(err.contains("missing row"), "{err}");

    let clash = r#"{"mobs": [{"mob": "mymod:z", "key": "petramond:owl", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "tags": {"petramond:health": 4.0}, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_fluids": false, "brain": []}]}"#;
    let err = parse_layers(&[&base(), clash])
        .map(|_| ())
        .expect_err("duplicate keys refused");
    assert!(err.contains("duplicate key"), "{err}");
}

#[test]
fn dynamic_pack_mob_flows_end_to_end() {
    let root = petramond_util::test_dirs::TestScratchDir::new("mobpack");
    let pack = root.join("mods/testmob");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{ "name": "Test Mob", "id": "testmob", "description": "dynamic mob fixture" }"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("mobs.json"),
        r#"{"mobs": [{
                "mob": "testmob:zombling", "key": "testmob:zombling",
                "model": "models/owl.bbmodel", "scale": 0.25,
                "size": {"half_width": 0.3, "height": 1.8}, "tags": {"petramond:health": 20.0},
                "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
                "category": "hostile", "cap": 8,
                "spawn": {"biomes": [], "ground": []},
                "spawn_group": {"min": 1, "max": 1},
                "wander": {"chance_per_tick": 0.0125, "radius": 8},
                "habitat": {"avoid": [], "prefer": []},
                "avoid_fluids": false,
                "brain": [
                    {"node": "wander", "priority": 0},
                    {"node": "chase_player", "priority": 20, "params": {"radius": 12.0, "give_up_radius": 18.0}},
                    {"node": "melee_attack", "priority": 30, "params": {"reach": 1.2, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20}}
                ]
            }]}"#,
    )
    .unwrap();

    let save = root.join("save");
    crate::modding::tests::with_fixture_content(&root, || dynamic_pack_mob_inner(&save));
}

fn dynamic_pack_mob_inner(save: &std::path::Path) {
    use super::super::{def, defs, Mob, Mobs};
    use crate::world::ServerWorld;

    let engine = ENGINE_MOB_NAMES.len();
    assert_eq!(defs().len(), engine + 1);
    let z = Mob(engine as u8);
    assert_eq!(def(z).name, "testmob:zombling");
    assert_eq!(
        serde_json::to_value(z).unwrap(),
        serde_json::Value::String("testmob:zombling".into())
    );

    let world = ServerWorld::new(0, 1);
    let mut mobs = Mobs::new(0);
    let home = WorldPos::new(8.0, 64.0, 8.0);
    assert!(mobs.spawn(z, home, 0.0));

    let near = home + Vec3::new(4.0, 0.0, 0.0);
    let far = home + Vec3::new(500.0, 0.0, 0.0);
    for _ in 0..40 {
        mobs.tick(
            0.05,
            &world,
            &[crate::mob::PlayerAnchor {
                pos: near,
                ..Default::default()
            }],
            false,
        );
    }
    assert_eq!(mobs.len(), 1, "a near player keeps the hostile mob alive");
    mobs.tick(
        0.05,
        &world,
        &[crate::mob::PlayerAnchor {
            pos: far,
            ..Default::default()
        }],
        false,
    );
    assert!(
        mobs.is_empty(),
        "a far player culls the hostile mob immediately"
    );

    std::fs::create_dir_all(save).unwrap();
    let blocks: Vec<&str> = petramond_world::block::ENGINE_BLOCK_NAMES.to_vec();
    let items: Vec<&str> = petramond_world::item::ENGINE_ITEM_NAMES.to_vec();
    std::fs::write(
        save.join("palette.json"),
        serde_json::json!({
            "blocks": blocks,
            "items": items,
            "mobs": ["petramond:owl", "othermod:phantom", "petramond:sheep"],
        })
        .to_string(),
    )
    .unwrap();
    let p = crate::save::palette::load_or_create(save, &Default::default()).unwrap();
    for &m in Mob::all() {
        let disk = p.mob_to_disk(m.id()).expect("every enabled species pins");
        assert_eq!(
            p.mob_from_disk(disk),
            Some(m.id()),
            "{m:?} round-trips by name"
        );
    }
    assert_eq!(p.mob_to_disk(Mob::Owl.id()), Some(0));
    assert_eq!(
        p.mob_to_disk(Mob::Sheep.id()),
        Some(2),
        "remapped past the stranger"
    );
    assert_eq!(
        p.mob_to_disk(z.id()),
        Some(3),
        "the dynamic mob was appended"
    );
    assert_eq!(
        p.mob_from_disk(1),
        None,
        "the unknown disk name decodes to a skip, never a wrong species"
    );
    let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
    assert!(
        text.contains("testmob:zombling"),
        "the dynamic mob is pinned in palette.json"
    );
}

#[test]
fn wander_avoid_ground_resolves_tags_and_forgives_unknown_ones() {
    let defs = parse_layers(&[&base()]).expect("base loads").defs;
    let sheep = defs
        .iter()
        .find(|d| d.key == "petramond:sheep")
        .expect("sheep row");
    assert!(
        !sheep.wander.avoid_ground.is_empty(),
        "the sheep's avoid_ground tag resolves to its members"
    );

    let mut value: serde_json::Value = serde_json::from_str(&base()).unwrap();
    let row = value["mobs"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|r| r["mob"] == "petramond:sheep")
        .unwrap();
    row["wander"]["avoid_ground"] = serde_json::json!(["ghost_pack:lava_crust"]);
    let text = serde_json::to_string(&value).unwrap();
    let defs = parse_layers(&[&text])
        .expect("an unloaded pack's tag is not an error")
        .defs;
    let sheep = defs.iter().find(|d| d.key == "petramond:sheep").unwrap();
    assert!(
        sheep.wander.avoid_ground.is_empty(),
        "unknown tag = empty set"
    );
}

#[test]
fn spawn_chances_resolve_aligned_and_bad_rows_fail_the_load() {
    let owl_with = |edit: fn(&mut serde_json::Value)| {
        let mut value: serde_json::Value = serde_json::from_str(&base()).unwrap();
        let row = value["mobs"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["mob"] == "petramond:owl")
            .unwrap();
        row["spawn"]["biomes"] = serde_json::json!(["forest", "redwood_forest"]);
        edit(row);
        serde_json::to_string(&value).unwrap()
    };

    let text = owl_with(|row| {
        row["spawn"]["chances"] = serde_json::json!({"redwood_forest": 0.25});
    });
    let defs = parse_layers(&[&text])
        .expect("a valid chance map loads")
        .defs;
    let spawn = &defs[Mob::Owl.0 as usize].spawn;
    use petramond_world::biome::Biome;
    assert_eq!(spawn.chance_in(Biome::FOREST), 1.0, "unmapped listed biome");
    assert_eq!(spawn.chance_in(Biome::REDWOOD_FOREST), 0.25, "mapped biome");
    assert_eq!(spawn.chance_in(Biome::DESERT), 0.0, "unlisted biome");

    let unlisted = parse_layers(&[&owl_with(|row| {
        row["spawn"]["chances"] = serde_json::json!({"desert": 0.5});
    })])
    .map(|_| ())
    .expect_err("a chance for a biome outside the spawn list fails");
    assert!(
        unlisted.contains("not in the spawn biomes list"),
        "{unlisted}"
    );

    let zeroed = parse_layers(&[&owl_with(|row| {
        row["spawn"]["chances"] = serde_json::json!({"forest": 0.0});
    })])
    .map(|_| ())
    .expect_err("a zero chance fails (drop the biome instead)");
    assert!(zeroed.contains("(0, 1]"), "{zeroed}");

    let text = owl_with(|row| {
        row["spawn"]["chance"] = serde_json::json!(0.25);
        row["spawn"]["chances"] = serde_json::json!({"redwood_forest": 0.5});
    });
    let defs = parse_layers(&[&text])
        .expect("a species-wide chance loads")
        .defs;
    let spawn = &defs[Mob::Owl.0 as usize].spawn;
    assert_eq!(
        spawn.chance_in(Biome::FOREST),
        0.25,
        "unmapped listed biome"
    );
    assert_eq!(
        spawn.chance_in(Biome::REDWOOD_FOREST),
        0.125,
        "both applied"
    );
    assert_eq!(spawn.chance_in(Biome::DESERT), 0.0, "unlisted biome");

    let bad = parse_layers(&[&owl_with(|row| {
        row["spawn"]["chance"] = serde_json::json!(1.5);
    })])
    .map(|_| ())
    .expect_err("a species-wide chance above 1 fails");
    assert!(bad.contains("(0, 1]"), "{bad}");
}
