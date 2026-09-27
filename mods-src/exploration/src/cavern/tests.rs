use super::dress::{patch_at, vine_at, Dress, STRAY_PER_MILLE};
use super::giants::{beats, could_beat, could_reach_box, roll_giant, Candidate, COMPETE_PAD};
use super::*;
use crate::cascade;
use crate::shroom::{self, Giant, Part};

/// Ground flora must CLUMP. The whole point of the colony lattice is that
/// the floor is not an even dusting, and "it clumps" is exactly the kind of
/// property a later tuning edit silently destroys — drop the falloff and
/// every number below still looks plausible while the caverns go back to
/// confetti. The test checks three spatial properties:
///
/// - the densest columns are far denser than the sparsest (a flat roll
///   gives one number everywhere);
/// - a good share of the floor is near-empty, so patches read as patches;
/// - a cell in a colony overwhelmingly shares its neighbour's species,
///   which is what makes a stand look like one organism's spread.
#[test]
fn ground_flora_grows_in_colonies_not_an_even_dusting() {
    for seed in [0x312u32, 0x1D001, 0x2BEEF] {
        let mut dens: Vec<i32> = Vec::new();
        let (mut same, mut pairs) = (0usize, 0usize);
        for wz in -60..60 {
            for wx in -60..60 {
                let (d, _, salt) = patch_at(seed, wx, wz);
                dens.push(d);
                let (dn, _, sn) = patch_at(seed, wx + 1, wz);
                if salt != 0 && sn != 0 && d > STRAY_PER_MILLE && dn > STRAY_PER_MILLE {
                    pairs += 1;
                    same += (salt % 4 == sn % 4) as usize;
                }
            }
        }
        dens.sort_unstable();
        let p10 = dens[dens.len() / 10];
        let p95 = dens[dens.len() * 95 / 100];
        let bare = dens.iter().filter(|&&d| d <= STRAY_PER_MILLE).count();
        assert!(
            p95 >= p10 * 4,
            "flora is nearly uniform (p10 {p10}, p95 {p95}) — the colonies are gone \
             (seed {seed:#x})"
        );
        assert!(
            bare * 5 >= dens.len(),
            "only {bare}/{} columns are bare; patches need gaps between them \
             (seed {seed:#x})",
            dens.len()
        );
        assert!(
            pairs > 200 && same * 10 >= pairs * 8,
            "neighbouring colony cells agree on species only {same}/{pairs} of the time \
             (seed {seed:#x})"
        );
    }
}

#[test]
fn rolled_mushrooms_never_escape_the_scan_margin() {
    for i in 0..4000i32 {
        let mut rng = GenRng::positional(0xC0FFEE, SALT_GIANT, i, i * 7, i * 13);
        let scale = rng.next_i32(0, 255) as u8;
        let g = Giant::roll(&mut rng, scale);
        assert!(
            g.reach() <= MAX_REACH,
            "reach {} exceeds MAX_REACH {MAX_REACH} for {g:?}",
            g.reach()
        );
        let mut top = 0;
        g.emit(|_, dy, _, _| top = top.max(dy));
        assert!(
            top <= MAX_RISE,
            "rise {top} exceeds MAX_RISE {MAX_RISE} for {g:?}"
        );
    }
}

#[test]
fn compete_pad_covers_every_rolled_cap() {
    let mut worst = 0;
    for i in 0..4000i32 {
        let mut rng = GenRng::positional(0xC0FFEE, SALT_GIANT, i, i * 7, i * 13);
        let scale = rng.next_i32(0, 255) as u8;
        let g = Giant::roll(&mut rng, scale);
        let (cx, cz, r) = g.cap_footprint();
        worst = worst.max(cx.abs().max(cz.abs()) + r);
    }
    assert!(
        2 * worst <= COMPETE_PAD,
        "two rolled caps can compete from {} apart, past COMPETE_PAD {COMPETE_PAD}",
        2 * worst
    );
}

