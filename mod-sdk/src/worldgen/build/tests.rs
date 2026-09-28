use super::*;
use crate::{BlockId, GenOutput, GenRng};

fn families() -> Families {
    Families::from_rows(
        [
            ("t:oak_planks", "oak", "block"),
            ("t:oak_slab", "oak", "slab"),
            ("t:oak_log", "oak", "log"),
        ]
        .map(|(b, f, k)| (b.to_string(), f.to_string(), k.to_string())),
    )
}

fn ids(name: &str) -> Option<BlockId> {
    Some(BlockId(match name {
        "petramond:air" => 0,
        "t:oak_planks" => 1,
        "t:oak_slab" => 2,
        "t:oak_log" => 3,
        "t:door" => 4,
        _ => return None,
    }))
}

#[test]
fn rings_close_face_to_face_without_revisiting_a_cell() {
    for seed in 0..200u64 {
        let mut rng = GenRng::positional(seed as u32, 7, 0, 0, 0);
        let base = rng.range(6.0, 30.0);
        let harmonics: Vec<(f32, f32, f32)> = (2..6)
            .map(|k| {
                (
                    k as f32,
                    rng.range(0.0, 0.12),
                    rng.range(0.0, std::f32::consts::TAU),
                )
            })
            .collect();
        let ring = closed_ring([3, -9], base, |cos, sin| {
            let th = sin.atan2(cos);
            base * (1.0
                + harmonics
                    .iter()
                    .map(|(k, a, p)| a * (k * th + p).sin())
                    .sum::<f32>())
        });
        let mut seen = std::collections::HashSet::new();
        for (i, c) in ring.iter().enumerate() {
            let n = ring[(i + 1) % ring.len()];
            assert_eq!(
                (c[0] - n[0]).abs() + (c[1] - n[1]).abs(),
                1,
                "seed {seed} step {i}"
            );
            assert!(seen.insert(*c), "seed {seed} revisits {c:?}");
        }
        let field = depths_inside(&ring, [3, -9]);
        assert_eq!(
            field.depth([3, -9]).map(|d| d > 0),
            Some(true),
            "seed {seed}"
        );
    }
}

#[test]
fn writing_into_a_multi_cell_object_removes_all_of_it() {
    let mut plan = Plan::new();
    plan.object(
        [0, 10, 0],
        &Material::named("t:door"),
        &[[0, 0, 0], [0, 1, 0]],
    );
    assert!(plan.occupied([0, 11, 0]));
    plan.set([0, 11, 0], &Material::named("t:oak_planks"));
    assert!(
        plan.get([0, 10, 0]).is_none(),
        "the door's lower half left with it"
    );
    assert_eq!(plan.len(), 1);
}

#[test]
fn slabs_never_leave_a_half_block_gap_against_what_holds_them() {
    let fam = families();
    let slab = fam.get(Name::new("oak"), Form::Slab).unwrap();
    let log = fam.get(Name::new("oak"), Form::Log).unwrap();
    let mut plan = Plan::new();
    plan.set([0, 11, 0], &log);
    plan.set([0, 12, 0], &slab.half(Half::Top));
    plan.set([2, 12, 0], &slab.half(Half::Bottom));
    plan.set([2, 13, 0], &log);
    plan.set([4, 12, 0], &slab.half(Half::Top));
    plan.set([6, 12, 0], &slab.half(Half::Bottom));
    plan.settle_slabs(&fam, |_, _| 10);
    let form = |p| plan.get(p).unwrap().form;
    assert_eq!(form([0, 12, 0]), Form::Block, "top slab resting on a post");
    assert_eq!(form([2, 12, 0]), Form::Block, "bottom slab under a post");
    assert_eq!(
        form([4, 12, 0]),
        Form::Slab,
        "a top slab over air is a real overhang"
    );
    assert_eq!(
        form([6, 12, 0]),
        Form::Slab,
        "a bottom slab under air stays"
    );
}

