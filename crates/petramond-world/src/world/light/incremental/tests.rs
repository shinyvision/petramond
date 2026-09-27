use std::sync::Arc;

use super::*;
use crate::chunk::{self, section_idx, ChunkPos, SectionPos};
use crate::torch::TorchPlacement;
use crate::world::light::test_fixture::{place_stair, Fixture, Rng};

const LOW: SectionPos = SectionPos::new(-1, -1, -1);
const SPAN: usize = 4;

fn random_edit(f: &mut Fixture, rng: &mut Rng, cell: IVec3) -> bool {
    let (lx, lz) = (chunk::lx(cell.x), chunk::lz(cell.z));
    let cover =
        f.columns[&ChunkPos::new(cell.x.div_euclid(16), cell.z.div_euclid(16))].sky_cover_y(lx, lz);
    let roll = rng.next() % 6;
    let (section, lx, ly, lz) = f.cell_mut(cell).expect("edit inside the window");
    if roll == 0 {
        if section.block(lx, ly, lz) != Block::OakStairs {
            return false;
        }
        let idx = section_idx(lx, ly, lz) as u16;
        let opaque = section
            .custom_light_apertures()
            .and_then(|m| m.get(&idx).copied())
            .unwrap_or(false);
        section.set_custom_light_aperture(idx, !opaque);
        return true;
    }
    let new = match roll {
        1 => Block::Air,
        2 => Block::Stone,
        3 => Block::Torch,
        4 => Block::Glass,
        _ => Block::OakStairs,
    };
    let transmits = new.transmits_direct_skylight();
    if (cell.y > cover && !transmits) || (cell.y == cover && transmits) {
        return false;
    }
    match new {
        Block::OakStairs => place_stair(rng, section, lx, ly, lz),
        Block::Torch => {
            section.set_block(lx, ly, lz, new);
            section.insert_torch(lx, ly, lz, TorchPlacement::Floor);
        }
        _ => section.set_block(lx, ly, lz, new),
    }
    section.mark_light_clean();
    true
}

fn interior_cell(rng: &mut Rng) -> IVec3 {
    let mut axis = |low: i32| low * 16 + 16 + (rng.next() % 32) as i32;
    IVec3::new(axis(LOW.cx), axis(LOW.cy), axis(LOW.cz))
}

#[test]
fn incremental_relight_equals_full_rebakes() {
    let mut rng = Rng(0x5eed_1a57_0bad_f00d);
    let mut f = Fixture::random(&mut rng, LOW, SPAN, 0);
    f.bake_all();
    f.assert_matches_full_bakes("fixture");

    let mut relit_total = 0;
    for round in 0..16 {
        let want = 1 + (rng.next() % 5) as usize;
        let mut edits = Vec::with_capacity(want);
        while edits.len() < want {
            let cell = interior_cell(&mut rng);
            if random_edit(&mut f, &mut rng, cell) {
                edits.push(cell);
            }
        }
        for &cell in &edits {
            assert!(
                edit_relightable(&f.sections, cell),
                "round {round}: {cell:?}"
            );
        }
        let relit = relight_edits(&f.sections, &f.columns, &edits)
            .unwrap_or_else(|| panic!("round {round}: a fully baked window must relight"));
        relit_total += relit.len();
        for r in relit {
            assert_ne!(
                r.mask, 0,
                "round {round}: {:?} reported an empty change",
                r.pos
            );
            f.install(r.pos, r.skylight, r.blocklight);
        }
        f.assert_matches_full_bakes(&format!("round {round}"));
    }
    assert!(relit_total > 0, "the edits must have moved some light");
}

#[test]
fn an_untrustworthy_region_declines() {
    let mut rng = Rng(0x0dd_ba11_c0ff_ee00);
    let mut f = Fixture::random(&mut rng, LOW, SPAN, 0);
    f.bake_all();
    let cell = IVec3::new(16, 16, 16);
    assert!(edit_relightable(&f.sections, cell));
    let relit = relight_edits(&f.sections, &f.columns, &[cell]).expect("baked region");
    assert!(relit.is_empty(), "an unchanged cell relights nothing");

    let neighbour = SectionPos::new(0, 1, 1);
    let taken = f.sections.remove(&neighbour).expect("fixture section");
    assert!(!edit_relightable(&f.sections, cell), "absent neighbour");
    assert!(relight_edits(&f.sections, &f.columns, &[cell]).is_none());

    let mut dirty = (*taken).clone();
    dirty.mark_light_dirty();
    f.sections.insert(neighbour, Arc::new(dirty));
    assert!(!edit_relightable(&f.sections, cell), "awaiting a bake");
    assert!(relight_edits(&f.sections, &f.columns, &[cell]).is_none());
}

#[test]
fn an_unbaked_opaque_neighbour_reads_as_implied_light() {
    let mut rng = Rng(0xfeed_face_1234_5678);
    let mut f = Fixture::random(&mut rng, LOW, SPAN, 0);
    let solid = SectionPos::new(0, 0, 0);
    let stone = || {
        let mut s = Section::new(solid.cx, solid.cy, solid.cz);
        s.blocks_mut().fill(Block::Stone.id());
        s.recompute_opaque_count();
        s
    };
    f.sections.insert(solid, Arc::new(stone()));
    f.columns = f.derived_columns();
    f.bake_all();
    f.sections.insert(solid, Arc::new(stone()));
    assert!(f.sections[&solid].light_dirty && f.sections[&solid].all_opaque());

    let mut edits = Vec::new();
    while edits.len() < 3 {
        let cell = IVec3::new((rng.next() % 16) as i32, 16, (rng.next() % 16) as i32);
        if random_edit(&mut f, &mut rng, cell) {
            edits.push(cell);
        }
    }
    let relit = relight_edits(&f.sections, &f.columns, &edits).expect("implied neighbour");
    assert!(
        relit.iter().all(|r| r.pos != solid),
        "an opaque section is never relit"
    );
    for r in relit {
        f.install(r.pos, r.skylight, r.blocklight);
    }
    f.assert_matches_full_bakes("beside an opaque section");
}
