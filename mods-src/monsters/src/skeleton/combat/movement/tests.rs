use super::*;

#[test]
fn escape_candidates_include_backsteps_and_both_sides_and_only_keep_separating_legs() {
    let me = [0.0; 3];
    let foe = [0.0, 0.0, -2.0];
    let dirs = candidates(me, foe, 1.0);
    assert!(dirs[0][1] > 0.99);
    assert!(dirs.iter().any(|d| d[0] > 0.99));
    assert!(dirs.iter().any(|d| d[0] < -0.99));
    assert!(separates(me, foe, dirs[0]));
    assert!(separates(me, foe, dirs[3]));
    assert!(!separates(me, foe, *dirs.last().unwrap()));
}
