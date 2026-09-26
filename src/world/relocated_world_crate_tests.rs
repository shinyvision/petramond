//! Engine integration tests whose save and placement assertions cannot live in
//! `petramond-world`. Data-only behaviour, shape, and query tests live beside
//! their implementations in that crate.

#[cfg(test)]
mod registry_palette {
    use crate::world::ServerWorld;
    use petramond_world::registry::names;
    /// End-to-end dynamic registration: a real pack (blocks.json + items.json
    /// under a fixture mods dir) registers a namespaced block + item, the
    /// block is placeable/breakable through `World`, and the save palette pins
    /// the dynamic entry by name with engine ids stable.
    ///
    /// The packs load into a content registry of the test's own, pinned on
    /// this thread — the process registry and every other test never see it.
    #[test]
    fn dynamic_pack_content_flows_end_to_end() {
        let root = std::env::temp_dir().join(format!("petramond-dynpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/testmod");
        std::fs::create_dir_all(&pack).unwrap();
        // `id` is mandatory since 2b: the pack introduces `testmod:` keys, and
        // namespaced keys must carry the owning pack's id.
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Test Mod", "id": "testmod", "description": "dynamic registration fixture" }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("blocks.json"),
            r#"{ "blocks": [ { "block": "testmod:glowrock", "shape": "cube", "flags": ["solid", "opaque", "ao_occluder"], "tags": [], "behavior": "inert", "interaction": "none", "collision": [{"min": [0, 0, 0], "max": [1, 1, 1]}], "emission": 28, "tiles": ["stone", "stone", "stone"], "material": "stone", "data": {"petramond:harvest": {"tier": 1}}, "hardness": 2, "drops": [{"item": "testmod:glowrock", "min": 1, "max": 1, "chance": 1.0}] } ] }"#,
        )
        .unwrap();
        std::fs::write(
            pack.join("items.json"),
            r#"{ "items": [ { "item": "testmod:glowrock", "key": "testmod:glowrock", "name": "Glowrock", "max_stack_size": 64, "held_pose": {"pitch": 0, "yaw": 1.8, "roll": 0}, "tags": [], "block": "testmod:glowrock" } ] }"#,
        )
        .unwrap();
        // Integration overlays: the one for an installed pack registers its
        // row, the one for a pack that is not there registers nothing.
        let helper = root.join("mods/helper");
        std::fs::create_dir_all(&helper).unwrap();
        std::fs::write(
            helper.join("pack.json"),
            r#"{ "name": "Helper", "id": "helper", "description": "integration target" }"#,
        )
        .unwrap();
        for (target, item) in [("helper", "bridge"), ("absent", "ghost")] {
            let overlay = pack.join("integrations").join(target);
            std::fs::create_dir_all(&overlay).unwrap();
            std::fs::write(
                overlay.join("items.json"),
                format!(
                    r#"{{ "items": [ {{ "item": "testmod:{item}", "key": "testmod:{item}", "name": "{item}", "max_stack_size": 64, "held_pose": {{"pitch": 0, "yaw": 0, "roll": 0}}, "sprite": "stick", "tags": [] }} ] }}"#
                ),
            )
            .unwrap();
        }

        let save = root.join("save");
        crate::modding::tests::with_fixture_content(&root, || dynamic_pack_world_inner(&save));
    }

