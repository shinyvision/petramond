use super::*;

fn solid_cube(cull: [Option<u8>; 6]) -> ModelCube {
    ModelCube {
        name: "c".into(),
        from: Vec3::ZERO,
        to: Vec3::ONE,
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some(crate::bbmodel::FaceUv::new([0.0, 0.0, 1.0, 1.0])); 6],
        cull,
    }
}

/// A cullface is authored in MODEL space but tested against the WORLD
/// neighbour, so the template bake must rotate it exactly like the
/// geometry: under a 90°-about-Y base transform the authored -Z cull
/// becomes a -X world test, and under identity it stays -Z.
#[test]
fn cullface_directions_rotate_with_the_facing_bake() {
    let mut cull = [None; 6];
    cull[5] = Some(5); // the north face culls against the north neighbour
    let cubes = [solid_cube(cull)];
    let ao = [[[1.0; 4]; 6]];
    let draw = [[true; 6]];
    let blend = [[false; 6]];

    let turned = bake_cell_template(
        Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2),
        &cubes,
        &[0],
        &ao,
        &draw,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    let culls: Vec<_> = turned.segments.iter().filter_map(|s| s.cull).collect();
    assert_eq!(culls, vec![Face::NegX]);

    let straight = bake_cell_template(
        Mat4::IDENTITY,
        &cubes,
        &[0],
        &ao,
        &draw,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    let gated: Vec<_> = straight
        .segments
        .iter()
        .filter(|s| s.cull.is_some())
        .collect();
    assert_eq!(gated.len(), 1);
    assert_eq!(gated[0].cull, Some(Face::NegZ));
    assert_eq!(gated[0].run.vert_len, 4, "exactly the one gated face");
    let ungated: u32 = straight
        .segments
        .iter()
        .filter(|s| s.cull.is_none())
        .map(|s| s.run.vert_len)
        .sum();
    assert_eq!(ungated, 20, "the other five faces stay ungated");
}

/// A face whose texture rect holds partial-alpha texels bakes into a
/// blend-routed segment of its own; everything else stays opaque.
#[test]
fn partial_alpha_faces_bake_into_blend_segments() {
    let cubes = [solid_cube([None; 6])];
    let ao = [[[1.0; 4]; 6]];
    let draw = [[true; 6]; 1];
    let mut blend = [[false; 6]; 1];
    blend[0][2] = true; // the up face's rect holds semi-transparent texels
    let t = bake_cell_template(
        Mat4::IDENTITY,
        &cubes,
        &[0],
        &ao,
        &draw,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    let blended: u32 = t
        .segments
        .iter()
        .filter(|s| s.blend)
        .map(|s| s.run.vert_len)
        .sum();
    let opaque: u32 = t
        .segments
        .iter()
        .filter(|s| !s.blend)
        .map(|s| s.run.vert_len)
        .sum();
    assert_eq!(blended, 4, "exactly the one semi-transparent face");
    assert_eq!(opaque, 20);
}

/// The model pipelines cull back faces, so the one kept face of a
/// zero-thickness plane bakes a second, reversed copy of itself — a decal
/// stays visible from both sides while solid cubes stay single-sided.
#[test]
fn flat_planes_bake_their_kept_face_twice_with_reversed_winding() {
    let mut plane = solid_cube([None; 6]);
    plane.from = Vec3::new(0.0, 0.0, 0.5);
    plane.to = Vec3::new(1.0, 1.0, 0.5); // flat on Z
    let cubes = [plane];
    let ao = [[[1.0; 4]; 6]];
    let draw = [[true; 6]; 1];
    let blend = [[false; 6]; 1];
    let t = bake_cell_template(
        Mat4::IDENTITY,
        &cubes,
        &[0],
        &ao,
        &draw,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    let total: u32 = t.segments.iter().map(|s| s.run.vert_len).sum();
    assert_eq!(total, 8, "one kept face, emitted front + reversed back");
    // The second quad is the first reversed: same corner positions in
    // [0, 3, 2, 1] order, i.e. the opposite winding over the same plane.
    for i in 0..4 {
        assert_eq!(t.verts[4 + i].pos, t.verts[[0, 3, 2, 1][i]].pos);
    }
}

/// A face whose atlas rect is fully transparent discards every fragment at
/// mip 0 — so the bake drops it outright. Left in, the cutout mip chain
/// would promote its texels to opaque with the neighbouring artwork's
/// colour, and an invisible sliver face would render as a bright line.
#[test]
fn fully_transparent_faces_are_dropped_from_the_bake() {
    let cubes = [solid_cube([None; 6])];
    let ao = [[[1.0; 4]; 6]];
    let mut draw = [[true; 6]; 1];
    draw[0][3] = false; // the down face's rect is fully transparent
    let blend = [[false; 6]; 1];
    let t = bake_cell_template(
        Mat4::IDENTITY,
        &cubes,
        &[0],
        &ao,
        &draw,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    let total: u32 = t.segments.iter().map(|s| s.run.vert_len).sum();
    assert_eq!(total, 20, "five faces bake; the invisible one is gone");
    let mut all_hidden = [[false; 6]; 1];
    all_hidden[0] = [false; 6];
    let t = bake_cell_template(
        Mat4::IDENTITY,
        &cubes,
        &[0],
        &ao,
        &all_hidden,
        &blend,
        &[],
        &[],
        |_| super::super::FaceAppearance::default(),
    );
    assert!(t.segments.is_empty() && t.verts.is_empty());
}
