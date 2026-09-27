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
    assert!(occlusion.flood(SectionPos::new(0, 2, 0), inside(4)));
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
    assert!(occlusion.flood(camera, inside(4)));
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
    assert!(occlusion.flood(camera, inside(4)));
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
    assert!(occlusion.flood(SectionPos::new(0, 0, 0), |p| p.cx.abs() <= 3 && p.cz == 0));
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
    assert!(!occlusion.flood(SectionPos::new(0, 0, 0), |_| true));
}
