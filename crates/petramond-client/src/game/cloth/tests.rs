use super::*;
use petramond_world::tile::Tile;
use petramond_world::verlet::closest;

const FLOOR: Aabb = Aabb {
    min: [0.0; 3],
    max: [1.0; 3],
};

fn flag() -> &'static ClothDef {
    Box::leak(Box::new(ClothDef {
        id: 0,
        key: "test:flag",
        tile: Tile::from_name("grass_top").unwrap(),
        uv: [0.0, 0.0, 1.0, 1.0],
        size: [1.5, 1.0],
        segments: [12, 8],
        anchor: [0.5, 1.0, 0.5],
        stiffness: 0.9,
        damping: 0.985,
        wind: 1.0,
        gravity: 1.0,
    }))
}

fn open_air(_: IVec3) -> &'static [Aabb] {
    &[]
}

fn run(
    sim: &mut ClothSim,
    seconds: f32,
    wind: [f32; 2],
    boxes: &impl Fn(IVec3) -> &'static [Aabb],
    bodies: &[(Vec3, f32, f32)],
) {
    let steps = (seconds / STEP) as usize;
    for n in 0..steps {
        sim.step(n as f32 * STEP, wind, boxes, bodies);
    }
}

fn free_edge(sim: &ClothSim) -> Vec<Vec3> {
    let cols = sim.cols();
    sim.points(1.0).skip(cols - 1).step_by(cols).collect()
}

#[test]
fn the_pinned_edge_stays_on_the_post_whatever_the_wind() {
    let def = flag();
    let mut sim = ClothSim::new(IVec3::ZERO, def, DEFAULT_WIND, 0.0);
    run(&mut sim, 4.0, [3.0, 2.0], &open_air, &[]);
    let cols = sim.cols();
    for (row, p) in sim.points(1.0).step_by(cols).enumerate() {
        let want = Vec3::new(0.5, 1.0 - row as f32 * 0.125, 0.5);
        assert!((p - want).length() < 1e-5, "row {row}: {p} vs {want}");
    }
}

#[test]
fn wind_streams_the_cloth_downwind_and_calm_lets_it_hang() {
    let def = flag();
    let settle = |wind: [f32; 2]| {
        let mut sim = ClothSim::new(IVec3::ZERO, def, [0.0, 1.0], 0.0);
        run(&mut sim, 6.0, wind, &open_air, &[]);
        let edge = free_edge(&sim);
        edge.iter().sum::<Vec3>() / edge.len() as f32
    };
    let windy = settle(DEFAULT_WIND);
    let calm = settle([0.0, 0.0]);
    assert!(
        windy.x < 0.5 - 1.0,
        "the free edge flies downwind (-X): {windy}"
    );
    assert!(
        calm.x > windy.x + 0.8,
        "calm air does not carry it: {calm} vs {windy}"
    );
    assert!(
        calm.y < windy.y - 0.3,
        "a calm cloth droops below a flying one: {calm} vs {windy}"
    );
}

#[test]
fn a_wall_downwind_keeps_every_point_out_of_it() {
    let def = flag();
    let wall = |c: IVec3| -> &'static [Aabb] {
        if c.x == -1 {
            std::slice::from_ref(&FLOOR)
        } else {
            &[]
        }
    };
    let mut sim = ClothSim::new(IVec3::ZERO, def, [0.0, 1.0], 0.0);
    run(&mut sim, 6.0, [-6.0, 0.0], &wall, &[]);
    for p in sim.points(1.0) {
        assert!(p.x >= 0.0 - 1e-3, "point inside the wall: {p}");
    }
}

#[test]
fn a_body_standing_in_the_cloth_pushes_it_aside() {
    let def = flag();
    let mut sim = ClothSim::new(IVec3::ZERO, def, DEFAULT_WIND, 0.0);
    let body = (Vec3::new(-0.3, -1.5, 0.5), 1.8, 0.35);
    run(&mut sim, 4.0, DEFAULT_WIND, &open_air, &[body]);
    for p in sim.points(1.0) {
        let axis = Vec3::new(body.0.x, p.y, body.0.z);
        if p.y > body.0.y && p.y < body.0.y + body.1 {
            assert!(
                (p - axis).length() >= body.2 - 1e-3,
                "point inside the body: {p}"
            );
        }
    }
}

#[test]
fn no_triangle_ever_cuts_a_block_however_the_wind_swings_it() {
    // The flag flaps over the edges and corners of a little block step beside its post:
    // the case where every vertex can stay outside while a triangle crosses a corner.
    let def = flag();
    let solid = |c: IVec3| {
        matches!(
            (c.x, c.y, c.z),
            (-1, -1, 0) | (-1, -1, 1) | (0, -1, 1) | (-1, 0, -1)
        )
    };
    let boxes = move |c: IVec3| -> &'static [Aabb] {
        if solid(c) {
            std::slice::from_ref(&FLOOR)
        } else {
            &[]
        }
    };
    let mut sim = ClothSim::new(IVec3::ZERO, def, [0.0, -1.0], 0.0);
    let tris = sim.tris.clone();
    for n in 0..400 {
        let t = n as f32 * STEP;
        let angle = t * 1.3;
        let speed = 2.0 + 2.0 * (t * 0.7).sin();
        sim.step(t, [angle.cos() * speed, angle.sin() * speed], &boxes, &[]);
        for &[a, b, c] in &tris {
            for x in -1..=0 {
                for y in -1..=0 {
                    for z in -1..=1 {
                        if !solid(IVec3::new(x, y, z)) {
                            continue;
                        }
                        let min = Vec3::new(x as f32, y as f32, z as f32);
                        let d = closest::triangle_box_distance(
                            sim.pos[a],
                            sim.pos[b],
                            sim.pos[c],
                            min,
                            min + Vec3::ONE,
                        );
                        assert!(
                            d > 0.0,
                            "step {n}: triangle {a},{b},{c} cuts block {x},{y},{z}"
                        );
                    }
                }
            }
        }
        assert_eq!(
            sim.furls, 0,
            "step {n}: the sheet cut a block and had to furl"
        );
    }
}

#[test]
fn the_far_pose_matches_where_the_sim_settles_so_the_handover_does_not_pop() {
    let def = flag();
    for wind in [[-0.75, 0.0], DEFAULT_WIND, [0.0, 3.0]] {
        let mut sim = ClothSim::new(IVec3::ZERO, def, wind, 0.0);
        run(&mut sim, 6.0, wind, &open_air, &[]);
        let settled = free_edge(&sim).iter().sum::<Vec3>() / sim.rows() as f32;
        let mut pose = Vec::new();
        let segments = def.segments.map(usize::from);
        far_pose(def, IVec3::ZERO, wind, 6.0, segments, &mut pose);
        let cols = segments[0] + 1;
        let posed = pose.iter().skip(cols - 1).step_by(cols).sum::<Vec3>() / sim.rows() as f32;
        assert!(
            (settled - posed).length() < 0.2,
            "wind {wind:?}: sim settles its free edge at {settled}, far pose at {posed}"
        );
    }
}
