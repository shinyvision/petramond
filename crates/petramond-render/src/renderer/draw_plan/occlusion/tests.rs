use super::*;

fn world(r: i32, vis: impl Fn(SectionPos) -> SectionVisibility) -> SectionOcclusion {
    let mut occlusion = SectionOcclusion::default();
    for cx in -r..=r {
        for cz in -r..=r {
            occlusion.insert_column(
                ChunkPos::new(cx, cz),
                (0, 3),
                (0..4).map(|cy| (cy, vis(SectionPos::new(cx, cy, cz)))),
            );
        }
    }
    occlusion
}

fn inside(r: i32) -> impl Fn(SectionPos) -> bool {
    move |p: SectionPos| p.cx.abs() <= r && p.cz.abs() <= r
}

fn visible_count(occlusion: &SectionOcclusion, r: i32) -> usize {
    let mut n = 0;
    for cx in -r..=r {
        for cz in -r..=r {
            for cy in 0..4 {
                n += occlusion.is_visible(SectionPos::new(cx, cy, cz)) as usize;
            }
        }
    }
    n
}

#[test]
fn an_open_field_hides_nothing() {
    let mut occlusion = world(4, |_| SectionVisibility::ALL);
    assert!(occlusion.flood(SectionPos::new(0, 2, 0), 8, inside(4)));
    assert_eq!(visible_count(&occlusion, 4), 9 * 9 * 4);
}

#[test]
fn a_sealed_cave_hides_everything_behind_its_walls() {
    let camera = SectionPos::new(0, 1, 0);
    let mut occlusion = world(4, |p| {
        if p == camera {
            SectionVisibility::ALL
        } else {
            SectionVisibility::NONE
        }
    });
    assert!(occlusion.flood(camera, 8, inside(4)));
    assert!(occlusion.is_visible(camera));
    for face in Face::ALL {
        let glam::IVec3 {
            x: dx,
            y: dy,
            z: dz,
        } = face.dir();
        let wall = SectionPos::new(dx, 1 + dy, dz);
        assert!(occlusion.is_visible(wall), "{face:?} wall");
    }
    assert!(!occlusion.is_visible(SectionPos::new(2, 1, 0)));
    assert!(!occlusion.is_visible(SectionPos::new(1, 1, 1)));
    assert_eq!(visible_count(&occlusion, 4), 7);
}

#[test]
fn a_tunnel_is_seen_along_its_length_only() {
    let tunnel = SectionVisibility::from_pairs(&[(Face::PosX, Face::NegX)]);
    let camera = SectionPos::new(-4, 1, 0);
    let mut occlusion = world(4, |p| {
        if p.cy == 1 && p.cz == 0 {
            tunnel
        } else {
            SectionVisibility::NONE
        }
    });
    assert!(occlusion.flood(camera, 8, inside(4)));
    for cx in -4..=4 {
        assert!(
            occlusion.is_visible(SectionPos::new(cx, 1, 0)),
            "tunnel {cx}"
        );
    }
    assert!(occlusion.is_visible(SectionPos::new(-4, 1, 1)));
    assert!(!occlusion.is_visible(SectionPos::new(0, 1, 1)));
    assert!(!occlusion.is_visible(SectionPos::new(0, 2, 0)));
}

#[test]
fn sight_lines_never_step_back_toward_the_camera() {
    let camera = SectionPos::new(0, 0, 0);
    assert!(
        moves_away(camera, camera, Face::NegX),
        "the camera's own section"
    );
    assert!(moves_away(camera, SectionPos::new(2, 0, 0), Face::PosX));
    assert!(!moves_away(camera, SectionPos::new(2, 0, 0), Face::NegX));
    assert!(moves_away(camera, SectionPos::new(2, 0, 0), Face::PosZ));
    assert!(!moves_away(camera, SectionPos::new(0, -3, 0), Face::PosY));
}

