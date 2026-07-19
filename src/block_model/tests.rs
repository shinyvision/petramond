/// Test helper: a kind's .bbmodel source read through the asset roots.
fn model_bytes(kind: BlockModelKind) -> Vec<u8> {
    let file = def(kind).model_file;
    crate::assets::read_bytes(file)
        .unwrap_or_else(|| panic!("bbmodel '{file}' not found"))
        .0
}

use glam::Vec3;

use crate::asset_cache::CompiledAsset;
use crate::block::Aabb;
use crate::mesh::face::Face;

use super::geometry::{face_slot, FLAT_FACE_BIAS};
use super::*;

const WB: BlockModelKind = BlockModelKind::FurnitureWorkbench;

#[test]
fn workbench_compiles_with_geometry_and_texture() {
    let m = BlockModel::compile(&model_bytes(WB)).expect("compiles");
    assert!(!m.cubes.is_empty());
    assert_eq!((m.tex_w, m.tex_h), (128, 128));
    assert_eq!(m.texture_rgba.len(), 128 * 128 * 4);
}

#[test]
fn every_registered_model_compiles_with_geometry_and_texture() {
    // A bad bbmodel export degrades to an EMPTY model at runtime (log +
    // invisible), so a compile failure must be caught here instead.
    for &kind in all() {
        let m = BlockModel::compile(&model_bytes(kind))
            .unwrap_or_else(|e| panic!("{kind:?} fails to compile: {e}"));
        assert!(!m.cubes.is_empty(), "{kind:?} has no geometry");
        assert_eq!(
            m.texture_rgba.len(),
            (m.tex_w * m.tex_h * 4) as usize,
            "{kind:?} texture size mismatch"
        );
        assert!(m.tex_w > 0 && m.tex_h > 0, "{kind:?} has no texture");
    }
}

#[test]
fn footprint_is_two_by_two_by_one() {
    assert_eq!(footprint(WB), [2, 2, 1], "authored 2 wide, 2 tall, 1 long");
}

