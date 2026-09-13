use super::*;

struct Branch;

impl Feature for Branch {
    fn generate(
        &self,
        ctx: &mut FeatureCtx,
        _: &mut dyn FnMut(IVec3) -> bool,
        origin: IVec3,
        _: &mut FeatureRng,
    ) {
        ctx.set_log(origin, Block::OakLog);
        ctx.set_log(origin + IVec3::Y, Block::OakLog);
        ctx.set_branch(origin + IVec3::Y + IVec3::X, Block::OakLog);
        ctx.set_leaf(origin + IVec3::Y + IVec3::X, Block::OakLeaves);
        ctx.set_leaf(origin + IVec3::Y + IVec3::X * 2, Block::OakLeaves);
    }
}

#[test]
fn whole_geometry_admission_precedes_section_clipping() {
    let origin = IVec3::new(-1, -17, 0);
    let make = |obstacle| {
        PlacedFeature::record(&Branch, origin, FeatureRng::from_state(1), |probes| {
            probes
                .iter()
                .map(|p| {
                    if p[1] < origin.y {
                        TerrainSpace::Solid
                    } else if p[0] == 1 {
                        obstacle
                    } else {
                        TerrainSpace::Air
                    }
                })
                .collect()
        })
        .unwrap()
    };
    let accepted = make(TerrainSpace::Air);
    let mut a = Section::new(-1, -2, 0);
    let mut b = Section::new(0, -1, 0);
    accepted.apply(&mut b);
    accepted.apply(&mut a);
    assert_eq!(a.block(15, 15, 0), Block::OakLog);
    assert_eq!(b.block(0, 0, 0), Block::OakLog);
    assert_eq!(b.block(1, 0, 0), Block::OakLeaves);
    for obstacle in [TerrainSpace::Solid, TerrainSpace::Fluid] {
        assert!(make(obstacle).cells.is_empty());
    }
}

#[test]
fn unsupported_roots_reject_the_whole_feature() {
    let plan = PlacedFeature::record(&Branch, IVec3::ZERO, FeatureRng::from_state(1), |probes| {
        vec![TerrainSpace::Air; probes.len()]
    })
    .unwrap();
    assert!(plan.cells.is_empty());
}

#[test]
fn first_complete_candidate_is_selected_before_any_section_is_applied() {
    let mut attempts = Vec::new();
    let selected =
        PlacedFeature::first_admitted(&[[-1, -17, 0], [31, -17, 0], [99, -17, 0]], |origin| {
            attempts.push(origin);
            PlacedFeature::record(
                &Branch,
                origin.into(),
                FeatureRng::from_state(1),
                |probes| {
                    probes
                        .iter()
                        .map(|p| {
                            if p[1] < -17 || p[0] == 0 {
                                TerrainSpace::Solid
                            } else {
                                TerrainSpace::Air
                            }
                        })
                        .collect()
                },
            )
            .map(Arc::new)
        })
        .unwrap();
    assert_eq!(attempts, [[-1, -17, 0], [31, -17, 0]]);
    let mut rejected = Section::new(-1, -2, 0);
    let mut root = Section::new(1, -2, 0);
    let mut crown = Section::new(2, -1, 0);
    selected.apply(&mut crown);
    selected.apply(&mut rejected);
    selected.apply(&mut root);
    assert_eq!(rejected.block(15, 15, 0), Block::Air);
    assert_eq!(root.block(15, 15, 0), Block::OakLog);
    assert_eq!(crown.block(0, 0, 0), Block::OakLog);
}
