use super::*;

#[test]
fn monotone_anchor_search_matches_a_linear_scan_at_negative_and_empty_bounds() {
    for (lo, hi) in [(-64, 40), (-16, -1), (0, 0), (5, 3)] {
        for threshold in -70..=45 {
            let mut calls = 0;
            let fast = first_match(lo, hi, true, |y| {
                calls += 1;
                y >= threshold
            });
            assert_eq!(fast, first_match(lo, hi, false, |y| y >= threshold));
            assert!(calls <= 8);
        }
    }
}
