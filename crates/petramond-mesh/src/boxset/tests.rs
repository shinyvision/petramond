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

#[test]
fn subtraction_conserves_uncovered_area() {
    let face = rect(0.0, 0.0, 1.0, 1.0);
    assert!(run_subtract(face, &[face]).is_empty(), "full burial");

    let out = run_subtract(face, &[rect(0.25, 0.25, 0.75, 0.75)]);
    assert!((area(&out) - 0.75).abs() < 1e-5, "area {:?}", out);
    for (i, a) in out.iter().enumerate() {
        for b in out.iter().skip(i + 1) {
            let w = (a.u1.min(b.u1) - a.u0.max(b.u0)).max(0.0);
            let h = (a.v1.min(b.v1) - a.v0.max(b.v0)).max(0.0);
            assert!(w * h < 1e-6, "overlap {a:?} {b:?}");
        }
    }
}

#[test]
fn subtraction_remerges_bands() {
    let face = rect(0.0, 0.0, 1.0, 1.0);
    assert_eq!(run_subtract(face, &[]), vec![face]);

    let out = run_subtract(face, &[rect(0.0, 0.4, 1.0, 0.6)]);
    assert_eq!(out.len(), 2, "{out:?}");
    assert!((area(&out) - 0.8).abs() < 1e-5);
}

struct OpenAir;

impl BoxWorld for OpenAir {
    fn neighbour_solid(&self, _: Face) -> bool {
        false
    }
    fn neighbour_boxes(&self, _: Face, _: &mut Vec<([f32; 3], [f32; 3])>) {}
    fn matter(&self, _: IVec3, _: [f32; 3], _: [f32; 3]) -> bool {
        false
    }
    fn face_light(&self, _: Face, _: IVec3, _: f32, _: bool) -> CornerLight {
        (
            [3; 4],
            [63; 4],
            [petramond_world::light::BlockLight6::DARK; 4],
        )
    }
}

fn plain(min: [f32; 3], max: [f32; 3]) -> ShapeBox {
    let t = petramond_world::tile::Tile::named("stone");
    ShapeBox::uniform(petramond_world::block::Aabb { min, max }, [t, t, t], |_| {
        [1.0, 1.0, 1.0]
    })
}

#[test]
fn occluder_rule_covers_butting_and_coincidence() {
    let plus_x_at_half = FacePlane {
        axes: (0, 2, 1),
        positive: true,
        d: 0.5,
    };
    let rect_full = rect(0.0, 0.0, 1.0, 1.0);
    let mut occ = vec![];

    push_occluder(
        &mut occ,
        ([0.5, 0.0, 0.0], [1.0, 1.0, 1.0]),
        plus_x_at_half,
        false,
        &rect_full,
    );
    assert_eq!(occ.len(), 1);

    occ.clear();
    push_occluder(
        &mut occ,
        ([0.0, 0.0, 0.0], [0.5, 1.0, 1.0]),
        plus_x_at_half,
        false,
        &rect_full,
    );
    assert!(occ.is_empty());

    push_occluder(
        &mut occ,
        ([0.0, 0.0, 0.0], [0.5, 1.0, 1.0]),
        plus_x_at_half,
        true,
        &rect_full,
    );
    assert_eq!(occ.len(), 1);

    occ.clear();
    push_occluder(
        &mut occ,
        ([0.25, 0.0, 0.0], [0.75, 1.0, 1.0]),
        plus_x_at_half,
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
    let top = FacePlane {
        axes: face_axes(Face::PosY),
        positive: true,
        d: 1.0,
    };
    let r = rect(0.0, 0.0, 1.0, 1.0);
    let corner = [0.0, 1.0, 0.0];
    let at = IVec3::ZERO;
    let ao_none = probe_ao(&boxes, corner, top, &r, at, &|_, _, _| false);
    assert_eq!(ao_none, 3);
    let ao_diag = probe_ao(&boxes, corner, top, &r, at, &|cl, _, _| {
        cl == IVec3::new(-1, 1, -1)
    });
    assert_eq!(ao_diag, 2);
    let ao_sides = probe_ao(&boxes, corner, top, &r, at, &|cl, _, _| {
        cl.y == 1 && (cl.x == -1) != (cl.z == -1)
    });
    assert_eq!(ao_sides, 0, "two solid sides bury the corner");
}

#[test]
fn probes_darken_creases_but_not_continuations() {
    let stair = [
        plain([0.0, 0.0, 0.0], [1.0, 0.5, 1.0]),
        plain([0.0, 0.5, 0.0], [0.5, 1.0, 1.0]),
    ];
    let plane_at = |d: f32| FacePlane {
        axes: face_axes(Face::PosY),
        positive: true,
        d,
    };
    let tread = rect(0.5, 0.0, 1.0, 1.0);
    let crease = probe_ao(
        &stair,
        [0.5, 0.5, 0.5],
        plane_at(0.5),
        &tread,
        IVec3::ZERO,
        &|_, _, _| false,
    );
    assert!(crease < 3, "tread corner against the riser must darken");

    let flat = [
        plain([0.0, 0.0, 0.0], [0.5, 1.0, 1.0]),
        plain([0.5, 0.0, 0.0], [1.0, 1.0, 1.0]),
    ];
    let left_top = rect(0.0, 0.0, 0.5, 1.0);
    let seam = probe_ao(
        &flat,
        [0.5, 1.0, 0.5],
        plane_at(1.0),
        &left_top,
        IVec3::ZERO,
        &|_, _, _| false,
    );
    assert_eq!(seam, 3, "coplanar continuation must not self-shadow");
}
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
        BoxCell {
            cell: IVec3::ZERO,
            anchor: IVec3::ZERO,
        },
        &boxes,
        &mut scratch,
        &OpenAir,
    );
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
    let top = vbuf
        .chunks_exact(4)
        .filter(|q| q.iter().all(|v| (v.pos[1] - 0.25).abs() < 1e-4))
        .count();
    assert_eq!(top, 1, "the flat box's top is drawn whole");

    let mut posed_cube = plain([0.0; 3], [1.0; 3]);
    posed_cube.pose = Some(pose);
    assert!(!covers_boundary(&[posed_cube], Face::NegY, &mut scratch));
    assert!(covers_boundary(
        &[plain([0.0; 3], [1.0; 3])],
        Face::NegY,
        &mut scratch
    ));
}