#[test]
fn cap_competition_is_deterministic_and_one_sided() {
    let giant = |cap_r: i32| Giant {
        form: shroom::Form::Flatcap,
        height: 9,
        stem_r: 1,
        cap_r,
        skirt: 1,
        lean_x: 0,
        lean_z: 0,
    };
    let cand = |lat: [i32; 3], x: i32, z: i32, cap_r: i32| Candidate {
        x,
        z,
        cell_floor_y: lat[1] * ANCHOR_LATTICE,
        lat,
        giant: giant(cap_r),
    };
    let a = cand([0, 0, 0], 0, 0, 5);
    let b = cand([1, 0, 0], 8, 0, 5);
    let (ra, rb) = ([0, -30, 0], [8, -30, 0]);
    assert!(beats(&a, ra, &b, rb));
    assert!(!beats(&b, rb, &a, ra));
    let c = cand([1, 0, 0], 10, 0, 5);
    assert!(!beats(&a, ra, &c, [10, -30, 0]));
    assert!(!beats(&a, ra, &b, [8, 30, 0]));
}

#[test]
fn giant_prefilter_keeps_every_body_cell_and_possible_winner() {
    let candidates: Vec<_> = (-5..=5)
        .flat_map(|lz| {
            (-5..=5).flat_map(move |lx| {
                (-2..=2).filter_map(move |ly| roll_giant(0x84713c32, lx, ly, lz))
            })
        })
        .collect();
    for a in &candidates {
        let root = [a.x, a.cell_floor_y + 3, a.z];
        a.giant.emit(|dx, dy, dz, _| {
            let p = [root[0] + dx, root[1] + dy, root[2] + dz];
            assert!(could_reach_box(a, p, p));
        });
        for b in &candidates {
            let other = [b.x, b.cell_floor_y + 3, b.z];
            if beats(a, root, b, other) {
                assert!(could_beat(a, b));
            }
        }
    }
}

#[test]
fn one_dispatch_stays_within_a_couple_of_abi_batches() {
    let cells = |span: i32| (span.div_euclid(ANCHOR_LATTICE) + 2) as usize;
    let giants = cells(16 + 2 * MAX_REACH).pow(2) * cells(16 + MAX_RISE) * PROBE_PER_CANDIDATE;
    let dressing = 2 * 256 + 256 * PROBE_PER_MARGIN;
    let worst = giants + dressing;
    assert!(
        worst <= 2 * SIM_BATCH_MAX,
        "worst-case probe batch {worst} ({giants} giants, {dressing} dressing) \
         needs more than two ABI batches"
    );
}

#[test]
fn a_dispatch_consults_only_a_handful_of_cascade_cells() {
    for origin in [[0, 0, 0], [16, -48, -16], [-16, -64, 48]] {
        let cells = cascade::cells_overlapping(origin, CLAIM_ROWS).len();
        assert!(cells <= 12, "a dispatch consults {cells} cascade cells");
    }
}

/// A vine curtain hangs DOWN, so it routinely crosses the floor of the
/// section its root sits in. Every cell of a run must therefore be reachable
/// by the section that OWNS that cell — either because the root is inside
/// it, or because the root falls in the margin rows it scans over its own
/// roof. Miss that and curtains end on the `y % 16 == 0` planes, which reads
/// as a short vine instead of a continuous curtain.
#[test]
fn every_cell_of_a_curtain_is_reachable_by_the_section_that_owns_it() {
    for root in -40..40i32 {
        for len in 1..=VINE_MAX_LEN {
            for d in 0..len {
                let cell = root - d;
                let origin = cell.div_euclid(16) * 16;
                let ly = root - origin;
                assert!(
                    (0..16 + CEILING_MARGIN).contains(&ly),
                    "root {root} writes {cell}, but the section at {origin} \
                     that owns that cell never scans row {ly}"
                );
            }
        }
    }
}

