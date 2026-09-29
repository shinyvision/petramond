use crate::skeleton::aim::{aim_at, scatter, Flight, TICK_SECONDS};

/// An item entity in flight, stepped the way the engine steps one: gravity, drag on the whole
/// velocity, then the move.
fn closest_approach(flight: Flight, from: [f64; 3], vel: [f32; 3], target: [f64; 3]) -> f64 {
    let mut pos = from;
    let mut vel = vel.map(f64::from);
    let k = f64::from((1.0 - flight.drag).max(0.0).powf(TICK_SECONDS));
    let dt = f64::from(TICK_SECONDS);
    let mut best = f64::INFINITY;
    for _ in 0..400 {
        vel[1] -= f64::from(flight.gravity) * dt;
        vel = vel.map(|v| v * k);
        let next = [
            pos[0] + vel[0] * dt,
            pos[1] + vel[1] * dt,
            pos[2] + vel[2] * dt,
        ];
        best = best.min(segment_distance(pos, next, target));
        pos = next;
    }
    best
}

fn segment_distance(a: [f64; 3], b: [f64; 3], p: [f64; 3]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ap = [p[0] - a[0], p[1] - a[1], p[2] - a[2]];
    let len2 = ab.iter().map(|c| c * c).sum::<f64>().max(1e-12);
    let t = (ap.iter().zip(&ab).map(|(x, y)| x * y).sum::<f64>() / len2).clamp(0.0, 1.0);
    let q = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
}

#[test]
fn a_launched_arrow_arrives_on_a_standing_target() {
    let arrow = Flight {
        gravity: 20.0,
        drag: 0.2,
    };
    let from = [100.5, 70.4, -20.5];
    let speed = 28.0;
    for (dx, dy, dz) in [
        (4.0, 0.0, 0.0),
        (0.0, -1.0, -12.0),
        (-15.0, 6.0, 9.0),
        (17.0, -9.0, -14.0),
        (0.0, 3.0, 24.0),
    ] {
        let target = [from[0] + dx, from[1] + dy, from[2] + dz];
        let shot = aim_at(arrow, speed, from, target).expect("in range");
        let vel = shot.dir.map(|c| c * speed);
        let miss = closest_approach(arrow, from, vel, target);
        assert!(miss < 0.15, "missed ({dx}, {dy}, {dz}) by {miss}");
        let norm: f32 = shot.dir.iter().map(|c| c * c).sum();
        assert!((norm - 1.0).abs() < 1e-4);
    }
}

#[test]
fn a_target_beyond_the_shots_reach_is_not_aimed_at() {
    let slow = Flight {
        gravity: 20.0,
        drag: 0.0,
    };
    let from = [0.0, 64.0, 0.0];
    assert!(
        aim_at(slow, 10.0, from, [30.0, 64.0, 0.0]).is_none(),
        "v²/g is 5"
    );
    assert!(aim_at(slow, 10.0, from, [3.0, 64.0, 0.0]).is_some());
}

#[test]
fn scatter_keeps_the_shot_near_its_aim() {
    let dir = [0.6, 0.0, 0.8];
    assert_eq!(scatter(dir, 0.0, [1, 2]), dir);
    for rolls in [[0, 0], [u64::MAX, u64::MAX], [1 << 63, 12345]] {
        let off = scatter(dir, 2.0, rolls);
        let cos: f32 = off.iter().zip(dir).map(|(a, b)| a * b).sum();
        assert!(cos >= 3.0f32.to_radians().cos(), "{off:?} strayed");
    }
}
