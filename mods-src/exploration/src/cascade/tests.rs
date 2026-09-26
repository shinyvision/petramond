use std::collections::{BTreeMap, HashMap, HashSet};

use mod_sdk::GenRng;

use super::*;

/// Roll candidate cells over a spread of lattice cells, at the two
/// vertical cells the synthetic terrain's floors actually cross.
fn rolled(seed: u32, n: i32) -> Vec<Cell> {
    (0..n)
        .flat_map(|i| Cell::roll(seed, i * 3 + 1, -i.rem_euclid(2), i * 7 - 4))
        .collect()
}

/// A synthetic cave with LONG contour edges: open above a floor that
/// descends ~3 rows every 9 columns of x (terraces running along z),
/// folded into a triangle wave so every lattice cell holds relief, with
/// positional roughness so the lips wander like real ground.
fn terraced(p: [i32; 3]) -> bool {
    let t = p[0].rem_euclid(180);
    let d = t.min(180 - t);
    let rough = GenRng::positional(11, 77, p[0], 0, p[2]).next_i32(0, 2) - 1;
    let floor = 24 - 3 * (d / 9) + rough;
    p[1] < floor
}

/// Dead-flat floor.
fn flat(p: [i32; 3]) -> bool {
    p[1] < 0
}

/// Run the whole pipeline for one cell over a synthetic terrain, exactly
/// as the dispatcher would: coarse scan, traces, band probe, build. The
/// probed set returned is the WINNING trace's, so a `finish` driven from
/// it sees the same terrain the build did.
fn run_built(c: &Cell, terrain: fn([i32; 3]) -> bool) -> Option<(Built, HashSet<[i32; 3]>)> {
    let mut coarse = Vec::new();
    c.coarse_plan(|p| coarse.push(terrain(p)));
    let free: Vec<bool> = coarse.iter().map(|s| !s).collect();
    for t in c.traces(&coarse, &free) {
        let mut probed = HashSet::new();
        t.plan(|p| {
            probed.insert(p);
        });
        let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terrain(p));
        if let Ok(b) = t.build(&oracle) {
            return Some((b, probed));
        }
    }
    None
}

fn run(c: &Cell, terrain: fn([i32; 3]) -> bool) -> Option<Feature> {
    let (b, probed) = run_built(c, terrain)?;
    let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terrain(p));
    Some(b.finish(&oracle, &[]))
}

/// Flat ground must be structurally unacceptable: no step edge exists, so
/// no trace is even offered, and a hand-built trace finds no second
/// terrace. This is the anti-"hole in the floor" guarantee — the failure
/// Rachel rejected twice — enforced as a gate rather than styled around.
#[test]
fn a_cascade_refuses_flat_ground() {
    let mut sited = 0;
    for c in rolled(0xC0FFEE, 4000) {
        let mut coarse = Vec::new();
        c.coarse_plan(|p| coarse.push(flat(p)));
        let free: Vec<bool> = coarse.iter().map(|s| !s).collect();
        if !c.traces(&coarse, &free).is_empty() {
            sited += 1;
        }
    }
    assert_eq!(
        sited, 0,
        "{sited} cells offered a trace on a dead-flat floor"
    );
}

