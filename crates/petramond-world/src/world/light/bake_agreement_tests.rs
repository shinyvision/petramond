use rustc_hash::FxHashMap;
use std::sync::Arc;

use crate::block::Block;
use crate::block_state::{StairHalf, StairState};
use crate::chunk::{section_idx, ChunkPos, SectionPos};
use crate::column::Column;
use crate::facing::Facing;
use crate::section::Section;

use super::batch::{run_light_bake_batch, snapshot_batch, GROUP};
use super::test_fixture::{report_first_diff, Fixture, Rng};

#[test]
fn batched_bake_matches_per_section_bakes() {
    let base = SectionPos::new(0, 0, 0);
    let low = SectionPos::new(-1, -1, -1);
    let mut rng = Rng(0xdead_beef_cafe_1234);

    for round in 0..4 {
        let fixture = Fixture::random(&mut rng, low, super::batch::SPAN, 9);
        let member_positions: Vec<SectionPos> = (0..GROUP)
            .flat_map(|my| {
                (0..GROUP).flat_map(move |mz| {
                    (0..GROUP)
                        .map(move |mx| SectionPos::new(base.cx + mx, base.cy + my, base.cz + mz))
                })
            })
            .filter(|p| fixture.sections.contains_key(p))
            .collect();
        assert!(
            !member_positions.is_empty(),
            "fixture produced an empty group in round {round}"
        );

        let job = snapshot_batch(base, &member_positions, &fixture.sections, &fixture.columns)
            .expect("batch snapshot");
        let batched = run_light_bake_batch(job);
        assert_eq!(batched.len(), member_positions.len());

        let label = format!("round {round}");
        for out in batched {
            let want = fixture.full_bake(out.pos);
            report_first_diff(&label, "skylight", out.pos, &out.skylight, &want.skylight);
            report_first_diff(
                &label,
                "block light",
                out.pos,
                &out.blocklight,
                &want.blocklight,
            );
        }
    }
}

#[test]
fn a_custom_aperture_gates_light_identically_in_both_bakes() {
    let pos = SectionPos::new(0, 0, 0);
    let (y, z) = (8usize, 8usize);
    let stair = (6usize, y, z);
    let far = section_idx(9, y, z);

    let lit_far_end = |opaque: bool| {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        for x in 2..=10 {
            section.set_block(x, y, z, Block::Air);
        }
        section.set_block(2, y, z, Block::Torch);
        section.insert_torch(2, y, z, crate::torch::TorchPlacement::Floor);
        section.set_block(stair.0, stair.1, stair.2, Block::OakStairs);
        section.set_stair_state(
            stair.0,
            stair.1,
            stair.2,
            StairState::new(Facing::North, StairHalf::Bottom),
        );
        section.set_custom_light_aperture(section_idx(stair.0, stair.1, stair.2) as u16, opaque);

        let mut sections = crate::world::section_map::SectionMap::default();
        sections.insert(pos, Arc::new(section));
        let mut columns: FxHashMap<ChunkPos, Arc<Column>> = FxHashMap::default();
        for cz in -1..=1 {
            for cx in -1..=1 {
                columns.insert(ChunkPos::new(cx, cz), Arc::new(Column::new()));
            }
        }

        let single = super::bake::bake_section(
            super::bake::SectionBakeJob::snapshot_unchecked(pos, &sections, &columns).unwrap(),
        );
        let batched = run_light_bake_batch(
            snapshot_batch(pos, &[pos], &sections, &columns).expect("batch snapshot"),
        );
        assert_eq!(batched.len(), 1);
        report_first_diff(
            &format!("opaque={opaque}"),
            "block light",
            pos,
            &batched[0].blocklight,
            &single.blocklight,
        );
        !single.blocklight[far].is_dark()
    };

    assert!(
        !lit_far_end(true),
        "a closed custom aperture must stop the light"
    );
    assert!(
        lit_far_end(false),
        "an open custom aperture must pass the light"
    );
}