#[test]
fn a_published_plan_emits_state_data_and_clears_per_section() {
    let fam = families();
    let mut plan = Plan::new();
    plan.set(
        [-3, 70, 5],
        &fam.get(Name::new("oak"), Form::Slab)
            .unwrap()
            .half(Half::Top),
    );
    plan.object(
        [1, 15, 0],
        &Material::named("t:door").facing(Dir::South),
        &[[0, 0, 0], [0, 1, 0]],
    );
    plan.set([1, 20, 0], &Material::named("t:oak_planks"));
    plan.data([1, 20, 0], "t:loot", vec![1]);
    plan.set([2, 20, 0], &Material::named("t:unknown"));
    plan.data([2, 20, 0], "t:loot", vec![2]);
    plan.clear_column(1, 0, 14, 22);
    let mut plan = IndexedPlan::parse(plan.encode(), 0).expect("parses");
    let mut resolve = ids;

    let mut upper = GenOutput::default();
    plan.emit([0, 1, 0], &mut resolve, &mut upper);
    let door = upper
        .authored
        .cells
        .iter()
        .find(|(p, _)| *p == [1, 15, 0])
        .expect("the door's anchor reaches the section its top half is in");
    let state = |out: &GenOutput, material: u16| -> Vec<(String, String)> {
        let entry = out.authored.palette.get(material as usize).unwrap();
        entry
            .state()
            .map(|(k, v)| (k.to_owned(), v.to_owned()))
            .collect()
    };
    assert!(state(&upper, door.1).contains(&("facing".into(), "south".into())));
    assert!(upper.authored.cells.iter().any(|(p, _)| p == [1, 20, 0]));
    assert_eq!(
        upper.authored.data.len(),
        1,
        "data only rides a cell that was emitted"
    );
    assert_eq!(
        upper
            .fills
            .iter()
            .map(|f| (f.min, f.max))
            .collect::<Vec<_>>(),
        vec![([1, 16, 0], [1, 22, 0])],
        "the clear is clipped to the section"
    );

    let mut lower = GenOutput::default();
    plan.emit([0, 0, 0], &mut resolve, &mut lower);
    assert!(lower.authored.cells.iter().any(|(p, _)| p == [1, 15, 0]));
    assert_eq!(
        lower
            .fills
            .iter()
            .map(|f| (f.min, f.max))
            .collect::<Vec<_>>(),
        vec![([1, 14, 0], [1, 15, 0])]
    );

    let mut far = GenOutput::default();
    plan.emit([-1, 4, 0], &mut resolve, &mut far);
    let slab = far.authored.cells.iter().next().unwrap().1;
    assert!(state(&far, slab).contains(&("half".into(), "top".into())));
}

#[test]
fn merged_clears_cover_exactly_the_cleared_cells() {
    for seed in 0..60u32 {
        let mut rng = GenRng::positional(seed, 11, 0, 0, 0);
        let mut plan = Plan::new();
        let mut want = std::collections::BTreeSet::new();
        for _ in 0..rng.int(1, 400) {
            let (x, z) = (rng.int(-20, 20), rng.int(-20, 20));
            let lo = rng.int(-10, 30);
            let hi = lo + [0, 3, 40][rng.int(0, 2) as usize];
            plan.clear_column(x, z, lo, hi);
            for y in lo..=hi {
                want.insert([x, y, z]);
            }
        }
        if seed % 2 == 1 {
            let (min, width) = ([rng.int(-20, 0), rng.int(-20, 0)], rng.int(1, 20) as usize);
            let ranges: Vec<(i32, i32)> = (0..width * rng.int(1, 20) as usize)
                .map(|_| {
                    let lo = rng.int(-10, 30);
                    (lo, lo + rng.int(-2, 40))
                })
                .collect();
            for (i, &(lo, hi)) in ranges.iter().enumerate() {
                let (x, z) = (min[0] + (i % width) as i32, min[1] + (i / width) as i32);
                for y in lo..=hi {
                    want.insert([x, y, z]);
                }
            }
            plan.clear_columns(min, width, ranges);
        }
        let mut plan = IndexedPlan::parse(plan.encode(), 0).unwrap();
        let mut got = std::collections::BTreeSet::new();
        for sx in -2..=1 {
            for sy in -1..=4 {
                for sz in -2..=1 {
                    let mut out = GenOutput::default();
                    plan.emit([sx, sy, sz], &mut ids, &mut out);
                    for f in out.fills.iter() {
                        for x in f.min[0]..=f.max[0] {
                            for y in f.min[1]..=f.max[1] {
                                for z in f.min[2]..=f.max[2] {
                                    assert!(got.insert([x, y, z]), "seed {seed}: overlap");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(got, want, "seed {seed}");
    }
}

#[test]
fn a_frame_maps_its_footprint_onto_the_world_rectangle_for_every_facing() {
    for fwd in Dir::ALL {
        let (w, d) = (5, 3);
        let frame = Frame::new([10, -4], w, d, fwd);
        let size = Frame::world_size(w, d, fwd);
        let mut cells: Vec<[i32; 2]> = (0..d)
            .flat_map(|z| (0..w).map(move |x| (x, z)))
            .map(|(x, z)| frame.at(x, z))
            .collect();
        cells.sort_unstable();
        let mut want: Vec<[i32; 2]> = (0..size[1])
            .flat_map(|z| (0..size[0]).map(move |x| [10 + x, -4 + z]))
            .collect();
        want.sort_unstable();
        assert_eq!(cells, want, "{fwd:?}");
        let a = frame.at(0, 0);
        let b = frame.at(0, 1);
        assert_eq!(
            [b[0] - a[0], b[1] - a[1]],
            fwd.offset(),
            "{fwd:?}: local +z points fwd"
        );
    }
}

#[test]
fn value_noise_stays_in_range_without_jumps() {
    let noise = super::Noise2(0x9e37_79b9);
    let mut prev: Option<f32> = None;
    for i in 0..50_000 {
        let (x, z) = (-300.0 + i as f32 * 0.011, 41.7 - i as f32 * 0.007);
        let v = noise.at(x, z);
        assert!((-1.0..=1.0).contains(&v), "{v} at {x} {z}");
        if let Some(p) = prev {
            assert!((v - p).abs() < 0.1, "jump from {p} to {v} at {x} {z}");
        }
        prev = Some(v);
    }
}