#[test]
fn unrecorded_sections_are_open() {
    let mut occlusion = SectionOcclusion::default();
    occlusion.insert_column(
        ChunkPos::new(3, 0),
        (0, 0),
        std::iter::once((0, SectionVisibility::NONE)),
    );
    assert!(
        occlusion.flood(SectionPos::new(0, 0, 0), 8, |p| p.cx.abs() <= 3
            && p.cz == 0)
    );
    assert!(
        occlusion.is_visible(SectionPos::new(3, 0, 0)),
        "reached through open air"
    );
    assert!(
        !occlusion.is_visible(SectionPos::new(0, 1, 0)),
        "above the loaded band"
    );
}

#[test]
fn nothing_loaded_means_nothing_to_cull_against() {
    let mut occlusion = SectionOcclusion::default();
    assert!(!occlusion.flood(SectionPos::new(0, 0, 0), 8, |_| true));
}

/// The plain hash-set breadth-first flood over (section, entry face) states that the dense-grid
/// flood replaced.
fn reference_flood(
    vis: &dyn Fn(SectionPos) -> SectionVisibility,
    cy_span: (i32, i32),
    camera: SectionPos,
    admit: &dyn Fn(SectionPos) -> bool,
) -> std::collections::HashSet<SectionPos> {
    let mut visible = std::collections::HashSet::from([camera]);
    let mut entered: std::collections::HashMap<SectionPos, u8> = Default::default();
    let mut queue = std::collections::VecDeque::from([(camera, None::<Face>)]);
    let (lo, hi) = (cy_span.0.min(camera.cy), cy_span.1.max(camera.cy));
    while let Some((pos, entry)) = queue.pop_front() {
        let here = vis(pos);
        for exit in Face::ALL {
            if !moves_away(camera, pos, exit)
                || entry.is_some_and(|entry| !here.connects(entry, exit))
            {
                continue;
            }
            let d = exit.dir();
            let next = SectionPos::new(pos.cx + d.x, pos.cy + d.y, pos.cz + d.z);
            if next.cy < lo || next.cy > hi || !admit(next) {
                continue;
            }
            let entry_face = Face::ALL[exit as usize ^ 1];
            let bit = if vis(next) == SectionVisibility::ALL {
                ANY_ENTRY
            } else {
                1 << entry_face as u8
            };
            let seen = entered.entry(next).or_insert(0);
            if *seen & bit == 0 {
                *seen |= bit;
                visible.insert(next);
                queue.push_back((next, Some(entry_face)));
            }
        }
    }
    visible
}

#[test]
fn the_grid_flood_reaches_exactly_what_a_plain_flood_reaches() {
    let hash = |p: SectionPos, salt: u64| {
        let mut z = (p.cx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (p.cy as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
            ^ (p.cz as u64).wrapping_mul(0x94D0_49BB_1331_11EB)
            ^ salt;
        z ^= z >> 29;
        z.wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 16
    };
    let pairs: Vec<(Face, Face)> = Face::ALL
        .iter()
        .flat_map(|&a| Face::ALL.iter().map(move |&b| (a, b)))
        .collect();
    let pairs = &pairs;
    for salt in 0..24u64 {
        let vis = move |p: SectionPos| match hash(p, salt) % 5 {
            0 => SectionVisibility::ALL,
            1 => SectionVisibility::NONE,
            _ => SectionVisibility::from_pairs(
                &pairs
                    .iter()
                    .copied()
                    .filter(|&(a, b)| hash(p, salt ^ (a as u64 * 6 + b as u64 + 1)) % 3 == 0)
                    .collect::<Vec<_>>(),
            ),
        };
        let occlusion = &mut world(5, vis);
        let camera = SectionPos::new(
            (salt % 5) as i32 - 2,
            (salt % 4) as i32,
            (salt % 3) as i32 - 1,
        );
        let admit = |p: SectionPos| p.cx.abs() <= 5 && p.cz.abs() <= 5 && (p.cx + p.cz) % 7 != 3;
        assert!(occlusion.flood(camera, 8, admit));
        let expected = reference_flood(&vis, (0, 3), camera, &admit);
        for cx in -7..=7 {
            for cz in -7..=7 {
                for cy in -1..=4 {
                    let p = SectionPos::new(cx, cy, cz);
                    assert_eq!(
                        occlusion.is_visible(p),
                        expected.contains(&p),
                        "{p:?} salt {salt}"
                    );
                }
            }
        }
    }
}
