use glam::Vec3;

use super::sample_track;
use crate::bbmodel::{BezierHandles, Interpolation, Keyframe};
use Interpolation::*;

fn key(time: f32, v: f32, interpolation: Interpolation) -> Keyframe {
    Keyframe::new(time, Vec3::splat(v), interpolation)
}

fn at(keys: &[Keyframe], t: f32) -> f32 {
    sample_track(keys, t, false).x
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// three.js's uniform Catmull-Rom, written out independently of the sampler.
fn uniform_catmull(t: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2.0 * p1
        + (p2 - p0) * t
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t
        + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t * t * t)
}

#[test]
fn linear_keys_blend_and_hold_outside_the_keyed_range() {
    let keys = [key(0.5, 2.0, Linear), key(1.5, 6.0, Linear)];
    assert!(close(at(&keys, 0.0), 2.0));
    assert!(close(at(&keys, 1.0), 4.0));
    assert!(close(at(&keys, 9.0), 6.0));
}

#[test]
fn a_step_key_holds_until_the_next_key_arrives() {
    let keys = [key(0.0, 1.0, Step), key(1.0, 5.0, Linear)];
    assert!(close(at(&keys, 0.99), 1.0));
    assert!(close(at(&keys, 1.0), 5.0));
}

#[test]
fn a_two_point_key_arrives_at_its_pre_and_leaves_from_its_post() {
    let mut cut = key(1.0, 4.0, Linear);
    cut.post = Vec3::splat(10.0);
    cut.split = true;
    let keys = [key(0.0, 0.0, Linear), cut, key(2.0, 10.0, Linear)];
    assert!(close(at(&keys, 0.5), 2.0), "travels toward the arriving value");
    assert!(close(at(&keys, 1.0), 4.0), "on the key: the arriving value");
    assert!(close(at(&keys, 1.5), 10.0), "leaves from the leaving value");
}

/// The spline is UNIFORM over neighbouring key values at the time fraction —
/// not time-aware tangents — and a missing neighbour duplicates the end key.
/// That is what Blockbench previews, so it is what plays.
#[test]
fn catmull_rom_is_the_uniform_spline_over_neighbouring_keys() {
    let keys = [
        key(0.0, 0.0, CatmullRom),
        key(1.0, 3.0, CatmullRom),
        key(3.0, -1.0, CatmullRom),
        key(4.0, 2.0, CatmullRom),
    ];
    assert!(close(at(&keys, 2.0), uniform_catmull(0.5, 0.0, 3.0, -1.0, 2.0)));
    assert!(close(at(&keys, 0.25), uniform_catmull(0.25, 0.0, 0.0, 3.0, -1.0)));
    assert!(close(at(&keys, 3.5), uniform_catmull(0.5, 3.0, -1.0, 2.0, 2.0)));
}

#[test]
fn a_looping_catmull_rom_borrows_neighbours_across_the_wrap() {
    let keys = [
        key(0.0, 0.0, CatmullRom),
        key(1.0, 4.0, CatmullRom),
        key(2.0, 0.0, CatmullRom),
    ];
    let v = sample_track(&keys, 0.5, true).x;
    assert!(close(v, uniform_catmull(0.5, 4.0, 0.0, 4.0, 0.0)));
}

#[test]
fn a_mixed_pair_takes_catmull_rom_when_either_side_is() {
    let keys = [
        key(0.0, 0.0, Linear),
        key(1.0, 1.0, CatmullRom),
        key(2.0, 5.0, Linear),
    ];
    let v = at(&keys, 0.5);
    assert!(close(v, uniform_catmull(0.5, 0.0, 0.0, 1.0, 5.0)));
    assert!(!close(v, 0.5), "not the straight lerp");
}

#[test]
fn bezier_eases_through_flat_default_handles_and_lands_on_its_keys() {
    let mut a = key(0.0, 0.0, Bezier);
    a.bezier = Some(BezierHandles::DEFAULT);
    let mut b = key(1.0, 10.0, Bezier);
    b.bezier = Some(BezierHandles::DEFAULT);
    let keys = [a, b];
    assert!(close(at(&keys, 0.5), 5.0), "symmetric handles meet at the middle");
    assert!(at(&keys, 0.05) < 0.5, "flat handles ease out of the first key");
    assert!(close(at(&keys, 1.0), 10.0));
}

#[test]
fn a_bezier_value_handle_bends_the_curve_past_the_line() {
    let mut a = key(0.0, 0.0, Bezier);
    a.bezier = Some(BezierHandles {
        right_time: Vec3::splat(0.3),
        right_value: Vec3::splat(8.0),
        ..BezierHandles::DEFAULT
    });
    let mut b = key(1.0, 1.0, Bezier);
    b.bezier = Some(BezierHandles::DEFAULT);
    let keys = [a, b];
    assert!(
        at(&keys, 0.4) > 1.0,
        "the leaving handle carries the value past the next key's"
    );
}
