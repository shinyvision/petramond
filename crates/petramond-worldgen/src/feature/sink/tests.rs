use petramond_world::block::{Block, CellView};
use petramond_world::block_state::StairState;
use petramond_world::facing::Facing;
use petramond_world::structure::Template;
use petramond_world::world::placement::authored::Turn;

use super::*;

#[test]
fn generated_state_and_metadata_clip_together_without_becoming_player_edits() {
    let template = Box::leak(Box::new(
        Template::parse(
            r#"{
        "size": [2, 1, 1],
        "palette": {"step": {"block": "step", "state": {"facing": "east"}}},
        "fills": [{"from": [0,0,0], "to": [1,0,0], "palette": "step"}],
        "markers": [{"pos": [1,0,0], "key": "fixture:once", "value": true}]
    }"#,
            |key| (key == "step").then_some(Block::OakStairs),
        )
        .unwrap(),
    ));
    let plan = crate::hooks::GenerationPlan {
        features: Vec::new(),
        blocks: Vec::new(),
        structures: vec![template
            .place(IVec3::new(-1, -17, 0), Turn::default())
            .unwrap()],
    };
    let mut a = Section::new(-1, -2, 0);
    let mut b = Section::new(0, -2, 0);
    apply_gen_plan(&mut b, &plan);
    apply_gen_plan(&mut a, &plan);
    for (section, x) in [(&a, 15), (&b, 0)] {
        assert_eq!(section.block(x, 15, 0), Block::OakStairs);
        assert_eq!(
            StairState::from_cell(section.cell_state(x, 15, 0)).facing,
            Facing::East
        );
        assert!(!section.modified);
    }
    assert!(a.cell_kv().is_empty());
    assert_eq!(
        b.cell_kv_get(0, 15, 0, "fixture:once"),
        Some(b"true".as_slice())
    );
    apply_gen_plan(
        &mut b,
        &crate::hooks::GenerationPlan {
            features: Vec::new(),
            blocks: vec![([0, -17, 0], Block::Stone.id())],
            structures: Vec::new(),
        },
    );
    assert!(b.cell_kv().is_empty());
    assert!(b.cell_state(0, 15, 0).bytes().is_empty());
}