#[test]
fn the_ceiling_margin_is_no_wider_than_a_curtain_reaches() {
    assert_eq!(CEILING_MARGIN, VINE_MAX_LEN - 1);
    assert_eq!(15 + VINE_MAX_LEN - 1, 15 + CEILING_MARGIN);
}

fn split_section(section: [i32; 3]) -> GenCtx {
    let mut blocks = vec![0u16; 4096];
    for ly in 0..8 {
        for lz in 0..16 {
            for lx in 0..16 {
                blocks[ly * 256 + lz * 16 + lx] = 3;
            }
        }
    }
    GenCtx::for_test(section, 0xC0FFEE, blocks, vec![64; 256], vec![0; 256], 62)
}

fn open_section(section: [i32; 3]) -> GenCtx {
    GenCtx::for_test(
        section,
        0xC0FFEE,
        vec![0u16; 4096],
        vec![64; 256],
        vec![0; 256],
        62,
    )
}

/// Floor flora checks the cell under the candidate. On row 0 that cell is in the section below.
/// Answering "not solid" there left every `y % 16 == 0` plane without flora, including the world
/// floor, the widest flat floor in the cavern, while giants standing on it generated fine.
#[test]
fn the_bottom_row_asks_the_terrain_for_the_support_it_cannot_see() {
    let ctx = open_section([0, -3, 0]);
    let origin = ctx.origin_world();
    let Dressing {
        floors, ceilings, ..
    } = Dressing::gather(&test_content(), &ctx, ctx.seed());
    let bottom: Vec<&Dress> = floors.iter().filter(|d| d.p[1] == origin[1]).collect();
    assert!(
        !bottom.is_empty(),
        "no candidate rolled on the bottom row; the test proves nothing"
    );
    for d in bottom {
        assert_eq!(d.below, None, "row 0 cannot see its own support");
        assert_eq!(
            d.unseen(),
            Some([d.p[0], origin[1] - 1, d.p[2]]),
            "the probe must be the support cell, not the roof"
        );
    }
    let top: Vec<&Dress> = ceilings
        .iter()
        .filter(|d| d.p[1] == origin[1] + 15)
        .collect();
    assert!(!top.is_empty(), "no candidate rolled on the top row");
    for d in top {
        assert_eq!(
            d.unseen(),
            Some([d.p[0], origin[1] + 16, d.p[2]]),
            "row 15 must probe the roof it cannot see"
        );
    }
}

#[test]
fn a_candidate_that_can_see_both_neighbours_costs_no_probe() {
    let ctx = split_section([0, -3, 0]);
    let Dressing {
        floors, ceilings, ..
    } = Dressing::gather(&test_content(), &ctx, ctx.seed());
    let mut inner = 0;
    for d in floors.iter().chain(&ceilings) {
        let ly = d.p[1] - ctx.origin_world()[1];
        if (1..15).contains(&ly) {
            inner += 1;
            assert_eq!(d.unseen(), None, "row {ly} sees both its neighbours");
        }
    }
    assert!(inner > 0, "no interior candidate rolled");
    for d in &floors {
        assert_eq!(d.below, Some(true), "a floor candidate rests on rock");
    }
}

