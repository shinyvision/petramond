use super::*;

fn rect(u0: f32, v0: f32, u1: f32, v1: f32) -> Rect {
    Rect { u0, v0, u1, v1 }
}

fn area(rects: &[Rect]) -> f32 {
    rects.iter().map(|r| (r.u1 - r.u0) * (r.v1 - r.v0)).sum()
}

fn run_subtract(face: Rect, occ: &[Rect]) -> Vec<Rect> {
    let mut cuts = vec![];
    let mut runs = vec![];
    let mut out = vec![];
    subtract(face, occ, &mut cuts, &mut runs, &mut out);
    out
}

/// Butted contact faces vanish entirely; a partial cover leaves exactly
/// the uncovered area, with no overlapping output rects.
#[test]
fn subtraction_conserves_uncovered_area() {
    let face = rect(0.0, 0.0, 1.0, 1.0);
    assert!(run_subtract(face, &[face]).is_empty(), "full burial");

    // A centred notch: 4 surrounding rects totalling 1 - 1/4.
    let out = run_subtract(face, &[rect(0.25, 0.25, 0.75, 0.75)]);
    assert!((area(&out) - 0.75).abs() < 1e-5, "area {:?}", out);
    // No two output rects overlap.
    for (i, a) in out.iter().enumerate() {
        for b in out.iter().skip(i + 1) {
            let w = (a.u1.min(b.u1) - a.u0.max(b.u0)).max(0.0);
            let h = (a.v1.min(b.v1) - a.v0.max(b.v0)).max(0.0);
            assert!(w * h < 1e-6, "overlap {a:?} {b:?}");
        }
    }
}

/// An uncovered face returns itself; disjoint covers split into bands
/// that re-merge where extents match.
#[test]
fn subtraction_remerges_bands() {
    let face = rect(0.0, 0.0, 1.0, 1.0);
    assert_eq!(run_subtract(face, &[]), vec![face]);

    // A strip across the middle: two rects remain (above and below),
    // NOT four (band split must re-merge horizontally-equal bands).
    let out = run_subtract(face, &[rect(0.0, 0.4, 1.0, 0.6)]);
    assert_eq!(out.len(), 2, "{out:?}");
    assert!((area(&out) - 0.8).abs() < 1e-5);
}

fn plain(min: [f32; 3], max: [f32; 3]) -> ShapeBox {
    // The tile is irrelevant to these geometry tests; any atlas row works.
    let t = petramond_world::tile::Tile::named("stone");
    ShapeBox::uniform(petramond_world::block::Aabb { min, max }, [t, t, t], |_| {
        [1.0, 1.0, 1.0]
    })
}

