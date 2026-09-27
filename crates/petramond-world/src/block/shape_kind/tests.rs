use super::*;
use crate::block::Block;
use crate::chunk::ChunkPos;
use crate::mathh::IVec3;
use crate::world::test_world::TestWorld;

#[test]
fn collision_state_free_kinds_resolve_identically() {
    let mut world = TestWorld::new(0);
    world.insert_empty_column(ChunkPos::new(0, 0));
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
            IVec3::new(8, 65, 8),
            IVec3::new(8, 66, 8),
            IVec3::new(3, 70, 12),
        ] {
            let live = k.sim.collision_boxes(&k.params, &world.data, pos, block);
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

#[test]
fn raw_shape_deserializes_families_params_and_named_references() {
    let de = |s: &str| serde_json::from_str::<RawShape>(s).expect("parses");
    assert!(matches!(de(r#""cube""#), RawShape::Cube));
    assert!(matches!(de(r#""fence""#), RawShape::Fence));
    assert!(matches!(de(r#""door""#), RawShape::Door));
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
    assert_eq!(
        set.boxes(0, 0)[1].faces,
        [false, false, true, false, false, false]
    );
    assert_eq!(set.collision(0, 0).len(), 1);
    assert_eq!(set.bounds(0, 0).max, [1.0, 1.0, 1.0]);
    for bad in [
        r#"{"boxes":[]}"#,
        r#"{"boxes":[{"from":[9,0,0],"to":[8,16,16]}]}"#,
        r#"{"boxes":[{"from":[8,0,8],"to":[8,16,8]}]}"#,
        r#"{"boxes":[{"to":[33,16,16]}]}"#,
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
    assert!(serde_json::from_str::<RawShape>(r#""bogus""#).is_err());
}

/// Apertures ask whether matter SEALS a boundary, not whether it fills the
/// octant behind it. A cover that stops a texel short of its top must read
/// OPEN above — otherwise its own cell floods to black, the sunken top the
/// mesher draws inside that cell samples the dark, and every neighbouring
/// face averages its smooth light against a cell that is plainly lit.
#[test]
fn a_cover_that_stops_short_of_its_top_stays_open_to_the_light() {
    let apertures = |json: &str| {
        let (family, params, _) = resolve_json(json).unwrap();
        let (sim, ..) = families::singletons(family);
        sim.light_apertures(&params, &facets::NoNeighborhood, IVec3::ZERO, Block::Air)
    };
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

fn resolve_corners(s: &str) -> Result<(ShapeFamily, ShapeParams, String), String> {
    serde_json::from_str::<RawShape>(s)
        .expect("parses")
        .resolve(true)
}

#[test]
fn four_quarter_turns_return_a_box_set_to_its_authored_form() {
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
    for t in 1..4 {
        assert_ne!(set.boxes(0, 0), set.boxes(t, 0), "turn {t} must differ");
    }
    assert!(set.boxes(0, 0)[0].faces[5] && set.boxes(1, 0)[0].faces[FRONT_AFTER_TURN[1]]);
    assert_eq!(
        set.boxes(0, 0)[0].tiles[5],
        set.boxes(1, 0)[0].tiles[FRONT_AFTER_TURN[1]],
        "the front TILE travels with the front face"
    );
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
/// untouched — corner joining must never change a shape's resting look.
#[test]
fn corner_forms_are_the_turned_intersection_and_union_of_the_shape() {
    let (_, params, key) = resolve_corners(
        r#"{"boxes":[
             {"from":[0,14,0],"to":[16,16,16]},
             {"from":[0,0,2],"to":[16,14,16]}
           ]}"#,
    )
    .unwrap();
    let set = params.box_set().expect("box set params");
    assert!(set.corner_joins());
    assert!(key.ends_with("+corners"), "the flag is kind identity");
    let t = |v: i32| v as f32 / 16.0;
    let straight = set.boxes(0, 0);
    assert_eq!(straight.len(), 2);
    assert_eq!(straight[1].aabb.min, [0.0, 0.0, t(2)]);
    assert_eq!(straight[1].aabb.max, [1.0, t(14), 1.0]);
    let outer = set.boxes(0, 1);
    let body: Vec<_> = outer.iter().filter(|b| b.aabb.max[1] < 1.0).collect();
    assert_eq!(body.len(), 1);
    assert_eq!(body[0].aabb.min, [0.0, 0.0, t(2)]);
    assert_eq!(body[0].aabb.max, [t(14), t(14), 1.0]);
    assert!(draws_front(body[0], 5, 0), "authored front still front");
    assert!(
        draws_front(body[0], 0, 0),
        "wrapped +X face draws front art"
    );
    assert!(!draws_front(body[0], 1, 0), "back-side face stays side art");
    let furnace = Block::Furnace;
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
    let inner = set.boxes(0, 3);
    assert_eq!(inner.len(), 3, "top + both bodies");
    assert_eq!(inner[1].aabb.max, [1.0, t(14), 1.0]);
    assert_eq!(inner[2].aabb.max, [t(14), t(14), 1.0]);
    for form in 0..5u8 {
        let expect: Vec<_> = set.boxes(0, form).iter().map(|b| b.turned()).collect();
        assert_eq!(set.boxes(1, form), &expect[..], "form {form} turns whole");
    }
    assert_eq!(set.collision(0, 1).len(), 2);
    assert_eq!(set.bounds(0, 1).max, [1.0; 3]);
    assert_eq!(set.boxes(0, 9), set.boxes(0, 0));
    let (_, plain, _) = resolve_json(r#"{"boxes":[{"to":[16,15,16]}]}"#).unwrap();
    let plain = plain.box_set().unwrap();
    assert!(!plain.corner_joins());
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
    let (_, params, _) = resolve_corners(
        r#"{"boxes":[
             {"from":[0,0,0],"to":[16,6,16],"tiles":{"up":"stone"}},
             {"from":[0,0,0],"to":[16,16,8]}
           ]}"#,
    )
    .unwrap();
    let set = params.box_set().expect("box set params");
    let t = |v: i32| v as f32 / 16.0;
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
    let own = outer
        .iter()
        .find(|b| b.aabb.max == [1.0, t(6), 1.0])
        .expect("shelf ∩ turned shelf");
    assert_eq!(own.art_turns, [0; 6]);
    // What actually matters is the DRAW: two tops of the same form, same
    // tile, in the same cell, must be counter-rotated DIFFERENTLY because
    // they were authored in different frames. Reading the cell's turn
    // alone gives both `0`, so the test checks their separate frame rotations.
    let drawn_top = |b: &BoxDef| {
        families::box_set_box(b, 0, Block::Stone, &|_| [1.0; 3]).faces[2]
            .expect("a top face")
            .uv_turns
    };
    assert_eq!(drawn_top(piece), 1, "inherited top turns with its parent");
    assert_eq!(drawn_top(own), 0, "the shape's own top does not");
    let turned = set.boxes(1, 1);
    let moved = turned
        .iter()
        .find(|b| b.aabb.min == [t(8), 0.0, 0.0] && b.aabb.max == [1.0, t(6), 1.0])
        .expect("the same piece, one turn on");
    assert_eq!(moved.art_turns[2], 1);
    assert_eq!(moved.tiles[2], Tile::from_name("stone"));
}

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

    let (fam, params, _) =
        resolve_json(r#"{"custom":{"family":"wall_panel","thickness":4,"height":12}}"#).unwrap();
    assert_eq!(fam, ShapeFamily::Ladder);
    let d = params.dimensions().unwrap();
    assert_eq!((d.thickness, d.height), (4.0 / 16.0, 12.0 / 16.0));

    let (_, params, _) = resolve_json(r#"{"custom":{"family":"crop"}}"#).unwrap();
    let d = params.dimensions().unwrap();
    assert_eq!((d.inset, d.drop), (2.0 / 16.0, 1.0 / 16.0));
}

#[test]
fn custom_dimension_families_validate() {
    assert!(resolve_json(r#"{"custom":{"family":"cross","inset":8}}"#).is_err());
    assert!(resolve_json(r#"{"custom":{"family":"crop","inset":20}}"#).is_err());
    assert!(resolve_json(r#"{"custom":{"family":"wall_panel","thickness":0}}"#).is_err());
    assert!(resolve_json(r#"{"custom":{"family":"cross","plane_count":3}}"#).is_err());
    assert!(resolve_json(r#"{"custom":{"family":"pyramid"}}"#).is_err());
    assert!(resolve_json(r#"{"custom":{"family":"crop","post_thickness":4}}"#).is_err());
}