/// A curtain rooted over our roof must be re-derived HERE, because the
/// section that owns the root cannot write into us. Roots are scanned in
/// the margin rows and only in columns whose top row is open, which is
/// exactly the set of columns a curtain can reach us through.
#[test]
fn roots_above_the_roof_are_scanned_when_a_curtain_can_reach_in() {
    let ctx = open_section([0, -3, 0]);
    let origin = ctx.origin_world();
    let Dressing {
        margins,
        margin_cols: cols,
        ..
    } = Dressing::gather(&test_content(), &ctx, ctx.seed());
    assert!(
        !margins.is_empty(),
        "no margin root rolled over an open roof"
    );
    for m in &margins {
        let ly = m.p[1] - origin[1];
        assert!(
            (16..16 + CEILING_MARGIN).contains(&ly),
            "margin root at row {ly} is outside the scanned band"
        );
        let c = &cols[m.col];
        assert_eq!(c.xz, [m.p[0], m.p[2]], "root filed under the wrong column");
        assert!(
            c.rows >= (ly - 16) as usize + 2 && c.rows <= PROBE_PER_MARGIN,
            "a root on row {ly} reads past its column's {} probed rows",
            c.rows
        );
    }
    let sealed = split_section([0, -3, 0]);
    let mut blocks = vec![3u16; 4096];
    for block in blocks.iter_mut().take(256) {
        *block = 0;
    }
    let sealed = GenCtx::for_test(
        sealed.section_pos(),
        sealed.seed(),
        blocks,
        vec![64; 256],
        vec![0; 256],
        62,
    );
    let Dressing { margins, .. } = Dressing::gather(&test_content(), &sealed, sealed.seed());
    assert!(margins.is_empty(), "a sealed roof cannot admit a curtain");
}

fn test_content() -> Content {
    Content {
        stem: BlockId(200),
        vine: BlockId(202),
        water: BlockId(203),
        fluids: crate::fluids::Fluids::of(&[BlockId(203), BlockId(205)]),
        silt: BlockId(204),
        air: BlockId::AIR,
        species: (0..4)
            .map(|i| Species {
                cap: BlockId(210 + i),
                sporeshroom: BlockId(220 + i),
                flower: BlockId(230 + i),
                glow_vine: BlockId(240 + i),
            })
            .collect(),
    }
}

#[test]
fn a_dressing_block_never_replaces_a_structural_block() {
    let ctx = open_section([0, -3, 0]);
    let content = test_content();
    let mut out = Emitter::new(&ctx);

    let mut rng = GenRng::positional(ctx.seed(), SALT_GIANT, 1, 2, 3);
    let giant = Giant::roll(&mut rng, 200);
    let root = [
        ctx.origin_world()[0] + 8,
        ctx.origin_world()[1],
        ctx.origin_world()[2] + 8,
    ];
    giant.emit(|dx, dy, dz, part| {
        let block = match part {
            Part::Stem => content.stem,
            Part::Cap | Part::Gill => content.species[0].cap,
        };
        out.push_if_clear([root[0] + dx, root[1] + dy, root[2] + dz], block);
    });
    let structural: Vec<GenWrite> = out.writes().to_vec();
    assert!(
        !structural.is_empty(),
        "the giant emitted nothing into the section"
    );

    for &(p, _) in &structural {
        out.push_if_clear(p, content.species[1].flower);
        out.push_if_clear(p, content.vine);
    }
    assert_eq!(
        out.writes().len(),
        structural.len(),
        "a dressing write took a structural cell"
    );

    let mut seen = std::collections::HashSet::new();
    for &(p, b) in out.writes() {
        assert!(seen.insert(p), "the same cell was written twice: {p:?}");
        assert!(
            b == content.stem || b == content.species[0].cap,
            "cell {p:?} holds {b:?}, not the mushroom's own block"
        );
    }
}

#[test]
fn species_are_stable_across_a_stand_and_vary_between_stands() {
    let content = test_content();
    let at = |x, z| pick_species(&content, 7, x, -40, z).cap.0;
    let base = at(0, 0);
    for d in 0..8 {
        assert_eq!(at(d, d), base, "species changed inside one stand");
    }
    let far: Vec<u16> = (1..12).map(|k| at(k * 40, k * 40)).collect();
    assert!(
        far.iter().any(|&c| c != base),
        "species never varies between distant stands: {far:?}"
    );
}