/// The occluder rule: butting-in-front hides, butting-behind does not,
/// straddling (interpenetration) hides, and coincident same-plane faces
/// of overlapping boxes keep exactly one winner.
#[test]
fn occluder_rule_covers_butting_and_coincidence() {
    let axes = (0usize, 2usize, 1usize); // +X face: u=Z, v=Y
    let rect_full = rect(0.0, 0.0, 1.0, 1.0);
    let mut occ = vec![];

    // In front (butted at d): hides.
    push_occluder(
        &mut occ,
        [0.5, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        axes,
        true,
        0.5,
        false,
        &rect_full,
    );
    assert_eq!(occ.len(), 1);

    // Behind (ends at d): does not hide without the coincidence tie.
    occ.clear();
    push_occluder(
        &mut occ,
        [0.0, 0.0, 0.0],
        [0.5, 1.0, 1.0],
        axes,
        true,
        0.5,
        false,
        &rect_full,
    );
    assert!(occ.is_empty());

    // Behind + coincident + earlier: hides (the tie-break winner).
    push_occluder(
        &mut occ,
        [0.0, 0.0, 0.0],
        [0.5, 1.0, 1.0],
        axes,
        true,
        0.5,
        true,
        &rect_full,
    );
    assert_eq!(occ.len(), 1);

    // Straddling (interpenetration): does NOT hide — cutout tiles may
    // show a face through the straddling box (the chain's crossing
    // plates), so only sealed flush contact culls.
    occ.clear();
    push_occluder(
        &mut occ,
        [0.25, 0.0, 0.0],
        [0.75, 1.0, 1.0],
        axes,
        true,
        0.5,
        false,
        &rect_full,
    );
    assert!(occ.is_empty());
}

/// For a full-cell box the corner probes leave the cell on every axis,
/// so self-AO reduces exactly to the grid ring: an empty ring keeps AO
/// at 3, a diagonal-only occluder gives 2, two sides give the buried 0.
#[test]
fn full_cell_probe_reduces_to_grid_vertex_ao() {
    let boxes = [plain([0.0; 3], [1.0; 3])];
    let axes = face_axes(Face::PosY);
    let r = rect(0.0, 0.0, 1.0, 1.0);
    // Corner (0, 1, 0): side cells (-1,1,0) and (0,1,-1), diag (-1,1,-1).
    let corner = [0.0, 1.0, 0.0];
    let at = (0, 0, 0);
    let ao_none = probe_ao(&boxes, corner, axes, true, &r, at, &|_, _, _| false);
    assert_eq!(ao_none, 3);
    let ao_diag = probe_ao(&boxes, corner, axes, true, &r, at, &|cl, _, _| {
        cl == (-1, 1, -1)
    });
    assert_eq!(ao_diag, 2);
    let ao_sides = probe_ao(&boxes, corner, axes, true, &r, at, &|cl, _, _| {
        cl.1 == 1 && (cl.0 == -1) != (cl.2 == -1)
    });
    assert_eq!(ao_sides, 0, "two solid sides bury the corner");
}

/// An inner crease darkens: the corner of a floor face that meets a wall
/// box probes into the wall. And a coplanar continuation does NOT: two
/// flush boxes forming one surface leave the shared seam at AO 3 (the
/// lifted probe passes above both).
#[test]
fn probes_darken_creases_but_not_continuations() {
    // Floor + wall along the u-min edge (a stair silhouette).
    let stair = [
        plain([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]),
        plain([0.0, 0.5, 0.0], [0.5, 1.0, 1.0]),
    ];
    let axes = face_axes(Face::PosY);
    // The tread: the floor box's +Y face right of the riser.
    let tread = rect(0.5, 0.0, 1.0, 1.0); // u = X in [0.5, 1], v = Z
    let crease = probe_ao(
        &stair,
        [0.5, 0.5, 0.5],
        axes,
        true,
        &tread,
        (0, 0, 0),
        &|_, _, _| false,
    );
    assert!(crease < 3, "tread corner against the riser must darken");

    // Two half boxes forming one flat top: the shared seam stays open.
    let flat = [
        plain([0.0, 0.0, 0.0], [0.5, 1.0, 1.0]),
        plain([0.5, 0.0, 0.0], [1.0, 1.0, 1.0]),
    ];
    let left_top = rect(0.0, 0.0, 0.5, 1.0);
    let seam = probe_ao(
        &flat,
        [0.5, 1.0, 0.5],
        axes,
        true,
        &left_top,
        (0, 0, 0),
        &|_, _, _| false,
    );
    assert_eq!(seam, 3, "coplanar continuation must not self-shadow");
}
/// A posed plane is emitted where its pose puts it — the authored corners
/// carried through the rotation — with the back winding when
/// double-sided, and it takes no part in the axis-aligned rules: a posed
/// box that would span a boundary if it were flat seals nothing, and a
/// posed sibling hides no axis face.
#[test]
fn a_posed_plane_lands_on_its_rotated_corners_and_seals_nothing() {
    let t = petramond_world::tile::Tile::named("stone");
    let pose = petramond_world::block::BoxPose::from_euler_degrees([45.0, 0.0, 0.0], [0.5; 3]);
    let mut slope = ShapeBox::uniform(
        petramond_world::block::Aabb {
            min: [0.0, 0.5, -0.2],
            max: [1.0, 0.5, 1.2],
        },
        [t, t, t],
        |_| [1.0; 3],
    )
    .double_sided()
    .as_face_carrier();
    slope.pose = Some(pose);
    for (i, f) in slope.faces.iter_mut().enumerate() {
        if i != 2 {
            *f = None;
        }
    }
    let boxes = [slope, plain([0.0, 0.0, 0.0], [1.0, 0.25, 1.0])];

    let mut vbuf = Vec::new();
    let mut scratch = BoxSetScratch::default();
    emit_box_set(
        &mut vbuf,
        0,
        0,
        0,
        glam::IVec3::ZERO,
        &boxes,
        &mut scratch,
        &|_| false,
        &|_, _| {},
        &|_, _, _| false,
        &|_, _, _| petramond_world::block::Block::Air,
        &|_, _, _| None,
        &|_, _, _| 63,
        &|_, _, _| petramond_world::light::LightRgb::ZERO,
    );
    // The slope's +Y face: front + back windings, 8 vertices, every
    // corner on the tilted plane (y = 1 - z).
    let slope_verts: Vec<&Vertex> = vbuf
        .iter()
        .filter(|v| v.pos[1] > 0.001 && (v.pos[1] - (1.0 - v.pos[2])).abs() < 1e-3)
        .collect();
    assert_eq!(
        slope_verts.len(),
        8,
        "front + back of one posed quad: {vbuf:?}"
    );
    assert!(slope_verts
        .iter()
        .any(|v| v.pos[1] > 0.99 && v.pos[2] < 0.01));
    assert!(slope_verts
        .iter()
        .any(|v| v.pos[1] < 0.01 && v.pos[2] > 0.99));
    // The flat box below keeps its whole top face: the posed plane
    // crossing it is not an axis-aligned occluder.
    let top = vbuf
        .chunks_exact(4)
        .filter(|q| q.iter().all(|v| (v.pos[1] - 0.25).abs() < 1e-4))
        .count();
    assert_eq!(top, 1, "the flat box's top is drawn whole");

    // And a posed full-cell box seals no boundary.
    let mut posed_cube = plain([0.0; 3], [1.0; 3]);
    posed_cube.pose = Some(pose);
    assert!(!covers_boundary(&[posed_cube], Face::NegY, &mut scratch));
    assert!(covers_boundary(
        &[plain([0.0; 3], [1.0; 3])],
        Face::NegY,
        &mut scratch
    ));
}