    /// The assertions, against the fixture registry pinned above.
    fn dynamic_pack_world_inner(save: &std::path::Path) {
        use petramond_world::block::Block;
        use petramond_world::chunk::{Chunk, ChunkPos};
        use petramond_world::item::ItemType;

        let engine_blocks = petramond_world::block::ENGINE_BLOCK_NAMES.len();
        let engine_items = petramond_world::item::ENGINE_ITEM_NAMES.len();

        // --- Registration: one fresh id past each engine set, name-addressed. ---
        assert_eq!(Block::all().len(), engine_blocks + 1);
        // The glowrock plus the installed integration's row; the overlay for
        // the absent pack contributed nothing.
        // Block rows without explicit items also get derived creative entries.
        assert!(ItemType::all().len() >= engine_items + 2);
        assert!(ItemType::by_name("testmod:bridge").is_some());
        assert!(ItemType::by_name("testmod:ghost").is_none());
        let glow = Block(engine_blocks as u16);
        let glow_item = ItemType(engine_items as u16);
        assert_eq!(names().blocks.id("testmod:glowrock"), Some(glow.0));
        // Serde speaks registry names for dynamic content too.
        assert_eq!(
            serde_json::to_value(glow).unwrap(),
            serde_json::Value::String("testmod:glowrock".into())
        );
        assert_eq!(
            serde_json::from_value::<Block>(serde_json::Value::String("testmod:glowrock".into()))
                .unwrap(),
            glow
        );

        // --- The def resolved like any engine row's. ---
        assert!(glow.is_solid() && glow.is_opaque());
        assert_eq!(glow.behavior().key(), "inert");
        assert_eq!(glow.light_emission(), 28);
        assert_eq!(glow.hardness(), 2.0);
        assert_eq!(glow.drop_spec().drops.len(), 1);
        assert_eq!(glow.drop_spec().drops[0].item, glow_item);
        // The item links back to its block both ways.
        assert_eq!(glow_item.as_block(), Some(glow));
        assert_eq!(ItemType::from_block(glow), glow_item);
        assert_eq!(glow.to_item(), glow_item);

        // --- Placeable + breakable through World. ---
        let mut w = ServerWorld::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let (x, y, z) = (5, 64, 5);
        assert!(w.set_block_world(x, y, z, glow), "placement succeeds");
        assert_eq!(Block::from_id(w.data.chunk_block(x, y, z)), glow);
        assert!(
            !w.data.collision_boxes_at(x, y, z).is_empty(),
            "the placed block collides via its row's boxes"
        );
        assert!(w.set_block_world(x, y, z, Block::Air), "break succeeds");
        assert_eq!(Block::from_id(w.data.chunk_block(x, y, z)), Block::Air);

        // --- Save palette: dynamic entry pinned by name, engine ids stable. ---
        // An "old" palette written before the mod existed, with a stranger
        // entry so disk ids and runtime ids genuinely diverge.
        std::fs::create_dir_all(save).unwrap();
        let mut blocks: Vec<&str> = petramond_world::block::ENGINE_BLOCK_NAMES.to_vec();
        blocks.push("othermod:stranger");
        let items: Vec<&str> = petramond_world::item::ENGINE_ITEM_NAMES.to_vec();
        std::fs::write(
            save.join("palette.json"),
            serde_json::json!({ "blocks": blocks, "items": items }).to_string(),
        )
        .unwrap();
        let p = crate::save::palette::load_or_create(save, &Default::default()).unwrap();
        for &b in Block::all() {
            assert_eq!(p.block_from_disk(p.block_to_disk(b.id())), b.id(), "{b:?}");
        }
        for id in 0..engine_blocks as u16 {
            assert_eq!(
                p.block_to_disk(id),
                id,
                "engine block ids are identity here"
            );
        }
        // The dynamic block was appended AFTER the stranger, so its disk id
        // differs from its runtime id — the palette remaps by name.
        assert_eq!(p.block_to_disk(glow.0), engine_blocks as u16 + 1);
        let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
        assert!(
            text.contains("testmod:glowrock"),
            "the dynamic entry is pinned in palette.json"
        );
    }
}

#[cfg(test)]
mod world_ladder {
    use crate::world::ServerWorld;
    use petramond_math::facing::Facing;
    use petramond_math::math::IVec3;
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world() -> ServerWorld {
        let mut w = ServerWorld::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn a_committed_wall_panel_is_the_facing_row_and_no_block_entity() {
        use crate::world::placement::PlacementPlan;
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        let wall = petramond_world::ladder::support_cell(p, Facing::East);
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        // The ladder family's plan resolves the held (base) row to the facing
        // sibling; the commit is the generic write path.
        let plan = PlacementPlan::single(
            p,
            Block::Ladder.wall_panel_row(Facing::East),
            petramond_world::block::ShapeState::NONE,
        );
        assert!(w.commit_placement(&plan, true));
        assert_eq!(
            Block::from_id(w.data.chunk_block(p.x, p.y, p.z)),
            Block::LadderEast
        );
        // The point of facing-as-identity: a ladder-only section never
        // classifies as a block-entity section (no per-tick furnace fan-out,
        // no per-frame chest/door collection walks it).
        assert!(
            w.data.block_entity_sections.is_empty(),
            "a ladder must not index its section as a block-entity section"
        );
    }
}
