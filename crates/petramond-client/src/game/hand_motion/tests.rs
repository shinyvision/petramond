use super::*;

fn sample() -> MotionSample {
    MotionSample {
        position: Vec3::ZERO,
        velocity: Vec3::ZERO,
        yaw: 0.0,
        pitch: 0.0,
    }
}

#[test]
fn spring_recovery_is_independent_of_frame_partition() {
    let (mut x, mut v) = (Vec3::new(0.04, -0.02, 0.01), Vec3::new(0.2, -0.5, 0.1));
    let (mut a, mut b) = (x, v);
    settle(&mut x, &mut v, 16.0, 0.25);
    for _ in 0..30 {
        settle(&mut a, &mut b, 16.0, 0.25 / 30.0);
    }
    assert!(x.distance(a) < 1e-6 && v.distance(b) < 1e-6);
}

#[test]
fn landing_compresses_the_hand_then_recovers_without_retriggering() {
    let mut motion = HandMotion::default();
    motion.advance(
        1.0 / 60.0,
        MotionSample {
            velocity: Vec3::new(0.0, -9.0, 0.0),
            ..sample()
        },
        true,
    );
    motion.advance(1.0 / 60.0, sample(), true);
    assert!(motion.offset()[1] < 0.0);
    for _ in 0..120 {
        motion.advance(1.0 / 60.0, sample(), true);
    }
    assert!(Vec3::from(motion.offset()).length() < 1e-5);
}

#[test]
fn teleport_and_disabled_presentation_cannot_carry_an_impulse() {
    let mut motion = HandMotion::default();
    motion.advance(0.016, sample(), true);
    motion.advance(
        0.016,
        MotionSample {
            yaw: 0.2,
            ..sample()
        },
        true,
    );
    assert!(motion.offset()[0] > 0.0);
    motion.advance(
        0.016,
        MotionSample {
            position: Vec3::splat(50.0),
            ..sample()
        },
        true,
    );
    assert_eq!(motion.offset(), [0.0; 3]);
    motion.advance(0.016, sample(), false);
    motion.advance(
        0.016,
        MotionSample {
            yaw: 2.0,
            ..sample()
        },
        true,
    );
    assert_eq!(motion.offset(), [0.0; 3]);
}