/// On terraced terrain the feature must actually generate — optimism is
/// the whole point of the rework — and every accepted feature must
/// satisfy the invariants the flood is trusted for, re-checked from the
/// outputs alone.
#[test]
fn accepted_cascades_are_sealed_grounded_and_confined() {
    let mut accepted = 0;
    for c in rolled(0xC0FFEE, 400) {
        let Some(f) = run(&c, terraced) else {
            continue;
        };
        accepted += 1;
        let writes: HashMap<[i32; 3], Kind> = f.writes.iter().copied().collect();
        let wet: HashSet<[i32; 3]> = f.wet.iter().copied().collect();
        let solid_final = |p: [i32; 3]| match writes.get(&p) {
            Some(Kind::Silt) => true,
            Some(Kind::Air) | Some(Kind::Water) => false,
            None => terraced(p),
        };
        for &w in &f.wet {
            let below = [w[0], w[1] - 1, w[2]];
            assert!(
                wet.contains(&below) || solid_final(below),
                "wet cell {w:?} hangs over dry open ground"
            );
            if solid_final(below) {
                for (dx, dz) in SIDES {
                    let q = [w[0] + dx, w[1], w[2] + dz];
                    assert!(
                        wet.contains(&q) || solid_final(q),
                        "grounded wet cell {w:?} is open sideways at {q:?}"
                    );
                }
            }
        }
        // Confinement: everything inside the candidate's own lattice cell.
        let cell = |p: [i32; 3]| {
            p[0].div_euclid(LATTICE) == c.lx
                && p[2].div_euclid(LATTICE) == c.lz
                && p[1].div_euclid(LATTICE_Y) == c.ly
        };
        for (p, _) in &f.writes {
            assert!(cell(*p), "write {p:?} leaves the candidate's lattice cell");
        }
        for p in &f.wet {
            assert!(cell(*p), "wet {p:?} leaves the candidate's lattice cell");
        }
    }
    assert!(
        accepted >= 40,
        "only {accepted} of ~200 rolled cells accepted on terrain built to \
         carry them; the gates are wedged shut and the feature is dead"
    );
}

/// The basin must be LONG — it follows a contour, it is not a blob. On
/// terrain whose terraces run the full length of the cell, an accepted
/// feature's water must span tens of blocks along the terrace axis and
/// read as a band, not a disc.
#[test]
fn a_basin_follows_the_contour_for_tens_of_blocks() {
    let mut longest = 0i32;
    let mut checked = 0;
    for c in rolled(0xC0FFEE, 400) {
        let Some(f) = run(&c, terraced) else {
            continue;
        };
        checked += 1;
        let water: Vec<[i32; 3]> = f
            .writes
            .iter()
            .filter(|(_, k)| *k == Kind::Water)
            .map(|(p, _)| *p)
            .collect();
        // terraces run along z in `terraced`
        let (mut z0, mut z1) = (i32::MAX, i32::MIN);
        for p in &water {
            z0 = z0.min(p[2]);
            z1 = z1.max(p[2]);
        }
        longest = longest.max(z1 - z0 + 1);
    }
    assert!(checked >= 40, "only {checked} features to judge");
    assert!(
        longest >= 60,
        "longest basin runs {longest} blocks along the contour; \
         a compact blob is a failure"
    );
}

/// The chain descends: the water sits at several heights and each basin's
/// surface is a full step under the one that feeds it.
#[test]
fn accepted_cascades_descend() {
    let mut checked = 0;
    for c in rolled(0xC0FFEE, 400) {
        let Some((b, _)) = run_built(&c, terraced) else {
            continue;
        };
        checked += 1;
        let count = b.live_counts();
        let live: Vec<usize> = (0..b.basins.pools.len()).filter(|&i| count[i] > 0).collect();
        assert!(live.len() >= MIN_POOLS);
        for &j in &live[1..] {
            assert!(
                b.basins.pools[j] <= b.basins.pools[b.basins.from[j]] - (BED_BAND + 1),
                "basin {j} is not a full step under its source"
            );
        }
    }
    assert!(checked >= 40, "only {checked} chains to judge");
}

/// Probe budgets: the coarse scan is a fixed two batches, and a trace's
/// band plan never exceeds its declared cap however the chain rolls.
#[test]
fn probes_stay_within_budget() {
    for c in rolled(0xC0FFEE, 400) {
        let mut n = 0usize;
        c.coarse_plan(|_| n += 1);
        assert_eq!(n, COARSE_PROBE);
        let mut coarse = Vec::new();
        c.coarse_plan(|p| coarse.push(terraced(p)));
        let free: Vec<bool> = coarse.iter().map(|s| !s).collect();
        for t in c.traces(&coarse, &free) {
            let mut m = 0usize;
            t.plan(|_| m += 1);
            assert!(
                m <= BAND_PROBE_MAX,
                "band plan probes {m} cells against the declared {BAND_PROBE_MAX}"
            );
        }
    }
}

