use super::*;
use petramond_math::world_pos::WorldPos;
use petramond_world::world::raycast;

#[test]
fn raycast_target_selection_cases() {
    struct Case {
        label: &'static str,
        eye: petramond_math::world_pos::WorldPos,
        dir: Vec3,
        blocks: fn(i32, i32, i32) -> Block,
        precise: bool,
        expect: Option<(IVec3, IVec3)>,
    }
    let cases = [
        Case {
            label: "solid block ahead hits with the face-toward-eye normal",
            eye: WorldPos::new(0.5, 64.5, 0.5),
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
            eye: WorldPos::new(0.5, 64.5, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |x, _, _| if x == 100 { Block::Stone } else { Block::Air },
            precise: false,
            expect: None,
        },
        Case {
            label: "eye inside solid hits its own cell with a zero normal",
            eye: WorldPos::new(0.5, 64.5, 0.5),
            dir: Vec3::new(1.0, 0.0, 0.0),
            blocks: |_, _, _| Block::Stone,
            precise: false,
            expect: Some((IVec3::new(0, 64, 0), IVec3::ZERO)),
        },
        Case {
            label: "precise shape reports the shape surface normal",
            eye: WorldPos::new(1.9, 64.75, 0.5),
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
            label: "ray over a short plant box misses it",
            eye: WorldPos::new(0.5, 64.95, 0.5),
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
            eye: WorldPos::new(0.5, 64.95, 0.5),
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
            raycast::blocks_core(case.eye, dir, REACH, &case.blocks, &|e, d, _, block| {
                if block != Block::DirtSlab {
                    return None;
                }
                raycast::ray_vs_aabb_hit(e, d, Vec3::ZERO, Vec3::new(1.0, 0.5, 1.0))
            })
        } else {
            raycast::blocks_core(case.eye, dir, REACH, &case.blocks, &|_, _, _, _| None)
        };
        let got = result.map(|(hit, _)| (hit.block, hit.normal));
        assert_eq!(
            got, case.expect,
            "[{}] (block, normal) of the raycast result",
            case.label
        );
    }
}

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
    let precise = |e, d, _: IVec3, block: Block| {
        let (mn, mx) = block.visual_aabb()?;
        raycast::ray_vs_aabb_hit(e, d, Vec3::from(mn), Vec3::from(mx))
    };

    let (hit, _) = raycast::blocks_core(
        WorldPos::new(0.5, 64.03, 0.5),
        Vec3::new(1.0, 0.0, 0.0),
        REACH,
        &blocks,
        &precise,
    )
    .expect("the cover is selectable");
    assert_eq!(hit.block, IVec3::new(2, 64, 0));

    assert!(
        raycast::blocks_core(
            WorldPos::new(0.5, 64.5, 0.5),
            Vec3::new(1.0, 0.0, 0.0),
            REACH,
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
    let eye = WorldPos::new(0.5, 64.25, 0.5);
    let (hit, _) = raycast::blocks_core(
        eye,
        Vec3::new(1.0, 0.0, 0.0),
        REACH,
        &blocks,
        &|_, _, _, _| None,
    )
    .unwrap();
    assert_eq!(hit.block, IVec3::new(2, 64, 0));
    assert_eq!(hit.normal, IVec3::new(-1, 0, 0));
    let SelectionShape::Box { origin, min, max } = hit.outline else {
        panic!("plant outlines are square, got {:?}", hit.outline);
    };
    assert_eq!(origin, IVec3::new(2, 64, 0));
    assert!(max.y < 1.0, "trims to the sprite's height, got {}", max.y);
    assert!(
        min.x > 0.0 && max.x < 1.0,
        "pulls in from the cell walls, got {}..{}",
        min.x,
        max.x
    );
}

#[test]
fn intersects_block_consistent_with_sweep_when_flush() {
    let pl = p(WorldPos::new(1.3, 64.0, 0.5));
    assert!(
        pl.aabb_min()[0] < 1.0,
        "precondition: float pulls min.x below 1.0"
    );
    assert!(
        !pl.intersects_block(IVec3::new(0, 64, 0)),
        "flush-beside cell must read as free, matching sweep"
    );
    assert!(pl.intersects_block(IVec3::new(1, 64, 0)));
}

#[test]
fn intersects_block_strict_faces() {
    let pl = p(WorldPos::new(0.5, 64.0, 0.5));
    assert!(pl.intersects_block(IVec3::new(0, 64, 0)));
    assert!(!pl.intersects_block(IVec3::new(1, 64, 0)));
    assert!(pl.intersects_block(IVec3::new(0, 65, 0)));
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
    use petramond_world::block::Block;
    use petramond_world::chunk::{Chunk, ChunkPos};

    let mut world = crate::world::ServerWorld::new(1, 2);
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
        let eye = WorldPos::new(
            base.x as f64 - 2.0,
            cell.y as f64 + 0.5,
            cell.z as f64 + 0.5,
        );
        let Some((hit, _)) = raycast::with_dist(eye, Vec3::new(1.0, 0.0, 0.0), world.data()) else {
            continue;
        };
        let SelectionShape::Box { min, max, .. } = hit.outline else {
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