#[test]
fn flat_model_cubes_emit_one_biased_surface_face() {
    let cube = ModelCube {
        name: String::new(),
        from: Vec3::new(0.0, 0.5, 0.0),
        to: Vec3::new(1.0, 0.5, 1.0),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let support = ModelCube {
        name: String::new(),
        from: Vec3::ZERO,
        to: Vec3::new(1.0, 0.5, 1.0),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let all = [cube, support];
    assert_eq!(
        render_face_bias(&all[0], &all, Face::PosY),
        Some(Vec3::Y * FLAT_FACE_BIAS)
    );
    assert_eq!(render_face_bias(&all[0], &all, Face::NegY), None);
    assert_eq!(render_face_bias(&all[0], &all, Face::PosX), None);
    assert_eq!(render_face_bias(&all[0], &all, Face::PosZ), None);
}

#[test]
fn flat_model_cubes_bias_away_from_backing_surface() {
    let poster = ModelCube {
        name: String::new(),
        from: Vec3::new(0.0, 0.0, 0.5),
        to: Vec3::new(1.0, 1.0, 0.5),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let backing = ModelCube {
        name: String::new(),
        from: Vec3::new(0.0, 0.0, 0.5),
        to: Vec3::new(1.0, 1.0, 0.75),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let all = [poster, backing];
    assert_eq!(
        render_face_bias(&all[0], &all, Face::NegZ),
        Some(Vec3::NEG_Z * FLAT_FACE_BIAS)
    );
    assert_eq!(render_face_bias(&all[0], &all, Face::PosZ), None);
}

#[test]
fn unsupported_flat_model_cubes_fall_back_to_authored_positive_face() {
    let mut cube = ModelCube {
        name: String::new(),
        from: Vec3::new(0.0, 0.5, 0.0),
        to: Vec3::new(1.0, 0.5, 1.0),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let all = [cube.clone()];
    assert_eq!(
        render_face_bias(&cube, &all, Face::PosY),
        Some(Vec3::Y * FLAT_FACE_BIAS)
    );
    cube.faces[face_slot(Face::PosY)] = None;
    let all = [cube.clone()];
    assert_eq!(
        render_face_bias(&cube, &all, Face::NegY),
        Some(Vec3::NEG_Y * FLAT_FACE_BIAS)
    );
}

#[test]
fn thick_model_cubes_emit_all_faces_without_bias() {
    let cube = ModelCube {
        name: String::new(),
        from: Vec3::ZERO,
        to: Vec3::ONE,
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };

    for face in Face::ALL {
        assert_eq!(
            render_face_bias(&cube, std::slice::from_ref(&cube), face),
            Some(Vec3::ZERO)
        );
    }
}

#[test]
fn every_footprint_cell_is_covered_and_splits_the_cubes() {
    let inst = instance(WB);
    // Each cube is assigned to exactly one cell (the split partitions geometry).
    let total: usize = inst.cells.iter().map(|c| c.cubes.len()).sum();
    assert_eq!(
        total,
        inst.cubes.len(),
        "every cube assigned to exactly one cell"
    );
    // The lower cells (resting on the floor, full Z) are present and collide.
    for off in [[0, 0, 0], [1, 0, 0]] {
        let c = inst.cell(off).expect("floor cell present");
        assert!(!c.collision.is_empty(), "floor cell {off:?} collides");
    }
}

#[test]
fn cells_are_local_and_within_unit_bounds() {
    let inst = instance(WB);
    for c in &inst.cells {
        for b in &c.collision {
            for i in 0..3 {
                assert!(
                    b.min[i] >= -1e-3 && b.max[i] <= 1.0 + 1e-3,
                    "cell-local box"
                );
                assert!(b.max[i] > b.min[i]);
            }
        }
    }
}

#[test]
fn footprint_geometry_fits_the_cell_box() {
    let inst = instance(WB);
    let (mn, mx) = (inst.bounds_min, inst.bounds_max);
    assert!(mn[0] >= -1e-3 && mx[0] <= 2.0 + 1e-3, "X within 2 cells");
    assert!(mn[1] >= -1e-3 && mx[1] <= 2.0 + 1e-3, "Y within 2 cells");
    assert!(mn[2] >= -1e-3 && mx[2] <= 1.0 + 1e-3, "Z within 1 cell");
}

#[test]
fn collision_is_the_multi_box_model_shape_not_one_coarse_box() {
    // The fix: collision follows the actual cubes (several boxes per cell), so the
    // workbench isn't one solid 2×2×1 block. The bottom cells (legs + body + top) get
    // many boxes; the outline is the whole model's tight box across all cells.
    let inst = instance(WB);
    let floor = inst.cell([0, 0, 0]).expect("floor cell");
    assert!(
        floor.collision.len() > 1,
        "collision is multiple cube boxes, not one"
    );
    // Outline spans the whole 2×2×1 footprint (one box hugging the model).
    assert!(
        inst.bounds_max[0] - inst.bounds_min[0] > 1.5,
        "outline spans ~2 cells wide"
    );
    assert!(
        inst.bounds_max[1] - inst.bounds_min[1] > 1.0,
        "outline spans >1 cell tall"
    );
}

#[test]
fn collision_hidden_parts_keep_visuals_but_remove_collision() {
    // A solid cube plus a decorative "water" cube that should render and
    // contribute to bounds/selection but not to player collision.
    let solid = ModelCube {
        name: "solid".into(),
        from: Vec3::new(0.0, 0.0, 0.0),
        to: Vec3::new(16.0, 16.0, 16.0),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let water = ModelCube {
        name: "water".into(),
        from: Vec3::new(2.0, 2.0, 2.0),
        to: Vec3::new(14.0, 14.0, 14.0),
        origin: Vec3::ZERO,
        rotation: Vec3::ZERO,
        faces: [Some([0.0, 0.0, 1.0, 1.0]); 6],
    };
    let mut model = BlockModel {
        cubes: vec![solid, water],
        texture_rgba: vec![0, 0, 0, 0],
        tex_w: 1,
        tex_h: 1,
        collision: Vec::new(),
        bounds: Aabb {
            min: [0.0; 3],
            max: [1.0; 3],
        },
        display: BlockDisplay::default(),
        display_pivot: [0.0, 8.0, 0.0],
    };
    model.rebake();
    assert_eq!(model.collision.len(), 2, "both cubes collide before hiding");
    let bounds_before = model.bounds;

    model.hide_collision_parts(&["water"], &[], "test:trough_filled");
    assert_eq!(
        model.collision.len(),
        1,
        "only the solid cube collides after hiding water"
    );
    assert_eq!(
        model.bounds, bounds_before,
        "bounds still hug the visible water"
    );
    assert_eq!(model.cubes.len(), 2, "water cube is still rendered");
}

#[test]
fn display_poses_are_parsed_and_cached() {
    // The workbench authors a full `display` block; the compile must capture the gui +
    // first-person poses (so the icon/held item pose as designed) rather than identity.
    let m = BlockModel::compile(&model_bytes(WB)).expect("compiles");
    let gui = m.display.gui;
    let fp = m.display.firstperson_righthand;
    // Non-identity rotations were authored for both contexts.
    assert_ne!(gui.rotation, [0.0; 3], "gui pose has an authored rotation");
    assert_ne!(
        fp.rotation, [0.0; 3],
        "first-person pose has an authored rotation"
    );
    // The cached accessor returns the same parsed data.
    assert_eq!(display(WB).gui, gui);
    // A finite pose matrix is produced for posing.
    assert!(fp
        .base_matrix()
        .to_cols_array()
        .iter()
        .all(|f| f.is_finite()));
}

/// The display euler must compose exactly as Blockbench/three.js 'XYZ' does
/// (matrix `Rx·Ry·Rz`) — the convention the in-hand pose replication depends on.
/// Single-axis mappings pin each axis's direction; the composed case pins the order.
#[test]
fn display_base_matrix_matches_blockbench_euler_convention() {
    let with_rot = |r: [f32; 3]| DisplayTransform {
        rotation: r,
        ..Default::default()
    };
    let close = |a: Vec3, b: Vec3| (a - b).length() < 1e-5;
    // Ry(+90°): +X → −Z (yaw left, as in Blockbench's preview).
    let m = with_rot([0.0, 90.0, 0.0]).base_matrix();
    assert!(close(m.transform_vector3(Vec3::X), -Vec3::Z));
    // Rx(+90°): +Y → +Z (pitch toward the viewer's side).
    let m = with_rot([90.0, 0.0, 0.0]).base_matrix();
    assert!(close(m.transform_vector3(Vec3::Y), Vec3::Z));
    // Order Rx·Ry: +X goes through Ry first (→ −Z), then Rx (→ +Y).
    let m = with_rot([90.0, 90.0, 0.0]).base_matrix();
    assert!(close(m.transform_vector3(Vec3::X), Vec3::Y));
}

/// With a `rotation_pivot` authored, the pose must rotate ABOUT that point: the
/// pivot itself only moves by the authored translation. Pins the Blockbench
/// position-correction algorithm (`pos -= R·piv − piv`).
#[test]
fn display_base_matrix_rotates_about_the_authored_pivot() {
    let piv = Vec3::new(0.25, -0.5, 0.125);
    let t = DisplayTransform {
        rotation: [16.0, 14.0, 4.0],
        translation: [1.0, 2.0, 3.0],
        rotation_pivot: piv.to_array(),
        ..Default::default()
    };
    let moved = t.base_matrix().transform_point3(piv);
    let expected = piv + Vec3::new(1.0, 2.0, 3.0) / 16.0;
    assert!(
        (moved - expected).length() < 1e-5,
        "pivot must stay fixed under rotation (moved to {moved:?}, expected {expected:?})"
    );
}

/// `display_from_unit` must be a POSITIVE uniform rescale + translation — no
/// rotation, no mirrored axis. Any flip smuggled in here (the historical 180°-yaw /
/// mirrored-euler hand bugs) would silently mis-pose every held model again.
#[test]
fn display_from_unit_is_an_unmirrored_uniform_rescale() {
    let m = instance(WB).display_from_unit;
    let (x, y, z) = (
        m.transform_vector3(Vec3::X),
        m.transform_vector3(Vec3::Y),
        m.transform_vector3(Vec3::Z),
    );
    for (v, axis) in [(x, Vec3::X), (y, Vec3::Y), (z, Vec3::Z)] {
        let k = v.dot(axis);
        assert!(k > 0.0, "axis {axis:?} must keep its direction, got {v:?}");
        assert!(
            (v - axis * k).length() < 1e-6,
            "axis {axis:?} must not rotate, got {v:?}"
        );
    }
    assert!(
        (x.length() - y.length()).abs() < 1e-6 && (y.length() - z.length()).abs() < 1e-6,
        "rescale must be uniform"
    );
}

/// A real furniture model (legs meeting a top) bakes some self-occlusion, and
/// every factor stays a valid shade multiplier — the invariant is "joint
/// definition exists and can never invert or black out a face"; the darkening
/// depth itself is a tuned constant and is not pinned.
#[test]
fn baked_self_ao_darkens_joints_and_stays_a_valid_multiplier() {
    let inst = instance(WB);
    let mut any_darkened = false;
    for per_face in &inst.face_ao {
        for corners in per_face {
            for &v in corners {
                assert!(
                    (0.0..=1.0 + 1e-6).contains(&v),
                    "AO factor out of the multiplier range: {v}"
                );
                any_darkened |= v < 1.0 - 1e-3;
            }
        }
    }
    assert!(
        any_darkened,
        "a model with adjoining cuboids must bake SOME self-occlusion"
    );
}