/// Curtain only cares about the cell's world position, nothing else.
///
/// Run is gathered from the dispatching section's own snapshot. Use the root's rng stream or run
/// offset instead and you get different answers depending on who wrote the cell and how long that
/// curtain rolled. Overlapping curtains must agree on shared cells. Bloom color follows the cell's
/// stand, not the root's.
#[test]
fn a_curtain_picks_each_segment_from_that_cell_alone() {
    let content = test_content();
    let seed = 0x1D001;
    let run = |root: i32, len: i32| -> Vec<u16> {
        let mut stream = GenRng::positional(seed, SALT_CEILING, 4, root, -9);
        (0..len)
            .map(|d| {
                let _ = stream.next_i32(0, 999);
                vine_at(&content, seed, [4, root - d, -9]).0
            })
            .collect()
    };
    assert_eq!(
        run(-30, 7)[3..],
        run(-33, 4)[..],
        "two curtains disagree about the cells they share"
    );

    let curtains: Vec<Vec<[i32; 3]>> = (0..40)
        .flat_map(|x| (0..40).map(move |z| (x, z)))
        .map(|(x, z)| (0..VINE_MAX_LEN).map(|d| [x, -34 - d, z]).collect())
        .collect();
    let mut cells = 0usize;
    let mut blooms = 0usize;
    let mut mixed = 0usize;
    for curtain in &curtains {
        let run: Vec<BlockId> = curtain
            .iter()
            .map(|&c| vine_at(&content, seed, c))
            .collect();
        for (&cell, &block) in curtain.iter().zip(&run) {
            assert!(
                block == content.vine
                    || block == pick_species(&content, seed, cell[0], cell[1], cell[2]).glow_vine,
                "the bloom at {cell:?} is not the colour of the stand it hangs in"
            );
        }
        cells += run.len();
        blooms += run.iter().filter(|&&b| b != content.vine).count();
        if run.contains(&content.vine) && run.iter().any(|&b| b != content.vine) {
            mixed += 1;
        }
    }
    let share = blooms as f64 / cells as f64;
    assert!(
        (0.02..0.25).contains(&share),
        "blooms are an accent on a plain strand, not absent and not the \
         norm: {share:.3} of {cells} cells"
    );
    assert!(
        mixed * 4 > curtains.len(),
        "only {mixed} of {} curtains mix plain and flowering segments",
        curtains.len()
    );
}

#[test]
fn a_section_above_the_biome_band_does_no_work_at_all() {
    let content = test_content();
    let first_clear = TOP_CONTENT_Y.div_euclid(16) + 1;
    let ctx = split_section([0, first_clear, 0]);
    assert!(generate(&content, &ctx).is_ok_and(|writes| writes.is_empty()));
}

fn all_ours(positions: Vec<[i32; 3]>) -> Vec<u8> {
    vec![7; positions.len()]
}

fn floor_under_section(positions: Vec<[i32; 3]>) -> Vec<TerrainSpace> {
    positions
        .into_iter()
        .map(|p| {
            if p[1] < -48 {
                TerrainSpace::Solid
            } else {
                TerrainSpace::Air
            }
        })
        .collect()
}

#[test]
fn the_bottom_plane_is_dressed_from_positional_support() {
    let ctx = open_section([0, -3, 0]);
    let content = test_content();
    let resolved = Dressing::gather(&content, &ctx, ctx.seed())
        .resolve(ctx.origin_world(), 7, all_ours, floor_under_section)
        .expect("the fake host answers in full");
    let mut out = Emitter::new(&ctx);
    resolved.emit(&content, &mut out, ctx.seed());
    let flora: Vec<BlockId> = content
        .species
        .iter()
        .flat_map(|s| [s.flower, s.sporeshroom])
        .collect();
    assert!(!out.writes().is_empty(), "the bottom plane was left bare");
    for &(p, b) in out.writes() {
        assert_eq!(p[1], -48, "{p:?} dressed off the floor plane");
        assert!(flora.contains(&b), "{p:?} holds {b:?}, not floor flora");
    }
}
