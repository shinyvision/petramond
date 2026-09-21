use super::*;

#[test]
fn a_primed_dig_pulses_at_once_and_a_long_frame_pulses_once() {
    let mut fresh = DigFeedback::primed();
    let first = fresh.advance(0.0);
    assert!(first.dust && first.hit);

    let mut clock = DigFeedback::default();
    let long = clock.advance(5.0);
    assert!(long.dust && long.hit);
    let next = clock.advance(0.01);
    assert!(
        !next.dust && !next.hit,
        "a long frame banks no extra pulses"
    );
}