/// A giant rooted in a pool is SUPPRESSED — no giant stands in water,
/// shallow or deep, and the basin's water survives it untouched. A giant
/// that would break containment is likewise suppressed and the basin
/// survives — a giant never vetoes a basin.
#[test]
fn giants_adapt_to_the_basin_never_veto_it() {
    for c in rolled(0xC0FFEE, 400) {
        let Some((b, probed)) = run_built(&c, terraced) else {
            continue;
        };
        let oracle = |p: [i32; 3]| probed.contains(&p).then(|| terraced(p));
        let base = b.finish(&oracle, &[]);

        // A giant standing in the head basin, rooted a block over the bed.
        let (&(x, z), &(_pi, bed)) = b
            .basins
            .cols
            .iter()
            .find(|&(_, &(pi, bed))| pi == 0 && b.basins.pools[pi] - bed >= 2)
            .expect("no deep head column");
        let root = [x, bed + 2, z];
        let mut solid = BTreeSet::new();
        for dy in 0..6 {
            solid.insert([x, root[1] + dy, z]);
        }
        let stander = Intruder {
            key: [1, 2, 3],
            root,
            solid,
        };
        let f = b.finish(&oracle, &[stander]);
        assert!(
            f.suppressed.contains(&[1, 2, 3]),
            "a giant rooted in the pool at {root:?} was not suppressed"
        );
        assert!(
            !f.writes
                .iter()
                .any(|&(p, k)| k == Kind::Silt && p == [x, root[1] - 1, z]),
            "a pedestal was still placed under the suppressed giant"
        );
        assert_eq!(
            base.writes
                .iter()
                .filter(|(_, k)| *k == Kind::Water)
                .count(),
            f.writes.iter().filter(|(_, k)| *k == Kind::Water).count(),
            "suppressing the in-pool giant changed the basin's water"
        );

        // A giant body burying every MOVING water cell (the falls and
        // spill flows — reach minus the still pools): delivery is
        // severed, containment breaks, and it is the GIANT that goes,
        // never the basin.
        let still: HashSet<[i32; 3]> = base
            .writes
            .iter()
            .filter(|(_, k)| *k == Kind::Water)
            .map(|(p, _)| *p)
            .collect();
        let wall: BTreeSet<[i32; 3]> = base
            .wet
            .iter()
            .filter(|p| !still.contains(*p))
            .copied()
            .collect();
        assert!(!wall.is_empty(), "a cascade with no moving water at all");
        let s0 = b.basins.pools[0];
        let blocker = Intruder {
            key: [7, 8, 9],
            root: [x, s0 + 8, z],
            solid: wall,
        };
        let f = b.finish(&oracle, &[blocker]);
        assert_eq!(
            f.suppressed,
            vec![[7, 8, 9]],
            "the blocking giant was not suppressed"
        );
        let base_water: Vec<_> = base
            .writes
            .iter()
            .filter(|(_, k)| *k == Kind::Water)
            .collect();
        let f_water: Vec<_> = f.writes.iter().filter(|(_, k)| *k == Kind::Water).collect();
        assert_eq!(base_water, f_water, "suppression changed the basin's water");
        return;
    }
    panic!("no accepted candidate to test the giant path on");
}

/// Determinism: two builds of the same cell produce identical writes and
/// reserves, in identical order. Everything downstream (probe reply
/// indexing, section-unanimous emission) rests on this.
#[test]
fn a_build_is_deterministic() {
    let mut compared = 0;
    for c in rolled(0xC0FFEE, 200) {
        let Some(a) = run(&c, terraced) else {
            continue;
        };
        let b = run(&c, terraced).unwrap();
        assert_eq!(a.writes, b.writes);
        assert_eq!(a.reserves, b.reserves);
        assert_eq!(a.wet, b.wet);
        compared += 1;
    }
    assert!(compared > 5, "only {compared} features compared");
}

