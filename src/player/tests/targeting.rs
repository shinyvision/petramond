use super::*;

/// DDA target selection: full cubes stop the ray on cell entry with the
/// entered face's normal (zero when the eye starts inside one), precise
/// shapes report their own surface normal, and a plant's trimmed selection
/// box is shorter than its cell. The plant OUTLINE shape is pinned separately
/// by `raycast_hits_a_plants_selection_box_without_pixel_precision`.
#[test]
fn raycast_target_selection_cases() {
    struct Case {
        label: &'static str,
        eye: Vec3,
        /// Normalized before the cast.
        dir: Vec3,
        blocks: fn(i32, i32, i32) -> Block,
        /// Probe precise shapes (the half-height slab AABB) instead of only
        /// full-cube cell entry.
        precise: bool,
        /// `Some((hit block, hit normal))` or a miss.
        expect: Option<(IVec3, IVec3)>,
    }
    let cases = [
        Case {
            // Single solid block at (4, 64, 0): entered at x=4.0, i.e. 3.5
            // from the eye — within REACH (4.0). (A block at x=5 would be 4.5
            // away → a miss.)
            label: "solid block ahead hits with the face-toward-eye normal",
            eye: Vec3::new(0.5, 64.5, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |x, y, z| {
                if (x, y, z) == (4, 64, 0) {
                    Block::Stone
                } else {
                    Block::Air
                }
            },
            precise: false,
            expect: Some((IVec3::new(4, 64, 0), IVec3::new(-1, 0, 0))),
        },
        Case {
            label: "solid block out of reach misses",
            eye: Vec3::new(0.5, 64.5, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |x, _, _| if x == 100 { Block::Stone } else { Block::Air },
            precise: false,
            expect: None,
        },
        Case {
            label: "eye inside solid hits its own cell with a zero normal",
            eye: Vec3::new(0.5, 64.5, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |_, _, _| Block::Stone,
            precise: false,
            expect: Some((IVec3::new(0, 64, 0), IVec3::ZERO)),
        },
        Case {
            // Placement should use the slab top face, not the full voxel side.
            label: "precise shape reports the shape surface normal",
            eye: Vec3::new(1.9, 64.75, 0.5),
            dir: Vec3::new(1.0, -0.5, 0.0),
            blocks: |x, y, z| {
                if (x, y, z) == (2, 64, 0) {
                    Block::DirtSlab
                } else {
                    Block::Air
                }
            },
            precise: true,
            expect: Some((IVec3::new(2, 64, 0), IVec3::Y)),
        },
        Case {
            // The plant's selection box is trimmed to the sprite: a ray near
            // the cell's top passes clean over it.
            label: "ray over a short plant box misses it",
            eye: Vec3::new(0.5, 64.95, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |x, y, z| {
                if (x, y, z) == (2, 64, 0) {
                    Block::Poppy
                } else {
                    Block::Air
                }
            },
            precise: false,
            expect: None,
        },
        Case {
            label: "ray over a short plant box hits the block behind",
            eye: Vec3::new(0.5, 64.95, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |x, y, z| match (x, y, z) {
                (2, 64, 0) => Block::Poppy,
                (3, 64, 0) => Block::Stone,
                _ => Block::Air,
            },
            precise: false,
            expect: Some((IVec3::new(3, 64, 0), IVec3::new(-1, 0, 0))),
        },
    ];

    for case in cases {
        let dir = case.dir.normalize();
        let result = if case.precise {
            Player::raycast_blocks_core(case.eye, dir, &case.blocks, &|e, d, pos, block| {
                if block != Block::DirtSlab {
                    return None;
                }
                let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
                interaction::ray_vs_aabb_hit(e, d, base, base + Vec3::new(1.0, 0.5, 1.0))
            })
        } else {
            Player::raycast_blocks_core(case.eye, dir, &case.blocks, &|_, _, _, _| None)
        };
        let got = result.map(|(hit, _)| (hit.block, hit.normal));
        assert_eq!(
            got, case.expect,
            "[{}] (block, normal) of the raycast result",
            case.label
        );
    }
}

/// A walk-through cover (a lowered cube whose shape declares no collision) is
/// selectable by its VISIBLE box, not by its collision: aiming at the cover
/// hits it, and a ray passing above the box misses. The DDA reaches it because
/// the shape answers with a box at all, not because the engine names its
/// family.
#[test]
fn raycast_targets_a_walk_through_cover_by_its_visible_box() {
    let blocks = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 64, 0) {
            Block::SnowLayer
        } else {
            Block::Air
        }
    };
    assert!(
        Block::SnowLayer.collision_boxes().is_empty(),
        "the fixture only means anything while the cover is walk-through"
    );
    // What `precise_shape_hit` does for a shape that is not a box SET: test
    // the family's own visible box.
    let precise = |e, d, pos: IVec3, block: Block| {
        let (mn, mx) = block.visual_aabb()?;
        let base = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        interaction::ray_vs_aabb_hit(e, d, base + Vec3::from(mn), base + Vec3::from(mx))
    };

    // Just above the floor, inside the 1/16 cover: a hit on the cover's cell.
    let (hit, _) = Player::raycast_blocks_core(
        Vec3::new(0.5, 64.03, 0.5),
        Vec3::new(1.0, 0.0, 0.0),
        &blocks,
        &precise,
    )
    .expect("the cover is selectable");
    assert_eq!(hit.block, IVec3::new(2, 64, 0));

    // Above the cover's box: the ray passes clean over it.
    assert!(
        Player::raycast_blocks_core(
            Vec3::new(0.5, 64.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            &blocks,
            &precise,
        )
        .is_none(),
        "a ray above a thin cover must not select it"
    );
}

#[test]
fn raycast_hits_a_plants_selection_box_without_pixel_precision() {
    let blocks = |x: i32, y: i32, z: i32| {
        if (x, y, z) == (2, 64, 0) {
            Block::Poppy
        } else {
            Block::Air
        }
    };
    // z = 0.5 crosses the cell centre where the sparse poppy art may well be
    // transparent — a BOX hitbox must select it anyway.
    let eye = Vec3::new(0.5, 64.25, 0.5);
    let (hit, _) =
        Player::raycast_blocks_core(eye, Vec3::new(1.0, 0.0, 0.0), &blocks, &|_, _, _, _| None)
            .unwrap();
    assert_eq!(hit.block, IVec3::new(2, 64, 0));
    assert_eq!(hit.normal, IVec3::new(-1, 0, 0));
    // The outline is the SAME square box the ray hit, trimmed to the art —
    // shorter than the cell (a ray can pass above it) and pulled in from the
    // cell walls.
    let SelectionShape::Box { min, max } = hit.outline else {
        panic!("plant outlines are square, got {:?}", hit.outline);
    };
    assert!(max.y < 65.0, "trims to the sprite's height, got {}", max.y);
    assert!(
        min.x > 2.0 && max.x < 3.0,
        "pulls in from the cell walls, got {}..{}",
        min.x,
        max.x
    );
}

#[test]
fn intersects_block_consistent_with_sweep_when_flush() {
    // Standing flush against a wall on the -X side: a -X resolve leaves the
    // min edge on the integer boundary, which float renders as 0.99999994.
    // `sweep` (lo = floor(min+EPS)) treats the cell beside you as free; the
    // place-gate must agree, or you can't build into a cell you clearly fit
    // next to.
    let pl = p(Vec3::new(1.3, 64.0, 0.5)); // min.x = 1.3 - 0.3 = 0.99999994
    assert!(
        pl.aabb_min().x < 1.0,
        "precondition: float pulls min.x below 1.0"
    );
    assert!(
        !pl.intersects_block(IVec3::new(0, 64, 0)),
        "flush-beside cell must read as free, matching sweep"
    );
    // And the cell the body actually stands in still counts.
    assert!(pl.intersects_block(IVec3::new(1, 64, 0)));
}

#[test]
fn intersects_block_strict_faces() {
    let pl = p(Vec3::new(0.5, 64.0, 0.5));
    // The cell the feet stand in overlaps.
    assert!(pl.intersects_block(IVec3::new(0, 64, 0)));
    // A block flush against +x face (player max.x = 0.8 < 1.0) does not.
    assert!(!pl.intersects_block(IVec3::new(1, 64, 0)));
    // A block at head height overlaps (player spans y in [64, 65.8]).
    assert!(pl.intersects_block(IVec3::new(0, 65, 0)));
    // Above the head does not.
    assert!(!pl.intersects_block(IVec3::new(0, 66, 0)));
}

/// A bbmodel block outlines its WHOLE model, from ANY of its cells.
///
/// The selection chain is one `else if` ladder and the model arm sits in the
/// middle of it: split that ladder and the per-CELL box from the generic arm
/// below silently overwrites the whole-model box, so a 2x2 workbench outlines
/// a quarter of itself and looks like a bug in the model. Nothing else in the
/// chain notices, which is why this is pinned here.
#[test]
fn a_multi_cell_model_block_outlines_its_whole_model_from_every_cell() {
    use crate::block::Block;
    use crate::chunk::{Chunk, ChunkPos};

    let mut world = crate::world::World::new(1, 2);
    for (cx, cz) in [(0, 0), (-1, 0), (0, -1), (-1, -1)] {
        world.insert_chunk_for_test(ChunkPos::new(cx, cz), Chunk::new(cx, cz));
    }
    let base = IVec3::new(4, 64, 4);
    assert!(
        world.place_model_block(base, Block::FurnitureWorkbench),
        "fixture: the workbench places"
    );
    let (_, _, cells) = world.model_group(base).expect("fixture: a placed group");
    assert!(cells.len() > 1, "fixture: this row is multi-cell");

    let mut checked = 0;
    for cell in cells {
        // Look along +X at the cell's own centre from outside the model, so
        // the DDA lands on THAT cell rather than a neighbour.
        let eye = Vec3::new(
            base.x as f32 - 2.0,
            cell.y as f32 + 0.5,
            cell.z as f32 + 0.5,
        );
        let Some((hit, _)) = Player::raycast_with_dist(eye, Vec3::new(1.0, 0.0, 0.0), &world)
        else {
            continue;
        };
        let SelectionShape::Box { min, max } = hit.outline else {
            panic!("a model block outlines as one box, got {:?}", hit.outline);
        };
        let span = max - min;
        assert!(
            span.x > 1.0 || span.y > 1.0 || span.z > 1.0,
            "cell {:?} outlined one cell ({min:?}..{max:?}) instead of the model",
            hit.block
        );
        checked += 1;
    }
    assert!(checked > 0, "fixture: at least one cell must be reachable");
}
