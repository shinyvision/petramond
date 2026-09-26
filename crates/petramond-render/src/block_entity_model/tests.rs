use super::*;
use petramond_math::facing::Facing;
use petramond_world::door::THICKNESS;

const FACINGS: [Facing; 4] = [Facing::North, Facing::South, Facing::West, Facing::East];

fn inst(block: Block, facing: Facing, variant: u8, open01: f32) -> BlockEntityInstance {
    BlockEntityInstance {
        pos: glam::IVec3::new(10, 64, -5),
        block,
        facing,
        variant,
        open01,
        skylight: super::super::lighting::FULL_SKYLIGHT,
        blocklight: petramond_world::light::BlockLight6::DARK,
    }
}

fn bake(instances: &[BlockEntityInstance]) -> (Vec<Vertex>, Vec<u32>) {
    let (mut v, mut i) = (Vec::new(), Vec::new());
    push_block_entities(instances, glam::IVec3::ZERO, &mut v, &mut i);
    (v, i)
}

fn span(v: &[Vertex], axis: usize) -> (f32, f32) {
    v.iter().fold((f32::MAX, f32::MIN), |(lo, hi), vert| {
        (lo.min(vert.pos[axis]), hi.max(vert.pos[axis]))
    })
}

fn extent(v: &[Vertex], axis: usize) -> f32 {
    let (lo, hi) = span(v, axis);
    hi - lo
}

#[test]
fn each_model_bakes_one_box_per_part() {
    for (block, boxes) in [
        (Block::Chest, 3),
        (Block::OakDoor, 2),
        (Block::OakTrapdoor, 1),
    ] {
        let (v, i) = bake(&[inst(block, Facing::North, 0, 0.0)]);
        assert_eq!(v.len(), boxes * 24, "{block:?}: 24 verts per box");
        assert_eq!(i.len(), boxes * 36, "{block:?}: 36 indices per box");
    }
}

#[test]
fn empty_input_and_a_modelless_block_produce_no_geometry() {
    assert!(bake(&[]).0.is_empty());
    assert!(bake(&[inst(Block::Stone, Facing::North, 0, 0.0)])
        .0
        .is_empty());
}

#[test]
fn a_closed_chest_fits_its_cell_and_its_lid_rises_when_opening() {
    let (closed, _) = bake(&[inst(Block::Chest, Facing::North, 0, 0.0)]);
    for vert in &closed {
        let [x, y, z] = vert.pos;
        assert!((10.0..=11.0).contains(&x), "x within cell, got {x}");
        assert!((-5.0..=-4.0).contains(&z), "z within cell, got {z}");
        assert!((64.0..=65.0).contains(&y), "y within cell, got {y}");
    }
    let (open, _) = bake(&[inst(Block::Chest, Facing::North, 0, 1.0)]);
    let (_, closed_top) = span(&closed, 1);
    let (_, open_top) = span(&open, 1);
    assert!(
        open_top > closed_top + 0.3,
        "the lid should rise when opening: {closed_top} -> {open_top}"
    );
}

#[test]
fn a_closed_door_spans_two_cells_tall_on_its_edge() {
    let (v, _) = bake(&[inst(Block::OakDoor, Facing::South, 0, 0.0)]);
    for vert in &v {
        assert!((10.0..=11.0).contains(&vert.pos[0]));
        assert!((-5.0..=-4.0).contains(&vert.pos[2]));
    }
    let (ymin, ymax) = span(&v, 1);
    assert!((ymin - 64.0).abs() < 1e-4, "rests on the cell floor");
    assert!((ymax - 66.0).abs() < 1e-4, "two cells tall");
}