/// Two walled basins on a synthetic floor: pool 0 at `x ∈ -2..=2` (surface
/// 0), pool 1 at `x ∈ 10..=14` (surface -4), both `z ∈ -2..=2`. `chasm`
/// opens the whole column plane at `x = 15`, east of pool 1.
fn two_pools(chasm: bool) -> (basin::Basins, impl Fn([i32; 3]) -> Option<bool>) {
    let mut cols = BTreeMap::new();
    for z in -2..=2 {
        for x in -2..=2 {
            cols.insert((x, z), (0usize, -1));
        }
        for x in 10..=14 {
            cols.insert((x, z), (1usize, -5));
        }
    }
    let basins = basin::Basins {
        pools: vec![0, -4],
        from: vec![0, 0],
        cols,
    };
    let terrain = move |[x, y, z]: [i32; 3]| -> Option<bool> {
        if chasm && x == 15 {
            return Some(false);
        }
        let floor = if x < 5 { 0 } else { -4 };
        let in_pool = z.abs() <= 2 && ((-2..=2).contains(&x) || (10..=14).contains(&x));
        Some(y < floor || (!in_pool && y < floor + 3))
    };
    (basins, terrain)
}

/// A rim with walls all round needs no silt but the beds.
#[test]
fn a_walled_chain_seals_with_beds_alone() {
    let (mut basins, terrain) = two_pools(false);
    let sealed = seal::seal(&mut basins, &terrain).expect("a walled chain seals");
    assert_eq!(basins.cols.len(), 50, "a walled basin retreated");
    assert!(sealed.rim_dam.is_empty(), "walls needed no dam");
    assert_eq!(sealed.silt.len(), 50, "only the bed courses are placed");
}

/// An edge over a chasm does not reject the chain: the water RETREATS from
/// it, and the new edge is dammed on footing instead.
#[test]
fn an_edge_over_a_chasm_retreats_the_water_instead_of_rejecting() {
    let (mut basins, terrain) = two_pools(true);
    let sealed = seal::seal(&mut basins, &terrain).expect("the chain survives a chasm edge");
    assert_eq!(basins.counts(), vec![25, 20], "exactly the chasm-side row retreats");
    assert!(!basins.cols.contains_key(&(14, 0)));
    let dammed: BTreeSet<(i32, i32)> = (-2..=2).map(|z| (14, z)).collect();
    assert_eq!(sealed.rim_dam, dammed, "the retreated row is dammed at the waterline");
    for z in -2..=2 {
        assert!(sealed.silt.contains(&[14, -4, z]), "no dam at the new edge");
    }
}

/// The containment flood: a pool nothing feeds is scenery, and one open side
/// into unprobed terrain is a rejection, never a guess.
#[test]
fn the_flood_rejects_unfed_pools_and_water_past_the_probe() {
    let (basins, terrain) = two_pools(false);
    let wet = basins.wet();
    let empty = BTreeSet::new();
    let geometry = flood::Geometry {
        basins: &basins,
        wet: &wet,
        silt: &empty,
        cuts: &empty,
        bodies: &empty,
    };
    assert_eq!(geometry.flood(&terrain), Err("a basin receives no fall"));
    let leaky = |p: [i32; 3]| match p {
        [3, 0, 0] => Some(false),
        [3, _, _] => None,
        _ => terrain(p),
    };
    assert_eq!(
        geometry.flood(&leaky),
        Err("water reaches past the probed domain")
    );
}

#[test]
fn a_rim_of_tall_walls_is_a_tank_not_a_terrace() {
    let rim: BTreeSet<(i32, i32)> = [(0, 0), (1, 0)].into_iter().collect();
    let column = |x: i32, h: i32| (0..h).map(move |y| [x, y, 0]);
    let low: BTreeSet<[i32; 3]> = column(0, 4).chain(column(1, 1)).collect();
    assert!(!seal::rim_is_a_wall(&low, &rim), "half the rim tall is still a lip");
    let tall: BTreeSet<[i32; 3]> = column(0, 4).chain(column(1, 4)).collect();
    assert!(seal::rim_is_a_wall(&tall, &rim));
}

