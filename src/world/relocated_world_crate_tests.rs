//! Tests relocated from `petramond-world` modules: they drive the engine's
//! `World` wrapper (tick harness, save palette, light queue), which the data
//! crate cannot link. Module internals they exercise are exposed through each
//! source module's `test_exports` shim (test-support builds only).

#[cfg(test)]
mod behavior_dirt {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::block::behavior::test_shims::dirt::*;
    use petramond_world::block::behavior::BlockBehavior;

    use petramond_world::chunk::{Chunk, ChunkPos};

    /// A world with one loaded chunk at (0,0). Coords kept a few blocks inside the
    /// 16-wide chunk so a `SPREAD_RADIUS` scan stays within the loaded cell.
    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn grass_at_radius_is_found() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x + SPREAD_RADIUS, p.y, p.z, Block::Grass);
        assert!(grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn grass_one_past_radius_is_not_found() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x + SPREAD_RADIUS + 1, p.y, p.z, Block::Grass);
        assert!(!grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn diagonal_grass_within_radius_is_found() {
        // A corner of the box still counts: proximity is per-axis, not Euclidean.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        let c = p + IVec3::new(SPREAD_RADIUS, SPREAD_RADIUS, SPREAD_RADIUS);
        w.set_block_world(c.x, c.y, c.z, Block::Grass);
        assert!(grass_within(&w, p, SPREAD_RADIUS));
    }

    #[test]
    fn dirt_with_grass_in_range_greens_over() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass);
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn dirt_with_no_grass_in_range_stays_dirt() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn covered_dirt_does_not_green_even_with_grass_in_range() {
        // A solid block on top means grass could not survive here, so dirt must not
        // spread onto it — otherwise it would flip to grass and straight back.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::Stone); // but covered on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn dirt_under_no_grass_decay_cover_greens() {
        // Grass spreads under a NoGrassDecay cover (leaves): the dirt greens even
        // though its top is solid, because that cover does not smother grass.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves); // leaf canopy on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn submerged_dirt_does_not_green_even_with_grass_in_range() {
        // Dirt under water must stay dirt even with grass alongside — otherwise the
        // spread would creep grass down a flooded slope (terrain under water is dirt).
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Dirt);
        w.set_block_world(p.x + 1, p.y, p.z, Block::Grass); // grass in range
        w.set_block_world(p.x, p.y + 1, p.z, Block::Water); // but flooded on top
        DIRT.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }
}

#[cfg(test)]
mod behavior_grass {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::block::behavior::test_shims::grass::*;
    use petramond_world::block::behavior::BlockBehavior;

    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn grass_under_solid_dies_to_dirt() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::Stone);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }

    #[test]
    fn uncovered_grass_survives() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn grass_under_no_grass_decay_cover_survives() {
        // A solid cover tagged NoGrassDecay (leaves being the canonical carrier) does
        // not smother the grass below: it stays grass instead of dying back to dirt.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::OakLeaves);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Grass));
    }

    #[test]
    fn flooded_grass_dies_to_dirt() {
        // Water directly overhead drowns grass — it reverts to dirt, so the spread
        // can never leave grass sitting under water.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::Grass);
        w.set_block_world(p.x, p.y + 1, p.z, Block::Water);
        GRASS.random_tick(&mut w, p);
        assert_eq!(w.block_if_loaded(p.x, p.y, p.z), Some(Block::Dirt));
    }
}

#[cfg(test)]
mod behavior_leaves {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::block::behavior::test_shims::leaves::*;
    use petramond_world::block::behavior::BlockBehavior;

    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world_with_chunk() -> World {
        let mut w = World::new(1, 1);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    /// Lay a straight +x run of `len` leaves from `start`, then a log — so the log
    /// sits exactly `len` face-steps from `start` through leaves. Stays inside the
    /// 16-wide chunk for `start.x + len <= 15`.
    fn leaf_run_to_log(w: &mut World, start: IVec3, len: i32) {
        for i in 0..len {
            w.set_block_world(start.x + i, start.y, start.z, Block::OakLeaves);
        }
        w.set_block_world(start.x + len, start.y, start.z, Block::OakLog);
    }

    #[test]
    fn log_at_max_distance_supports() {
        let mut w = world_with_chunk();
        let p = IVec3::new(2, 70, 8);
        leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE);
        assert!(leaf_supported(&w, p));
    }

    #[test]
    fn log_one_step_past_max_does_not_support() {
        let mut w = world_with_chunk();
        let p = IVec3::new(2, 70, 8);
        leaf_run_to_log(&mut w, p, MAX_LOG_DISTANCE + 1);
        assert!(!leaf_supported(&w, p));
    }

    #[test]
    fn adjacent_log_supports() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        w.set_block_world(p.x + 1, p.y, p.z, Block::OakLog);
        assert!(leaf_supported(&w, p));
    }

    #[test]
    fn isolated_leaf_is_unsupported() {
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves);
        assert!(!leaf_supported(&w, p));
    }

    #[test]
    fn a_decaying_leaf_breaks_naturally_so_its_drop_can_roll() {
        // A leaf cut off from wood doesn't vanish silently: it breaks as a NATURAL
        // break, so `Game` plays the burst and rolls the leaf's drop table — the 10%
        // sapling. Here we assert the decay is recorded as a natural break (the drop
        // hand-off), independent of the probabilistic roll itself.
        let mut w = world_with_chunk();
        let p = IVec3::new(8, 70, 8);
        w.set_block_world(p.x, p.y, p.z, Block::OakLeaves); // isolated → unsupported
        LEAVES.random_tick(&mut w, p);
        assert_eq!(
            w.block_if_loaded(p.x, p.y, p.z),
            Some(Block::Air),
            "the leaf decayed"
        );
        let breaks = w.take_natural_breaks();
        assert!(
            breaks
                .iter()
                .any(|&(bp, b)| bp == p && b == Block::OakLeaves),
            "a decayed leaf is recorded as a natural break so its sapling drop rolls",
        );
    }
}

#[cfg(test)]
mod behavior_wasm {
    #[allow(unused_imports)]
    use petramond_world::block::behavior::test_shims::wasm::*;

    #[test]
    fn namespaced_keys_intern_to_shared_singletons_that_enqueue_hooks() {
        let a = petramond_world::block::behavior::by_name("testmod:zap")
            .expect("namespaced keys resolve");
        let b = petramond_world::block::behavior::by_name("testmod:zap").expect("stable");
        assert_eq!(a.key(), "testmod:zap", "key() inverts by_name()");
        assert!(
            std::ptr::eq(a as *const _ as *const u8, b as *const _ as *const u8),
            "one singleton per key"
        );
        assert!(a.has_random_tick());
        assert!(
            petramond_world::block::behavior::by_name("bogus").is_none(),
            "bare unknowns still error"
        );

        let mut world = crate::world::testutil::flat_world();
        let pos = IVec3::new(1, 65, 1);
        a.random_tick(&mut world, pos);
        a.neighbor_update(&mut world, pos);
        let hooks = world.take_block_hooks();
        assert_eq!(hooks.len(), 2);
        assert_eq!(hooks[0].kind, BlockHookKind::RandomTick);
        assert_eq!(hooks[1].kind, BlockHookKind::NeighborUpdate);
        assert_eq!(hooks[0].key, "testmod:zap");
        assert_eq!(hooks[0].pos, pos);
        assert!(world.take_block_hooks().is_empty(), "take drains");
    }
}

