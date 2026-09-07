use super::*;
use petramond_world::texture_transition::Rules;

fn rules() -> Rules {
    Rules::from_layers(&[r#"{
        "sets": [
            {"set": "test:organic", "mask": "organic_transition_mask", "width_texels": 6},
            {"set": "test:hard", "mask": "organic_transition_mask", "width_texels": 2}
        ],
        "pairs": [
            {"pair": "test:dirt_grass", "set": "test:organic", "blocks": ["petramond:dirt", "petramond:grass"]},
            {"pair": "test:dirt_stone", "set": "test:hard", "blocks": ["petramond:dirt", "petramond:stone"]}
        ]}"#])
    .unwrap()
}

fn set_id(rules: &Rules, name: &str) -> u8 {
    rules.sets.iter().position(|s| s.name == name).unwrap() as u8
}

fn plan_with(
    rules: &Rules,
    blocks: &dyn Fn(i32, i32, i32) -> Block,
    known: &dyn Fn(i32, i32, i32) -> bool,
    blocked: &dyn Fn(i32, i32, i32) -> bool,
    pos: IVec3,
    face: Face,
) -> Option<Transition> {
    let ctx = Context {
        rules,
        block: blocks,
        known,
        blocked,
        covered: &|p, _| blocks(p.x, p.y, p.z).is_opaque(),
    };
    ctx.plan(pos, face, blocks(pos.x, pos.y, pos.z).id())
}

#[test]
fn donor_coordinates_follow_every_face_orientation_across_section_seams() {
    let rules = rules();
    let organic = set_id(&rules, "test:organic");
    let source = rules.local(organic, Block::Dirt.id());
    let donor = rules.local(organic, Block::Grass.id());
    for face in Face::ALL {
        for pos in [IVec3::ZERO, IVec3::splat(-16), IVec3::splat(15)] {
            let (u, v) = axes(face);
            let blocks = |x, y, z| {
                let p = IVec3::new(x, y, z);
                if p == pos {
                    Block::Dirt
                } else if p == pos + u || p == pos + v || p == pos + u + v {
                    Block::Grass
                } else {
                    Block::Air
                }
            };
            let plan = |at| {
                plan_with(&rules, &blocks, &|_, _, _| true, &|_, _, _| false, at, face).unwrap()
            };
            let forward = plan(pos);
            assert_eq!(forward.set, organic);
            assert_eq!(forward.grid[0], source);
            assert_eq!(forward.grid[5], donor);
            assert_eq!(forward.grid[7], donor);
            assert_eq!(forward.grid[8], donor);
            assert_eq!(plan(pos + u).grid[4], source);
        }
    }
}

#[test]
fn excluded_covered_unknown_and_dyed_donors_do_not_bleed() {
    let rules = rules();
    for scenario in 0..5 {
        let blocks = |x, y, z| match (x, y, z) {
            (0, 0, 0) => Block::Dirt,
            (1, 0, 0) => {
                if scenario == 0 {
                    Block::OakLog
                } else {
                    Block::Grass
                }
            }
            (1, 1, 0) if scenario == 1 => Block::Stone,
            _ => Block::Air,
        };
        let known = |x, _, _| scenario != 2 || x != 1;
        let blocked = |x, _, _| (scenario == 3 && x == 1) || (scenario == 4 && x == 0);
        assert!(
            plan_with(&rules, &blocks, &known, &blocked, IVec3::ZERO, Face::PosY).is_none(),
            "scenario {scenario}"
        );
    }
}

#[test]
fn diagonal_does_not_jump_across_missing_cardinals() {
    let rules = rules();
    let blocks = |x, y, z| match (x, y, z) {
        (0, 0, 0) => Block::Dirt,
        (1, 0, 1) => Block::Grass,
        _ => Block::Air,
    };
    assert!(plan_with(
        &rules,
        &blocks,
        &|_, _, _| true,
        &|_, _, _| false,
        IVec3::ZERO,
        Face::PosY
    )
    .is_none());
}

#[test]
fn a_face_renders_in_the_first_set_that_offers_a_donor() {
    let rules = rules();
    let (hard, organic) = (set_id(&rules, "test:hard"), set_id(&rules, "test:organic"));
    assert!(hard < organic, "sets order by name");
    // Dirt is in both sets; stone only pairs with it in the hard set.
    let blocks = |x, y, z| match (x, y, z) {
        (0, 0, 0) => Block::Dirt,
        (1, 0, 0) => Block::Stone,
        (-1, 0, 0) => Block::Grass,
        _ => Block::Air,
    };
    let stone_only = |x, y, z| {
        if (x, y, z) == (-1, 0, 0) {
            Block::Air
        } else {
            blocks(x, y, z)
        }
    };
    let hard_plan = plan_with(
        &rules,
        &stone_only,
        &|_, _, _| true,
        &|_, _, _| false,
        IVec3::ZERO,
        Face::PosY,
    )
    .unwrap();
    assert_eq!(hard_plan.set, hard);
    assert_eq!(hard_plan.grid[0], rules.local(hard, Block::Dirt.id()));
    assert_eq!(hard_plan.grid[5], rules.local(hard, Block::Stone.id()));
    let both = plan_with(
        &rules,
        &blocks,
        &|_, _, _| true,
        &|_, _, _| false,
        IVec3::ZERO,
        Face::PosY,
    )
    .unwrap();
    assert_eq!(both.set, hard, "sets are tried in name order");
    assert_eq!(both.grid[5], rules.local(hard, Block::Stone.id()));
    assert_eq!(
        both.grid[4], 0,
        "a material outside the chosen set is no donor"
    );
    let grass = |x, y, z| {
        if y == 0 && z == 0 && (0..=1).contains(&x) {
            Block::Grass
        } else {
            Block::Air
        }
    };
    let same = plan_with(
        &rules,
        &grass,
        &|_, _, _| true,
        &|_, _, _| false,
        IVec3::ZERO,
        Face::PosY,
    );
    assert!(
        same.is_none(),
        "same-material neighbours alone plan nothing"
    );
}
