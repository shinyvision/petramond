use super::*;
use crate::feature::FeatureField;
use petramond_world::biome::Biome;

struct Forest;
impl FeatureField for Forest {
    fn column_at(&mut self, _: i32, _: i32) -> (i32, Biome) {
        (70, Biome::Forest)
    }
}

#[test]
fn replay_matches_direct_generation_in_any_section_order_and_on_occupied_cells() {
    let plan = FeaturePlan::record(-1, 2, |ctx| {
        super::super::place_trees(ctx, &mut Forest, 786, -16, 32)
    });
    for cy in [6, 4, 8, 5, 7] {
        let mut direct = Section::new(-1, cy, 2);
        for y in 0..16 {
            for z in 0..16 {
                for x in 0..16 {
                    if (x + y + z) % 11 == 0 {
                        direct.set_block_raw(x, y, z, Block::Stone.id());
                    }
                }
            }
        }
        let mut replay = direct.clone();
        super::super::tree_select::place_features_section(&mut direct, &mut Forest, 786);
        plan.apply(&mut replay);
        assert_eq!(
            direct.blocks_iter().collect::<Vec<_>>(),
            replay.blocks_iter().collect::<Vec<_>>()
        );
    }
}

#[test]
fn replay_keeps_predicates_and_write_order() {
    let pos = IVec3::new(1, 65, 1);
    let mut recorder = Recorder {
        ox: 0,
        oz: 0,
        sections: BTreeMap::new(),
    };
    let mut ctx = FeatureCtx::new(&mut recorder);
    ctx.set_leaf(pos, Block::Stone);
    ctx.set_branch(pos, Block::Dirt);
    ctx.replace_block(pos, Block::Stone, Block::Dirt);
    let plan = FeaturePlan {
        sections: recorder.sections,
    };
    let mut air = Section::new(0, 4, 0);
    plan.apply(&mut air);
    assert_eq!(air.block(1, 1, 1), Block::Dirt);
    let mut occupied = Section::new(0, 4, 0);
    occupied.set_block_raw(1, 1, 1, Block::Sand.id());
    plan.apply(&mut occupied);
    assert_eq!(occupied.block(1, 1, 1), Block::Sand);
}