#[cfg(test)]
mod shape_kind {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::block::shape_kind_test_shim::*;

    /// The per-id collision table ([`Block::static_collision_boxes`]) is only
    /// sound while every kind flagged [`ShapeKindDef::collision_state_free`]
    /// really answers the same boxes with and without a cell to read. A family
    /// that grows a per-cell `collision_boxes` override must leave
    /// `families::collision_is_state_free` — and this is what says so.
    #[test]
    fn collision_state_free_kinds_resolve_identically() {
        use petramond_world::block::Block;
        use petramond_world::chunk::ChunkPos;
        let mut world = World::new(0, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        // Neighbours that a state-reading family WOULD react to (a fence arm, a
        // stair corner, a pane join), so a mis-flagged kind cannot pass by
        // being surrounded by air.
        world.set_block_world(8, 64, 8, Block::Stone);
        world.set_block_world(9, 65, 8, Block::Stone);
        world.set_block_world(8, 65, 9, Block::OakFence);
        for &block in Block::all() {
            let k = block.shape_kind_def();
            let Some(baked) = block.static_collision_boxes() else {
                assert!(
                    !k.collision_state_free,
                    "block id {} ({}) is flagged state-free but has no baked boxes",
                    block.id(),
                    k.key
                );
                continue;
            };
            for pos in [
                petramond_math::math::IVec3::new(8, 65, 8),
                petramond_math::math::IVec3::new(8, 66, 8),
                petramond_math::math::IVec3::new(3, 70, 12),
            ] {
                let live = k.sim.collision_boxes(&k.params, &world, pos, block);
                assert_eq!(
                    baked,
                    live,
                    "block id {} ({}) resolves per cell at {pos:?} but is flagged state-free",
                    block.id(),
                    k.key
                );
            }
        }
    }

    /// `RawShape` accepts the engine family strings, the parameterized tagged
    /// forms, and a bare NAMESPACED string as a custom-shape reference —
    /// while a bare unknown (non-namespaced) string is a load error.
    #[test]
    fn raw_shape_deserializes_families_params_and_named_references() {
        let de = |s: &str| serde_json::from_str::<RawShape>(s).expect("parses");
        assert!(matches!(de(r#""cube""#), RawShape::Cube));
        assert!(matches!(de(r#""fence""#), RawShape::Fence));
        assert!(matches!(de(r#""door""#), RawShape::Door));
        // A box list: extents default to the whole cell, faces to all six,
        // and a box may declare that it draws without obstructing.
        let (family, params, _) = resolve_json(
            r#"{"boxes":[{"to":[16,15,16]},{"from":[0,15,0],"faces":["up"],"collides":false}]}"#,
        )
        .unwrap();
        assert_eq!(family, ShapeFamily::BoxSet);
        let set = params.box_set().expect("box set params");
        assert_eq!(set.boxes(0, 0).len(), 2);
        assert_eq!(set.boxes(0, 0)[0].aabb.max, [1.0, 15.0 / 16.0, 1.0]);
        assert_eq!(set.boxes(0, 0)[0].faces, [true; 6]);
        assert!(set.boxes(0, 0)[0].collides);
        // Face order is +X, -X, +Y, -Y, +Z, -Z.
        assert_eq!(
            set.boxes(0, 0)[1].faces,
            [false, false, true, false, false, false]
        );
        // Only the colliding box is collision; the outline is the drawn union.
        assert_eq!(set.collision(0, 0).len(), 1);
        assert_eq!(set.bounds(0, 0).max, [1.0, 1.0, 1.0]);
        // Empty lists, inverted extents, out-of-range texels, unknown face
        // names and a tile on an undrawn face are load errors, not silently
        // dropped values.
        for bad in [
            r#"{"boxes":[]}"#,
            r#"{"boxes":[{"from":[8,0,0],"to":[8,16,16]}]}"#,
            r#"{"boxes":[{"to":[17,16,16]}]}"#,
            r#"{"boxes":[{"faces":["sideways"]}]}"#,
            r#"{"boxes":[{"tiles":{"up":"petramond:no_such_tile"}}]}"#,
            r#"{"boxes":[{"faces":["up"],"tiles":{"down":"stone"}}]}"#,
        ] {
            assert!(resolve_json(bad).is_err(), "{bad}");
        }
        assert!(matches!(
            de(r#"{"custom":{"family":"fence"}}"#),
            RawShape::Custom(_)
        ));
        match de(r#""mymod:gate""#) {
            RawShape::Named(key) => assert_eq!(key, "mymod:gate"),
            _ => panic!("a namespaced string is a custom-shape reference"),
        }
        // A bare (non-namespaced) unknown string is not a valid shape.
        assert!(serde_json::from_str::<RawShape>(r#""bogus""#).is_err());
    }

    /// Apertures ask whether matter SEALS a boundary, not whether it fills the
    /// octant behind it. A cover that stops a texel short of its top must read
    /// OPEN above — otherwise its own cell floods to black, the sunken top the
    /// mesher draws inside that cell samples the dark, and every neighbouring
    /// face averages its smooth light against a cell that is plainly lit
    /// (the 2026-07-25 black-farmland playtest bug).
    #[test]
    fn a_cover_that_stops_short_of_its_top_stays_open_to_the_light() {
        let apertures = |json: &str| {
            let (family, params, _) = resolve_json(json).unwrap();
            let (sim, ..) = families::singletons(family);
            sim.light_apertures(
                &params,
                &facets::NoNeighborhood,
                petramond_math::math::IVec3::ZERO,
                petramond_world::block::Block::Air,
            )
        };
        // 15/16 tall: fills most of its top octant, seals none of its top face.
        let farmland = apertures(r#"{"boxes":[{"to":[16,15,16]}]}"#);
        assert_eq!(
            light_aperture_face(farmland, (0, 1, 0)),
            0b1111,
            "an unsealed top must let light in"
        );
        assert_eq!(
            light_aperture_face(farmland, (0, -1, 0)),
            0,
            "its floor-flush base still seals downward"
        );
        assert_eq!(
            light_aperture_face(farmland, (1, 0, 0)),
            0,
            "its sides reach the boundary on both halves"
        );
        // An inset column under an overhanging cap: sealed top and bottom, but
        // its SIDES stay open. The cap clips the extreme texel of every side
        // quadrant, so a probe over the whole quadrant would call the cell
        // sealed and black it out — the cactus half of the same playtest bug.
        let capped = apertures(
            r#"{"boxes":[{"from":[1,0,1],"to":[15,16,15]},{"from":[0,15,0],"faces":["up"]}]}"#,
        );
        assert_eq!(light_aperture_face(capped, (0, 1, 0)), 0, "the cap seals");
        assert_eq!(
            light_aperture_face(capped, (0, -1, 0)),
            0,
            "the trunk seals its own floor"
        );
        assert_eq!(
            light_aperture_face(capped, (1, 0, 0)),
            0b1111,
            "an inset trunk leaves its recessed sides open to the light"
        );
    }

    fn resolve_json(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
        serde_json::from_str::<RawShape>(s)
            .expect("parses")
            .resolve(false)
    }

    /// [`resolve_json`] with the row's corner-joining flag set.
    fn resolve_corners(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
        serde_json::from_str::<RawShape>(s)
            .expect("parses")
            .resolve(true)
    }

    /// Turning a box set is a quarter turn about Y — an order-4 action, so
    /// four turns must land back on the authored list, geometry AND per-face
    /// data together. This is what catches a face permutation that disagrees
    /// with the extent swap: the individual turns still "look" plausible, but
    /// a shape's front art walks off its front.
    #[test]
    fn four_quarter_turns_return_a_box_set_to_its_authored_form() {
        // Deliberately asymmetric on every axis and per face, so a wrong
        // permutation cannot coincide with the right one.
        let (_, params, _) = resolve_json(
            r#"{"boxes":[
                 {"from":[1,2,3],"to":[5,14,7],"faces":["+x","up","-z"],
                  "tiles":{"up":"stone","-z":"dirt"}},
                 {"from":[0,0,9],"to":[16,1,16],"faces":["all"],"tiles":{"+x":"sand"}}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        let four: Vec<BoxDef> = set.boxes(3, 0).iter().map(BoxDef::turned).collect();
        assert_eq!(four, set.boxes(0, 0), "four quarter turns is the identity");
        // ...and no intermediate turn is: an authored front must actually move.
        for t in 1..4 {
            assert_ne!(set.boxes(0, 0), set.boxes(t, 0), "turn {t} must differ");
        }
        // One turn carries the authored -Z front to +X, matching Facing's
        // North -> East step (the convention `FRONT_AFTER_TURN` encodes).
        assert!(set.boxes(0, 0)[0].faces[5] && set.boxes(1, 0)[0].faces[FRONT_AFTER_TURN[1]]);
        assert_eq!(
            set.boxes(0, 0)[0].tiles[5],
            set.boxes(1, 0)[0].tiles[FRONT_AFTER_TURN[1]],
            "the front TILE travels with the front face"
        );
        // The collision and outline views are the same turn, not a stale
        // authored copy.
        for t in 0..4u8 {
            let boxes = set.boxes(t, 0);
            let collision: Vec<_> = boxes
                .iter()
                .filter(|b| b.collides)
                .map(|b| b.aabb)
                .collect();
            assert_eq!(set.collision(t, 0), collision, "turn {t} collision");
            for b in boxes {
                for a in 0..3 {
                    assert!(set.bounds(t, 0).min[a] <= b.aabb.min[a], "turn {t} bounds");
                    assert!(set.bounds(t, 0).max[a] >= b.aabb.max[a], "turn {t} bounds");
                }
            }
        }
    }

    /// The UV turn must exactly undo what turning the shape did to a face's
    /// cell-local UV: sampling a turned box at the turned point has to land on
    /// the same texel as sampling the authored box at the authored point, or a
    /// tile authored once cannot serve all four facings.
    ///
    /// The sides come out right for free; `+Y`/`-Y` are the two that need the
    /// correction, in OPPOSITE directions, which is exactly the pair a
    /// hand-derived sign gets backwards.
    #[test]
    fn the_uv_turn_undoes_the_shape_turn_on_every_face() {
        use petramond_math::face::Face;
        const FACES: [Face; 6] = Face::ALL;
        use petramond_mesh::plane::cell_uv;

        // A cell-local point off-centre on every axis, so no symmetry can hide
        // a mistake.
        let authored = [3.0 / 16.0, 5.0 / 16.0, 6.0 / 16.0];
        for turns in 0..4u8 {
            // The same material point after `turns` quarter turns: the turn the
            // box extents get, (x, z) -> (1 - z, x).
            let mut p = authored;
            for _ in 0..turns {
                p = [1.0 - p[2], p[1], p[0]];
            }
            for (i, face) in FACES.into_iter().enumerate() {
                // Face `i` of the turned box is authored face `a`; sampling the
                // turned face at the turned point must land where the authored
                // face sampled the authored point.
                let want = cell_uv(FACES[FACE_BEFORE_TURN_N(i, turns)], authored);
                let [u, v] = cell_uv(face, p);
                let got = petramond_world::block::ShapeFace::turn_uv(face_uv_turns(i, turns), u, v);
                assert!(
                    (got.0 - want[0]).abs() < 1e-5 && (got.1 - want[1]).abs() < 1e-5,
                    "turn {turns} face {i}: sampled {got:?}, authored {want:?}"
                );
            }
        }
    }

    /// Which authored face ends up at canonical index `i` after `turns`.
    #[allow(non_snake_case)]
    fn FACE_BEFORE_TURN_N(i: usize, turns: u8) -> usize {
        (0..turns).fold(i, |f, _| FACE_BEFORE_TURN[f])
    }

    /// Whether face `i` of `b` draws the row's `front` tile once the shape is
    /// turned `turns` — the exact predicate `families::box_set_box` applies,
    /// restated here so these tests pin the BEHAVIOUR rather than the field it
    /// happens to be derived from.
    fn draws_front(b: &BoxDef, i: usize, turns: u8) -> bool {
        i == FRONT_AFTER_TURN[((turns + b.art_turns[i]) & 3) as usize]
    }

    /// The corner forms are the stair rule lifted from quadrant masks to box
    /// lists: OUTER = the shape intersected with its quarter-turned self (the
    /// matter both perpendicular orientations agree on), INNER = the union.
    /// Straight, lone, and end-of-run cells keep the AUTHORED geometry
    /// untouched — corner joining must never change a shape's resting look
    /// (the 2026-07-25 inset misdesign changed every isolated unit and is
    /// exactly what this pins against).
    #[test]
    fn corner_forms_are_the_turned_intersection_and_union_of_the_shape() {
        // A counter: full-cell top slab over a body whose front (`-Z`) is
        // inset 2 texels.
        let (_, params, key) = resolve_corners(
            r#"{"boxes":[
                 {"from":[0,14,0],"to":[16,16,16]},
                 {"from":[0,0,2],"to":[16,14,16]}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        assert!(set.corner_joins);
        assert!(key.ends_with("+corners"), "the flag is kind identity");
        let t = |v: i32| v as f32 / 16.0;
        // STRAIGHT is byte-identical to the authored list.
        let straight = set.boxes(0, 0);
        assert_eq!(straight.len(), 2);
        assert_eq!(straight[1].aabb.min, [0.0, 0.0, t(2)]);
        assert_eq!(straight[1].aabb.max, [1.0, t(14), 1.0]);
        // OUTER: the body keeps only what a quarter-turned body also covers,
        // so the front inset wraps around the turned side; the full-cell top
        // stays whole. Form 1 = the perpendicular neighbour one turn
        // clockwise (its front toward `+X` -> its body ends at x=14).
        let outer = set.boxes(0, 1);
        let body: Vec<_> = outer.iter().filter(|b| b.aabb.max[1] < 1.0).collect();
        assert_eq!(body.len(), 1);
        assert_eq!(body[0].aabb.min, [0.0, 0.0, t(2)]);
        assert_eq!(body[0].aabb.max, [t(14), t(14), 1.0]);
        // ...and the wrapped face inherits the turned parent's authoring
        // FRAME, so the row's `front` tile lands on it too and the apron art
        // continues around the corner. The draw asks exactly this question.
        assert!(draws_front(body[0], 5, 0), "authored front still front");
        assert!(
            draws_front(body[0], 0, 0),
            "wrapped +X face draws front art"
        );
        assert!(!draws_front(body[0], 1, 0), "back-side face stays side art");
        // ...and that is what the DRAW puts on the face: TWO faces of one box
        // carry the row's `front` tile, which no single turn index can name.
        let furnace = petramond_world::block::Block::Furnace;
        let front = furnace.front_tile().expect("the furnace row has a front");
        let drawn = families::box_set_box(body[0], 0, furnace, &|_| [1.0; 3]);
        let tile_at = |i: usize| drawn.faces[i].expect("a drawn face").tile;
        assert_eq!(tile_at(5), front, "authored front");
        assert_eq!(tile_at(0), front, "wrapped corner front");
        assert_eq!(
            tile_at(1),
            furnace.tiles()[2],
            "the far side stays side art"
        );
        // INNER: the union — both bodies, straight parent first (coincident
        // tie-break), duplicates (the identical top) dropped.
        let inner = set.boxes(0, 3);
        assert_eq!(inner.len(), 3, "top + both bodies");
        assert_eq!(inner[1].aabb.max, [1.0, t(14), 1.0]);
        assert_eq!(inner[2].aabb.max, [t(14), t(14), 1.0]);
        // Turning distributes over the composition: form F at turn t is
        // turn^t of form F at turn 0.
        for form in 0..5u8 {
            let expect: Vec<_> = set.boxes(0, form).iter().map(|b| b.turned()).collect();
            assert_eq!(set.boxes(1, form), &expect[..], "form {form} turns whole");
        }
        // Collision follows the same variant; the outline never shrinks (the
        // top spans the cell in every form).
        assert_eq!(set.collision(0, 1).len(), 2);
        assert_eq!(set.bounds(0, 1).max, [1.0; 3]);
        // A stale stored byte past the vocabulary reads as STRAIGHT, never a
        // panic or a garbage index (old worlds hold old bytes until the load
        // sweep rewrites them).
        assert_eq!(set.boxes(0, 9), set.boxes(0, 0));
        // A plain box set: one form, no refinement, indexing still uniform —
        // and the five slots SHARE one leaked list rather than holding five
        // identical copies of it.
        let (_, plain, _) = resolve_json(r#"{"boxes":[{"to":[16,15,16]}]}"#).unwrap();
        let plain = plain.box_set().unwrap();
        assert!(!plain.corner_joins);
        for turns in 0..4u8 {
            for form in 0..5 {
                assert!(
                    std::ptr::eq(plain.boxes(turns, form), plain.boxes(turns, 0)),
                    "a formless kind must not leak a copy per form slot"
                );
                assert!(std::ptr::eq(
                    plain.collision(turns, form),
                    plain.collision(turns, 0)
                ));
            }
        }
        for b in plain.boxes(0, 0) {
            assert_eq!(b.art_turns, [0; 6], "authored art is in its own frame");
        }
        // The flag off a boxes shape is a load error.
        assert!(
            serde_json::from_str::<RawShape>(r#""cube""#)
                .unwrap()
                .resolve(true)
                .is_err(),
            "'corners' requires a boxes shape"
        );
    }

    /// A corner form's inherited face carries its PARENT's authoring frame,
    /// and every frame-dependent decision must read that frame rather than the
    /// cell's turn alone.
    ///
    /// The wrapped FRONT is covered above; this pins the other half, the `±Y`
    /// UV counter-rotation. It needs a shape whose intersection is bounded by
    /// the TURNED parent's top — the counter's two boxes are both full-cell or
    /// both coplanar there, so they never expose it. With `face_uv_turns` read
    /// off the cell's turn alone, this piece's inherited top tile draws a
    /// quarter turn off, invisibly for symmetric art and wrongly for anything
    /// else.
    #[test]
    fn an_inherited_top_face_is_uv_turned_by_its_parents_frame() {
        // A low full-cell shelf with its own `up` art, under a tall half-depth
        // riser. Turning the shelf is the identity; turning the riser is not.
        let (_, params, _) = resolve_corners(
            r#"{"boxes":[
                 {"from":[0,0,0],"to":[16,6,16],"tiles":{"up":"stone"}},
                 {"from":[0,0,0],"to":[16,16,8]}
               ]}"#,
        )
        .unwrap();
        let set = params.box_set().expect("box set params");
        let t = |v: i32| v as f32 / 16.0;
        // riser ∩ turn(shelf): the shelf's top bounds it, so its `+Y` face —
        // tile and frame — comes from the TURNED shelf.
        let outer = set.boxes(0, 1);
        let piece = outer
            .iter()
            .find(|b| b.aabb.max == [1.0, t(6), t(8)])
            .expect("riser clipped by the turned shelf");
        assert_eq!(
            piece.tiles[2],
            Tile::from_name("stone"),
            "inherited up tile"
        );
        assert_eq!(piece.art_turns[2], 1, "...authored one turn round");
        // A face bounded by the shape's OWN box keeps frame 0 throughout.
        let own = outer
            .iter()
            .find(|b| b.aabb.max == [1.0, t(6), 1.0])
            .expect("shelf ∩ turned shelf");
        assert_eq!(own.art_turns, [0; 6]);
        // What actually matters is the DRAW: two tops of the same form, same
        // tile, in the same cell, must be counter-rotated DIFFERENTLY because
        // they were authored in different frames. Reading the cell's turn
        // alone gives both `0` and is the bug this pins.
        let drawn_top = |b: &BoxDef| {
            families::box_set_box(b, 0, petramond_world::block::Block::Stone, &|_| [1.0; 3]).faces
                [2]
            .expect("a top face")
            .uv_turns
        };
        assert_eq!(drawn_top(piece), 1, "inherited top turns with its parent");
        assert_eq!(drawn_top(own), 0, "the shape's own top does not");
        // The frame is a RELATIVE offset, so turning the whole form carries it
        // to the face it followed and never changes its value.
        let turned = set.boxes(1, 1);
        let moved = turned
            .iter()
            .find(|b| b.aabb.min == [t(8), 0.0, 0.0] && b.aabb.max == [1.0, t(6), 1.0])
            .expect("the same piece, one turn on");
        assert_eq!(moved.art_turns[2], 1);
        assert_eq!(moved.tiles[2], Tile::from_name("stone"));
    }

    /// The secondary parameterized families (`cross`/`crop`/`wall_panel`) resolve to
    /// their engine family + `Dimensions` params, texels folded to fractions.
    #[test]
    fn custom_dimension_families_resolve_to_dimension_params() {
        let (fam, params, _) = resolve_json(r#"{"custom":{"family":"cross","inset":4}}"#).unwrap();
        assert_eq!(fam, ShapeFamily::Cross);
        assert_eq!(params.dimensions().unwrap().inset, 4.0 / 16.0);

        let (fam, params, _) =
            resolve_json(r#"{"custom":{"family":"crop","inset":3,"drop":2}}"#).unwrap();
        assert_eq!(fam, ShapeFamily::Crop);
        let d = params.dimensions().unwrap();
        assert_eq!((d.inset, d.drop), (3.0 / 16.0, 2.0 / 16.0));

        // A wall_panel is the ladder family with a retuned slab.
        let (fam, params, _) =
            resolve_json(r#"{"custom":{"family":"wall_panel","thickness":4,"height":12}}"#)
                .unwrap();
        assert_eq!(fam, ShapeFamily::Ladder);
        let d = params.dimensions().unwrap();
        assert_eq!((d.thickness, d.height), (4.0 / 16.0, 12.0 / 16.0));

        // Omitted dims fall back to the engine defaults (crop inset 2 / drop 1).
        let (_, params, _) = resolve_json(r#"{"custom":{"family":"crop"}}"#).unwrap();
        let d = params.dimensions().unwrap();
        assert_eq!((d.inset, d.drop), (2.0 / 16.0, 1.0 / 16.0));
    }

    /// Load-time validation rejects out-of-range dims, unknown families, a
    /// nonsense cross plane count, and connection fields on a dimension family.
    #[test]
    fn custom_dimension_families_validate() {
        assert!(resolve_json(r#"{"custom":{"family":"cross","inset":8}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"crop","inset":20}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"wall_panel","thickness":0}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"cross","plane_count":3}}"#).is_err());
        assert!(resolve_json(r#"{"custom":{"family":"pyramid"}}"#).is_err());
        // A connection field on a crop is almost certainly a mistake.
        assert!(resolve_json(r#"{"custom":{"family":"crop","post_thickness":4}}"#).is_err());
    }
}

#[cfg(test)]
mod registry_palette {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::registry::test_exports::*;

    #[test]
    fn tag_table_interns_namespaced_and_rejects_bare_unknowns() {
        let t = TagTable::new(&["fuel", "planks"]);
        assert_eq!(t.resolve("fuel"), Ok(0));
        assert_eq!(
            t.resolve("petramond:planks"),
            Ok(1),
            "an engine tag resolves under its namespaced recipe form too"
        );
        let a = t.resolve("mymod:ores").expect("namespaced tags intern");
        assert_eq!(t.resolve("mymod:ores"), Ok(a), "stable on re-resolution");
        assert_eq!(t.name(a), "mymod:ores");
        assert!(
            t.resolve("orees").is_err(),
            "a bare unknown is a typo'd engine tag, never a silent new tag"
        );
        assert!(
            t.resolve("petramond:fuell").is_err(),
            "the engine namespace is reserved: a typo there must not intern a \
             dead tag that nothing carries and nothing reports"
        );
        assert_eq!(
            t.lookup("petramond:fuell"),
            None,
            "and the query side never sees one either"
        );
    }

    /// The reason the ids are two bytes: with EVERY shipped pack installed the
    /// registries must still have room for more content, not a couple of dozen
    /// free ids. This reads the real installed pack set, so it fails the day
    /// the shipped packs genuinely crowd the table again.
    #[test]
    fn the_installed_pack_set_leaves_room_for_more_packs() {
        // Comfortably more than any one content pack registers, and small
        // enough that it is a real bound rather than a restatement of the cap.
        const ROOM: usize = 1024;
        let names = names();
        for (what, used) in [("block", names.blocks.len()), ("item", names.items.len())] {
            assert!(
                used <= WIDE_ID_CAP && WIDE_ID_CAP - used >= ROOM,
                "{what} registry: {used}/{WIDE_ID_CAP} ids used, only {} free — a further \
                 content pack would be refused admission",
                WIDE_ID_CAP - used,
            );
        }
    }

    #[test]
    fn namespaced_keys_register_and_bare_unknowns_error() {
        let engine = &["petramond:air", "petramond:stone"];
        // Engine override (known `petramond:*`) + a namespaced addition.
        let table = NameTable::build(
            engine,
            &[vec!["petramond:stone".into(), "mymod:gadget".into()]],
            "block",
            WIDE_ID_CAP,
        )
        .expect("valid layers");
        assert_eq!(table.len(), 3, "override adds no id; the addition does");
        assert_eq!(
            table.id("petramond:stone"),
            Some(1),
            "engine ids never move"
        );
        assert_eq!(table.id("mymod:gadget"), Some(2), "appended after engine");
        assert_eq!(table.name(2), Some("mymod:gadget"));
        // Restating a registered dynamic name in a later layer adds no id.
        let table = NameTable::build(
            engine,
            &[vec!["mymod:gadget".into()], vec!["mymod:gadget".into()]],
            "block",
            WIDE_ID_CAP,
        )
        .unwrap();
        assert_eq!(table.len(), 3);
        // A NEW bare name is an error, not a registration.
        let err = NameTable::build(engine, &[vec!["gadget".into()]], "block", WIDE_ID_CAP)
            .expect_err("bare additions are refused");
        assert!(err.contains("gadget") && err.contains("namespace"), "{err}");
        let err = NameTable::build(
            engine,
            &[vec!["petramond:gadget".into()]],
            "block",
            WIDE_ID_CAP,
        )
        .expect_err("unknown engine-namespace additions are refused");
        assert!(
            err.contains("petramond") && err.contains("reserved"),
            "{err}"
        );
        // Degenerate namespaces are not namespaces.
        for bad in [":gadget", "mymod:", ":"] {
            assert!(!is_namespaced(bad), "{bad}");
        }
        assert!(is_namespaced("mymod:gadget"));
    }

    #[test]
    fn registry_caps_at_its_declared_ceiling() {
        // The wide (block/item) ceiling and the byte ceiling are separate
        // numbers, and each catalog is held to its own.
        let engine = &["petramond:air"];
        let keys: Vec<String> = (0..WIDE_ID_CAP)
            .map(|i| format!("mymod:thing_{i}"))
            .collect();
        let err = NameTable::build(engine, std::slice::from_ref(&keys), "block", WIDE_ID_CAP)
            .expect_err("cap enforced");
        assert!(err.contains(&WIDE_ID_CAP.to_string()), "{err}");
        assert!(
            NameTable::build(
                engine,
                &[keys[..WIDE_ID_CAP - 1].to_vec()],
                "block",
                WIDE_ID_CAP
            )
            .is_ok(),
            "one under the ceiling still loads"
        );
        let byte_keys: Vec<String> = (0..BYTE_ID_CAP)
            .map(|i| format!("mymod:thing_{i}"))
            .collect();
        assert!(
            NameTable::build(engine, &[byte_keys], "mob", BYTE_ID_CAP).is_err(),
            "a byte-capped catalog is held to 256, not to the wide ceiling"
        );
    }

    /// End-to-end dynamic registration: a real pack (blocks.json + items.json
    /// under a `PETRAMOND_MODS` dir) registers a namespaced block + item, the
    /// block is placeable/breakable through `World`, and the save palette pins
    /// the dynamic entry by name with engine ids stable.
    ///
    /// The global registries are process-wide LazyLocks, so pack injection
    /// must happen before ANY test touches them — this outer test spawns the
    /// test binary again as a child with the env set, running only the
    /// `#[ignore]`d inner test below. Deterministic regardless of test order.
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

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("registry::tests::dynamic_pack_world_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("PETRAMOND_MODS", root.join("mods"))
            .env("PETRAMOND_DYNPACK_SAVE", root.join("save"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Runs ONLY in the child process spawned above (needs `PETRAMOND_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by dynamic_pack_content_flows_end_to_end with a fixture pack env"]
    fn dynamic_pack_world_inner() {
        use petramond_world::block::Block;
        use petramond_world::chunk::{Chunk, ChunkPos};
        use petramond_world::item::ItemType;

        let engine_blocks = petramond_world::block::ENGINE_BLOCK_NAMES.len();
        let engine_items = petramond_world::item::ENGINE_ITEM_NAMES.len();

        // --- Registration: one fresh id past each engine set, name-addressed. ---
        assert_eq!(Block::all().len(), engine_blocks + 1);
        assert_eq!(ItemType::all().len(), engine_items + 1);
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
        let mut w = World::new(1, 4);
        w.clear_world();
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        let (x, y, z) = (5, 64, 5);
        assert!(w.set_block_world(x, y, z, glow), "placement succeeds");
        assert_eq!(Block::from_id(w.chunk_block(x, y, z)), glow);
        assert!(
            !w.collision_boxes_at(x, y, z).is_empty(),
            "the placed block collides via its row's boxes"
        );
        assert!(w.set_block_world(x, y, z, Block::Air), "break succeeds");
        assert_eq!(Block::from_id(w.chunk_block(x, y, z)), Block::Air);

        // --- Save palette: dynamic entry pinned by name, engine ids stable. ---
        let save = std::path::PathBuf::from(std::env::var_os("PETRAMOND_DYNPACK_SAVE").unwrap());
        // An "old" palette written before the mod existed, with a stranger
        // entry so disk ids and runtime ids genuinely diverge.
        std::fs::create_dir_all(&save).unwrap();
        let mut blocks: Vec<&str> = petramond_world::block::ENGINE_BLOCK_NAMES.to_vec();
        blocks.push("othermod:stranger");
        let items: Vec<&str> = petramond_world::item::ENGINE_ITEM_NAMES.to_vec();
        std::fs::write(
            save.join("palette.json"),
            serde_json::json!({ "blocks": blocks, "items": items }).to_string(),
        )
        .unwrap();
        let p = crate::save::palette::load_or_create(&save, &Default::default()).unwrap();
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
mod world_fence {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::world::fence::test_exports::*;

    use petramond_math::facing::Facing;
    use petramond_world::block::Block;
    use petramond_world::block_state::{SlabSplit, StairHalf, StairState};
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn fence_connects_to_opaque_cubes_and_fences_but_not_transparent_blocks() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        assert_eq!(w.fence_mask_at(p), 0, "isolated fence is a bare post");

        w.set_block_world(7, 64, 8, Block::Stone);
        w.set_block_world(9, 64, 8, Block::OakFence);
        assert_eq!(
            w.fence_mask_at(p),
            petramond_world::pane::WEST | petramond_world::pane::EAST
        );

        w.set_block_world(7, 64, 8, Block::OakLeaves);
        w.set_block_world(8, 64, 9, Block::Glass);
        assert_eq!(
            w.fence_mask_at(p),
            petramond_world::pane::EAST,
            "transparent blocks must not grow fence arms"
        );
    }

    #[test]
    fn fence_connects_to_a_stair_back_but_not_its_open_side() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        // Stair east of the fence, facing east: its flat high/back side faces the fence.
        assert!(w.place_stair(
            IVec3::new(9, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.fence_mask_at(p), petramond_world::pane::EAST);

        // Stair west of the fence, also facing east: its open side faces the fence.
        assert!(w.place_stair(
            IVec3::new(7, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.fence_mask_at(p), petramond_world::pane::EAST);
    }

    #[test]
    fn fence_connects_to_a_full_slab_stack_but_not_a_single_slab() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::OakFence));
        let n = IVec3::new(8, 64, 7);
        let slot = |index| petramond_world::slab::SlabSlot {
            split: SlabSplit::Y,
            index,
        };
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(0)));
        assert_eq!(w.fence_mask_at(p), 0, "a single slab is not a full face");
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(1)));
        assert_eq!(w.fence_mask_at(p), petramond_world::pane::NORTH);
    }
}

#[cfg(test)]
mod world_ladder {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::world::ladder::test_exports::*;

    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn a_ladder_is_supported_only_by_a_complete_wall_face() {
        let mut w = world();
        let ladder = IVec3::new(8, 64, 8);
        // An east-facing ladder hangs on the wall to its west.
        let wall = petramond_world::ladder::support_cell(ladder, Facing::East);
        assert!(
            !w.ladder_supported_at(ladder, Facing::East),
            "no wall, no support"
        );
        w.set_block_world(wall.x, wall.y, wall.z, Block::Stone);
        assert!(w.ladder_supported_at(ladder, Facing::East));
        // A wall on a different side does not support this facing.
        assert!(!w.ladder_supported_at(ladder, Facing::North));
    }

    #[test]
    fn a_placed_ladder_collides_as_its_facing_resolved_panel() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        w.set_block_world(p.x, p.y, p.z, Block::LadderEast);
        let boxes = w.collision_boxes_at(p.x, p.y, p.z);
        assert_eq!(
            boxes,
            petramond_world::ladder::collision_boxes(Facing::East)
        );
        // The panel is thin, standable geometry hugging the wall side — not a
        // full cube and not empty (a body bumps it and can stand on top).
        assert_eq!(boxes.len(), 1);
        let b = &boxes[0];
        assert!(b.max[0] - b.min[0] < 0.5 || b.max[2] - b.min[2] < 0.5);
        assert_eq!((b.min[1], b.max[1]), (0.0, 1.0));
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
            Block::from_id(w.chunk_block(p.x, p.y, p.z)),
            Block::LadderEast
        );
        // The point of facing-as-identity: a ladder-only section never
        // classifies as a block-entity section (no per-tick furnace fan-out,
        // no per-frame chest/door collection walks it).
        assert!(
            w.block_entity_sections.is_empty(),
            "a ladder must not index its section as a block-entity section"
        );
    }

    #[test]
    fn climbable_query_reads_the_facing_row() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        assert_eq!(w.climb_at(p.x, p.y, p.z), None);
        w.set_block_world(p.x, p.y, p.z, Block::LadderSouth);
        assert_eq!(w.climb_at(p.x, p.y, p.z), Some(Climb::Panel(Facing::South)));
        // A non-climbable block never answers.
        w.set_block_world(p.x, p.y, p.z, Block::Stone);
        assert_eq!(w.climb_at(p.x, p.y, p.z), None);
    }
}

#[cfg(test)]
mod light_batch {
    use petramond_world::chunk::SECTION_VOLUME;

    use crate::world::light::{run_light_bake, LightBakeJob};
    #[allow(unused_imports)]
    use petramond_world::world::light::batch::test_exports::*;

    use petramond_math::facing::Facing;
    use petramond_world::block::Block;
    use petramond_world::block_state::{StairHalf, StairState};
    use petramond_world::torch::TorchPlacement;

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// Randomized rough terrain with caves, stairs, and torches across the whole
    /// 4×4×4 span (some sections absent), members straddling the surface band so
    /// Full, Dark, and Flood classifications all occur across rounds.
    #[test]
    fn batched_bake_matches_per_section_bakes() {
        let base = SectionPos::new(0, 0, 0);
        let mut rng = 0xdead_beef_cafe_1234u64;

        for round in 0..3 {
            let mut sections: FxHashMap<SectionPos, Arc<Section>> = FxHashMap::default();
            let mut columns: FxHashMap<ChunkPos, Column> = FxHashMap::default();

            // Per world column (4×4 chunk columns × 16² cells): a rough height.
            let mut heights = vec![0i32; (SPAN * SECTION_SIZE) * (SPAN * SECTION_SIZE)];
            for h in heights.iter_mut() {
                *h = (xorshift(&mut rng) % 56) as i32 - 12;
            }

            for dy in 0..SPAN {
                for dz in 0..SPAN {
                    for dx in 0..SPAN {
                        // Leave a few sections absent: absent neighbours read as air.
                        if xorshift(&mut rng).is_multiple_of(9) {
                            continue;
                        }
                        let pos = SectionPos::new(dx as i32 - 1, dy as i32 - 1, dz as i32 - 1);
                        let (_, oy, _) = pos.origin_world();
                        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
                        for ly in 0..SECTION_SIZE {
                            for lz in 0..SECTION_SIZE {
                                for lx in 0..SECTION_SIZE {
                                    let gx = dx * SECTION_SIZE + lx;
                                    let gz = dz * SECTION_SIZE + lz;
                                    let h = heights[gz * SPAN * SECTION_SIZE + gx];
                                    let wy = oy + ly as i32;
                                    // Solid below the surface with random cave holes.
                                    if wy <= h && !xorshift(&mut rng).is_multiple_of(8) {
                                        section.set_block(lx, ly, lz, Block::Stone);
                                    } else if xorshift(&mut rng).is_multiple_of(401) {
                                        section.set_block(lx, ly, lz, Block::OakStairs);
                                        section.set_stair_state(
                                            lx,
                                            ly,
                                            lz,
                                            StairState::new(Facing::East, StairHalf::Bottom),
                                        );
                                    } else if xorshift(&mut rng).is_multiple_of(353) {
                                        section.set_block(lx, ly, lz, Block::Torch);
                                        section.insert_torch(lx, ly, lz, TorchPlacement::Floor);
                                    }
                                }
                            }
                        }
                        sections.insert(pos, Arc::new(section));
                    }
                }
            }

            // Sky cover derived from the ACTUAL blocks (topmost cell that blocks
            // direct skylight), the invariant the engine maintains. Fabricating
            // cover independently of blocks creates phantom full-skylight shafts
            // through which the undecayed down rule tunnels arbitrarily deep —
            // which is exactly where 48³ and 64³ cubes may legitimately disagree.
            for dcz in 0..SPAN {
                for dcx in 0..SPAN {
                    let mut col = Column::new();
                    for lz in 0..SECTION_SIZE {
                        for lx in 0..SECTION_SIZE {
                            let mut cover = petramond_world::column::NO_SURFACE;
                            'scan: for dy in (0..SPAN).rev() {
                                let pos =
                                    SectionPos::new(dcx as i32 - 1, dy as i32 - 1, dcz as i32 - 1);
                                let Some(section) = sections.get(&pos) else {
                                    continue;
                                };
                                let blocks = section.blocks();
                                for ly in (0..SECTION_SIZE).rev() {
                                    let b = Block::from_id(blocks.get(section_idx(lx, ly, lz)));
                                    if !b.transmits_direct_skylight() {
                                        cover = pos.origin_world().1 + ly as i32;
                                        break 'scan;
                                    }
                                }
                            }
                            col.set_surface_y(lx, lz, cover);
                            col.set_sky_cover_y(lx, lz, cover);
                        }
                    }
                    columns.insert(ChunkPos::new(dcx as i32 - 1, dcz as i32 - 1), col);
                }
            }

            let member_positions: Vec<SectionPos> = (0..GROUP)
                .flat_map(|my| {
                    (0..GROUP).flat_map(move |mz| {
                        (0..GROUP).map(move |mx| {
                            SectionPos::new(base.cx + mx, base.cy + my, base.cz + mz)
                        })
                    })
                })
                .filter(|p| sections.contains_key(p))
                .collect();
            assert!(
                !member_positions.is_empty(),
                "fixture produced an empty group in round {round}"
            );

            let job = snapshot_batch(base, &member_positions, &sections, &columns)
                .expect("batch snapshot");
            let batched = run_light_bake_batch(job);
            assert_eq!(batched.len(), member_positions.len());

            fn report_first_diff<T: PartialEq + std::fmt::Debug>(
                label: &str,
                round: usize,
                pos: SectionPos,
                got: &[T],
                want: &[T],
            ) {
                for i in 0..SECTION_VOLUME {
                    if got[i] != want[i] {
                        let (lx, ly, lz) = petramond_world::chunk::section_local(i);
                        panic!(
                            "{label} mismatch at {pos:?} cell ({lx},{ly},{lz}) in round \
                             {round}: batched {:?} vs per-section {:?}",
                            got[i], want[i]
                        );
                    }
                }
            }
            for out in batched {
                let job = LightBakeJob::snapshot_unchecked(1, out.pos, &sections, &columns)
                    .expect("per-section snapshot");
                let want = run_light_bake(job);
                report_first_diff("skylight", round, out.pos, &out.skylight, &want.skylight);
                report_first_diff(
                    "block light",
                    round,
                    out.pos,
                    &out.blocklight,
                    &want.blocklight,
                );
            }
        }
    }
}

#[cfg(test)]
mod world_pane {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::world::pane::test_exports::*;

    use petramond_math::facing::Facing;
    use petramond_world::block::Block;
    use petramond_world::block_state::{SlabSplit, StairHalf, StairState};
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn pane_connects_to_full_cubes_and_panes_but_not_tagged_irregulars() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
        assert_eq!(w.pane_mask_at(p), 0, "isolated pane is a bare post");

        w.set_block_world(7, 64, 8, Block::Stone);
        w.set_block_world(9, 64, 8, Block::GlassPane);
        assert_eq!(
            w.pane_mask_at(p),
            petramond_world::pane::WEST | petramond_world::pane::EAST
        );

        w.set_block_world(8, 64, 7, Block::Chest);
        w.set_block_world(8, 64, 9, Block::Cactus);
        assert_eq!(
            w.pane_mask_at(p),
            petramond_world::pane::WEST | petramond_world::pane::EAST,
            "no_pane_connect blocks must not add arms"
        );
    }

    #[test]
    fn pane_connects_to_a_stair_back_but_not_its_open_side() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
        // Stair east of the pane, facing east: its flat high/back side faces the pane.
        assert!(w.place_stair(
            IVec3::new(9, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.pane_mask_at(p), petramond_world::pane::EAST);

        // Stair west of the pane, also facing east: its open side faces the pane.
        assert!(w.place_stair(
            IVec3::new(7, 64, 8),
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom),
        ));
        assert_eq!(w.pane_mask_at(p), petramond_world::pane::EAST);
    }

    #[test]
    fn pane_connects_to_a_full_slab_stack_but_not_a_single_slab() {
        let mut w = world();
        let p = IVec3::new(8, 64, 8);
        // The probe shape is REAL: masks are refined per-cell state now, so
        // the cell must hold the block whose state the cascade maintains.
        assert!(w.set_block_world(p.x, p.y, p.z, Block::GlassPane));
        let n = IVec3::new(8, 64, 7);
        let slot = |index| petramond_world::slab::SlabSlot {
            split: SlabSplit::Y,
            index,
        };
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(0)));
        assert_eq!(w.pane_mask_at(p), 0, "a single slab is not a full face");
        assert!(w.place_slab_layer(n, Block::OakSlab, slot(1)));
        assert_eq!(w.pane_mask_at(p), petramond_world::pane::NORTH);
    }
}

#[cfg(test)]
mod world_torch {
    use crate::world::World;
    #[allow(unused_imports)]
    use petramond_world::world::torch::test_exports::*;

    use petramond_math::facing::Facing;
    use petramond_world::block::Block;
    use petramond_world::block_state::{SlabSplit, StairHalf, StairState};
    use petramond_world::chunk::{Chunk, ChunkPos};

    fn world() -> World {
        let mut w = World::new(0, 4);
        w.insert_chunk_for_test(ChunkPos::new(0, 0), Chunk::new(0, 0));
        w
    }

    #[test]
    fn stair_flat_back_supports_a_wall_torch() {
        let mut w = world();
        let stair = IVec3::new(8, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));

        let torch = stair - IVec3::new(1, 0, 0);
        assert!(
            w.torch_supported_at(torch, TorchPlacement::West),
            "the full-height back face of a stair should hold a wall torch"
        );
    }

    #[test]
    fn single_slab_side_does_not_support_a_wall_torch() {
        let mut w = world();
        let slab = IVec3::new(8, 64, 8);
        assert!(w.place_slab_layer(
            slab,
            Block::DirtSlab,
            petramond_world::slab::SlabSlot {
                split: SlabSplit::Y,
                index: 0,
            }
        ));

        let torch = slab + IVec3::new(1, 0, 0);
        assert!(
            !w.torch_supported_at(torch, TorchPlacement::East),
            "a single slab side is not a complete wall face"
        );
    }

    #[test]
    fn stair_open_side_does_not_support_a_wall_torch() {
        let mut w = world();
        let stair = IVec3::new(8, 64, 8);
        assert!(w.place_stair(
            stair,
            Block::OakStairs,
            StairState::new(Facing::East, StairHalf::Bottom)
        ));

        let torch = stair + IVec3::new(1, 0, 0);
        assert!(
            !w.torch_supported_at(torch, TorchPlacement::East),
            "the open side of a stair is not a complete wall face"
        );
    }

    #[test]
    fn fence_post_top_supports_a_floor_torch_but_its_sides_hold_no_wall_torch() {
        let mut w = world();
        w.set_block_world(8, 64, 8, Block::OakFence);

        let floor_torch = IVec3::new(8, 65, 8);
        assert!(
            w.torch_supported_at(floor_torch, TorchPlacement::Floor),
            "a fence's post top should hold a floor torch"
        );

        let fence = IVec3::new(8, 64, 8);
        for (torch, placement) in [
            (fence + IVec3::new(1, 0, 0), TorchPlacement::East),
            (fence + IVec3::new(-1, 0, 0), TorchPlacement::West),
            (fence + IVec3::new(0, 0, 1), TorchPlacement::South),
            (fence + IVec3::new(0, 0, -1), TorchPlacement::North),
        ] {
            assert!(
                !w.torch_supported_at(torch, placement),
                "{placement:?} must not mount on a fence side"
            );
        }
    }

    #[test]
    fn full_slab_stacks_support_torches_like_full_blocks() {
        let mut w = world();
        let slab = IVec3::new(8, 64, 8);
        for (block, index) in [(Block::DirtSlab, 0), (Block::CobblestoneSlab, 1)] {
            assert!(w.place_slab_layer(
                slab,
                block,
                petramond_world::slab::SlabSlot {
                    split: SlabSplit::Y,
                    index,
                }
            ));
        }

        for (torch, placement) in [
            (slab + IVec3::new(0, 1, 0), TorchPlacement::Floor),
            (slab + IVec3::new(1, 0, 0), TorchPlacement::East),
            (slab + IVec3::new(-1, 0, 0), TorchPlacement::West),
            (slab + IVec3::new(0, 0, 1), TorchPlacement::South),
            (slab + IVec3::new(0, 0, -1), TorchPlacement::North),
        ] {
            assert!(
                w.torch_supported_at(torch, placement),
                "{placement:?} should be supported by a full slab stack"
            );
        }
    }
}