#[test]
fn thin_edge_faces_carry_a_uv_slice_mode() {
    // The 3/16-deep edge faces crop their tile (packed bits 29..32) so the
    // plank side isn't a whole tile squished flat; the wide art is full-tile.
    // Faces are emitted per box in ALL_FACES order, 4 verts each.
    let (v, _) = bake(&[inst(Block::OakDoor, Facing::South, 0, 0.0)]);
    let slice = |face: usize| (v[face * 4].packed >> petramond_mesh::UV_MODE_SHIFT) & 0x3;
    assert_eq!(slice(0), 1, "PosX side edge crops U");
    assert_eq!(slice(1), 1, "NegX side edge crops U");
    assert_eq!(slice(2), 2, "PosY top edge crops V");
    assert_eq!(slice(3), 2, "NegY bottom edge crops V");
    assert_eq!(slice(4), 0, "PosZ front art is full-tile");
    assert_eq!(slice(5), 0, "NegZ back art is full-tile");
    // A part that does not declare thin edges keeps every tile whole.
    let (chest, _) = bake(&[inst(Block::Chest, Facing::South, 0, 0.0)]);
    assert!(chest
        .iter()
        .all(|v| (v.packed >> petramond_mesh::UV_MODE_SHIFT) & 0x3 == 0));
}

#[test]
fn opening_swings_a_door_onto_the_perpendicular_edge() {
    let (closed, _) = bake(&[inst(Block::OakDoor, Facing::South, 0, 0.0)]);
    let (open, _) = bake(&[inst(Block::OakDoor, Facing::South, 0, 1.0)]);
    assert!(extent(&closed, 0) > 0.9 && extent(&closed, 2) < 0.3);
    assert!(extent(&open, 0) < 0.3 && extent(&open, 2) > 0.9);
}

#[test]
fn opening_lifts_a_trapdoor_off_the_floor_onto_an_edge() {
    let closed = bake(&[inst(Block::OakTrapdoor, Facing::South, 0, 0.0)]).0;
    let open = bake(&[inst(Block::OakTrapdoor, Facing::South, 0, 1.0)]).0;
    assert!(extent(&closed, 1) < 0.25, "closed: thin on Y");
    assert!(extent(&open, 1) > 0.9, "open: full height");
    assert!(extent(&open, 2) < 0.25, "open: thin on Z (on its edge)");
}

#[test]
fn a_swung_panel_keeps_to_its_own_cell() {
    // The point of the inset hinge: the RESTING poses lie exactly in the
    // cell, and mid-swing the corner nearest the hinge sweeps only the tiny
    // arc a rigid rotation about an inset pivot must — bounded by
    // (√2 - 1)·T/2, reached at 45°.
    let arc = (std::f32::consts::SQRT_2 - 1.0) * THICKNESS / 2.0 + 1e-4;
    for facing in FACINGS {
        for variant in [0, 1] {
            for step in 0..=4 {
                let open01 = step as f32 / 4.0;
                let slack = if step == 0 || step == 4 { 1e-4 } else { arc };
                let (v, _) = bake(&[inst(Block::OakTrapdoor, facing, variant, open01)]);
                for (axis, origin) in [(0, 10.0), (1, 64.0), (2, -5.0)] {
                    let (lo, hi) = span(&v, axis);
                    assert!(
                        lo >= origin - slack && hi <= origin + 1.0 + slack,
                        "{facing:?} variant={variant} open={step}/4: axis {axis} {lo}..{hi} \
                         escaped the cell at {origin}"
                    );
                }
            }
        }
    }
}

#[test]
fn the_chest_item_is_the_closed_model_centred_in_its_cube() {
    let model = item_model(Block::Chest).expect("the chest's item draws its model");
    assert!(
        item_model(Block::OakDoor).is_none(),
        "a door's item is a sprite"
    );
    let (mut v, mut i) = (Vec::new(), Vec::new());
    push_item(
        &mut v,
        &mut i,
        model,
        Block::Chest,
        Vec3::splat(-0.5),
        1.0,
        DynLight::FULL,
    );
    assert_eq!(v.len(), 3 * 24);
    let (lo, hi) = span(&v, 1);
    assert!((lo + hi).abs() < 1e-5, "centred vertically: {lo}..{hi}");
}
