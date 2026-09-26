use super::*;
use crate::block::Block;
use crate::chunk::ChunkPos;
use crate::mathh::IVec3;
use crate::world::test_world::TestWorld;

/// The per-id collision table ([`Block::static_collision_boxes`]) is only
/// sound while every kind flagged [`ShapeKindDef::collision_state_free`]
/// really answers the same boxes with and without a cell to read. A family
/// that grows a per-cell `collision_boxes` override must stop answering
/// `ShapeSim::collision_state_free` — and this is what says so.
#[test]
fn collision_state_free_kinds_resolve_identically() {
    let mut world = TestWorld::new(0);
    world.insert_empty_column(ChunkPos::new(0, 0));
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
    // Empty lists, inverted extents, a box flat on two axes, texels past
    // the overhang room, unknown face names and a tile on an undrawn face
    // are load errors, not silently dropped values. (Flat on ONE axis is
    // a plane, and a box may reach one cell past its own.)
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
            IVec3::ZERO,
            Block::Air,
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
    assert!(set.corner_joins());
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
        families::box_set_box(b, 0, Block::Stone, &|_| [1.0; 3]).faces
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