/// The spill path runs source-first and ends on the plunge column inside
/// the lower basin, through unowned columns only.
#[test]
fn a_notch_path_runs_from_the_source_rim_to_the_plunge() {
    let mut cols = BTreeMap::new();
    cols.insert((0, 0), (1usize, -5));
    cols.insert((3, 0), (0usize, -1));
    assert_eq!(
        notch::notch_path(&cols, 1, 0),
        Some(vec![(2, 0), (1, 0), (0, 0)])
    );
    cols.insert((20, 0), (2usize, -9));
    assert_eq!(
        notch::notch_path(&cols, 2, 0),
        None,
        "a path past the notch budget"
    );
}

/// Column classification against a working surface: a member within the
/// bed band, a shore above it, a seed a full step below, and nothing past
/// the growth band.
#[test]
fn columns_classify_against_a_working_surface() {
    let terrain = |[x, y, _]: [i32; 3]| {
        let floor = if x < 20 { 0 } else { -4 };
        Some(y < floor)
    };
    let trace = Trace {
        samples: vec![(18, 0, 0)],
        anchor: (18, 0),
        s0: 0,
        cell: site::CellBox {
            x0: -100,
            x1: 100,
            z0: -100,
            z1: 100,
            y0: -40,
            y1: 40,
        },
    };
    let survey = basin::Survey::new(&trace, &terrain);
    assert_eq!(survey.classify(18, 0, 0), basin::Mem::In(-1));
    assert_eq!(survey.classify(18, 0, 2), basin::Mem::In(-1));
    assert_eq!(survey.classify(18, 0, -1), basin::Mem::Rock);
    assert_eq!(survey.classify(18, 0, 3), basin::Mem::Step(Some(0)));
    assert_eq!(survey.classify(21, 0, 0), basin::Mem::Step(Some(-4)));
    assert_eq!(
        survey.classify(40, 0, 0),
        basin::Mem::Off,
        "past the growth band"
    );
}

/// The memo encoding round-trips a feature exactly and refuses bytes it did
/// not write.
#[test]
fn a_settled_cell_round_trips_through_the_memo() {
    let feature = Feature {
        writes: vec![
            ([1, -2, 3], Kind::Water),
            ([4, 5, -6], Kind::Silt),
            ([0, 0, 0], Kind::Air),
        ],
        reserves: vec![[7, 8, 9]],
        suppressed: vec![[-1, -1, -1], [2, 2, 2]],
        wet: Vec::new(),
    };
    let bytes = Feature::encode(Some(&feature));
    assert_eq!(bytes.len(), Feature::encoded_len(3, 1, 2), "the size formula drifted");
    let back = Feature::decode(&bytes)
        .expect("well-formed")
        .expect("a cascade");
    assert_eq!(back.writes, feature.writes);
    assert_eq!(back.reserves, feature.reserves);
    assert_eq!(back.suppressed, feature.suppressed);
    assert!(matches!(Feature::decode(&Feature::encode(None)), Some(None)));
    assert!(Feature::decode(&[9]).is_none(), "an unknown tag decoded");
    assert!(
        Feature::decode(&bytes[..bytes.len() - 1]).is_none(),
        "a truncated value decoded"
    );
}

/// A settled feature must always PUBLISH: a lease holder whose value the memo
/// refuses leaves every other worker deferred and then re-flooding the cell.
/// One trace's refined probe is capped at [`BAND_PROBE_MAX`] cells, and every
/// cell a feature names comes from that band (a write, the wet set, the cell
/// over water, a giant anchor overlapping it), so twice the cap per list is a
/// generous ceiling — above one plain memo entry, which is why cascades
/// publish through the paged blob, and inside the blob's limit.
#[test]
fn the_worst_case_feature_fits_the_memo_blob() {
    let ceiling = 2 * BAND_PROBE_MAX;
    let worst = Feature::encoded_len(ceiling, ceiling, ceiling);
    assert!(
        worst <= mod_sdk::MEMO_BLOB_MAX_BYTES,
        "a {worst}-byte feature exceeds the blob limit"
    );
    assert!(
        Feature::encoded_len(BAND_PROBE_MAX, 0, 0) > mod_sdk::MEMO_MAX_VALUE_BYTES,
        "a long basin outgrows one memo entry; the blob is load-bearing"
    );
}
